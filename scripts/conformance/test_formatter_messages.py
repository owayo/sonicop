"""実 Oracle 記録が空集合・別対象・中断を成功扱いしないことを検査する。"""

import copy
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import record_formatter_messages as recorder


class FormatterMessageRecorderTest(unittest.TestCase):
    """未測定の成功を拒否する recorder の陰性対照。"""

    def report(self):
        """実 Oracle の完全な 1 診断を、破損させる試験の入力に使う。

        Returns:
            dict: fixture の本家診断と summary の独立したコピー。

        """
        fixture = (
            pathlib.Path(__file__).resolve().parents[2] / "tests/fixtures/formatter_messages.json"
        )
        document = json.loads(fixture.read_text(encoding="utf-8"))
        return copy.deepcopy(document["cases"][0]["outputs"][0]["report"])

    def test_real_complete_record_is_accepted(self):
        """完全な実参照記録を拒まない陰性対照。"""
        report = self.report()
        self.assertEqual(recorder.validate_report(report), report)

    def test_empty_success_is_rejected(self):
        """対象を何も検査していない終了成功を拒む。"""
        with self.assertRaises(ValueError):
            recorder.validate_report({"files": [], "summary": {"offense_count": 0}})

    def test_duplicate_or_wrong_paths_are_rejected(self):
        """別対象や重複対象を、この 1 ファイルの証拠にしない。"""
        for duplicate in (True, False):
            with self.subTest(duplicate=duplicate):
                report = self.report()
                if duplicate:
                    report["files"].append(copy.deepcopy(report["files"][0]))
                else:
                    report["files"][0]["path"] = "other.rb"
                with self.assertRaises(ValueError):
                    recorder.validate_report(report)

    def test_missing_offense_and_incomplete_summary_are_rejected(self):
        """診断や完了件数の欠落を実測成功にしない。"""
        for key in (
            "offenses",
            "cop",
            "target_file_count",
            "inspected_file_count",
            "offense_count",
        ):
            with self.subTest(key=key):
                report = self.report()
                if key == "offenses":
                    report["files"][0]["offenses"] = []
                elif key == "cop":
                    report["files"][0]["offenses"][0]["cop_name"] = "Lint/Syntax"
                else:
                    report["summary"][key] = 0
                with self.assertRaises(ValueError):
                    recorder.validate_report(report)

    def test_error_exits_and_stderr_are_rejected(self):
        """参照 CLI の中断やエラー出力を差分と混ぜない。"""
        for exit_code, stderr in ((2, b""), (-1, b""), (0, b"warning"), (1, b"error")):
            with self.subTest(exit_code=exit_code, stderr=stderr):
                result = subprocess.CompletedProcess([], exit_code, b"", stderr)
                with patch.object(recorder.subprocess, "run", return_value=result):
                    with self.assertRaises(ValueError):
                        recorder.execute(["fake"], pathlib.Path("."), {})

    def test_incomplete_message_or_location_is_rejected(self):
        """診断の一部だけがある記録を全項目比較に使わない。"""
        for key in ("message", "location"):
            with self.subTest(key=key):
                report = self.report()
                if key == "message":
                    del report["files"][0]["offenses"][0][key]
                else:
                    del report["files"][0]["offenses"][0][key]["last_column"]
                with self.assertRaises(ValueError):
                    recorder.validate_report(report)

    def test_wrong_reference_runtime_is_rejected_before_measurement(self):
        """仕様版と違う参照環境では測定を開始しない。"""
        result = subprocess.CompletedProcess([], 0, b"{}", b"")
        with tempfile.TemporaryDirectory() as scratch:
            with patch.object(recorder.subprocess, "run", return_value=result) as run:
                with self.assertRaises(ValueError):
                    recorder.collect("fake", pathlib.Path(scratch))
                self.assertEqual(run.call_count, 1)

    def observe_first_child(self, keys):
        """実collectの最初の子環境を実Pythonへ渡し、全35条件に進む前に止める。

        Returns:
            dict: 実子が観測した指定キーの値。

        """

        class StopAfterProbe(Exception):
            """版確認後の全条件採取を行わないため、最初の子の観測で停止する。"""

        original_run = subprocess.run
        observed = []

        def probe(*_args, **options):
            """実プロセスの観測だけを行い、元の参照ループへ戻らない。

            Raises:
                StopAfterProbe: 子の環境を観測した直後。

            """
            command = [
                sys.executable,
                "-c",
                "import json,os; print(json.dumps({key:os.environ.get(key) for key in "
                + repr(keys)
                + "}))",
            ]
            child = original_run(command, env=options["env"], capture_output=True, check=True)
            observed.append(json.loads(child.stdout))
            raise StopAfterProbe

        parent = dict(os.environ)
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(recorder.subprocess, "run", side_effect=probe):
                with self.assertRaises(StopAfterProbe):
                    recorder.collect("unused-reference", pathlib.Path(temporary))
        self.assertTrue(dict(os.environ) == parent, "採取器が親ENVを変更した")
        self.assertEqual(len(observed), 1)
        return observed[0]

    def test_actual_first_child_drops_four_parent_overrides(self):
        """親の起動注入が最初の版確認childへ到達しないことを陰性対照で守る。"""
        poison = {
            "RUBYOPT": "-rsonicop_formatter_env_missing",
            "RUBYLIB": "unexpected-load-path",
            "RUBOCOP_OPTS": "--only Unknown/Cop",
            "RUBOCOP_TARGET_RUBY_VERSION": "0.0",
        }
        with patch.dict(os.environ, poison):
            self.assertEqual(self.observe_first_child(list(poison)), {key: None for key in poison})

    def test_actual_first_child_keeps_runtime_paths_and_parent(self):
        """空環境への置換でGem・Windows環境や親の独立した値を消さない。"""
        keys = ["GEM_HOME", "GEM_PATH", "PATH", "SystemRoot", "OS", "SONICOP_ENV_PARENT_SENTINEL"]
        with patch.dict(os.environ, {"SONICOP_ENV_PARENT_SENTINEL": "親環境を保持する"}):
            expected = {key: os.environ.get(key) for key in keys}
            self.assertEqual(self.observe_first_child(keys), expected)


class FormatterMessagePublicationTest(unittest.TestCase):
    """実本家を起動せず、公開先の所有と失敗時の証拠保持を検査する。"""

    def document(self, cache_controls):
        """既存fixtureの値を変えず、保存だけを試す入力として読み込む。

        Returns:
            dict: 通常35件または追加cache9件の既存参照値。

        """
        filename = (
            "formatter_message_cache_controls.json" if cache_controls else "formatter_messages.json"
        )
        path = pathlib.Path(__file__).resolve().parents[2] / "tests/fixtures" / filename
        return json.loads(path.read_text(encoding="utf-8"))

    def output(self, root, cache_controls):
        """各invocationが宣言する1公開先だけを選ぶ。

        Returns:
            Path: 私用test directory内の出力先。

        """
        return root / (
            "formatter_message_cache_controls.json" if cache_controls else "formatter_messages.json"
        )

    def invoke(self, root, output, cache_controls, response, extra=()):
        """既存mainを通し、採取・scratch作成・子起動と成功表示を観測する。

        Returns:
            dict: 例外と各処理の呼出し回数。実Rubyやnativeは起動しない。

        """
        import contextlib
        import io

        temporary = tempfile.TemporaryDirectory

        def scratch(*args, **kwargs):
            """mainの所有scratchも新test namespace内へ閉じる。

            Returns:
                TemporaryDirectory: 他者の既存workを使わない一時領域。

            """
            return temporary(*args, dir=root, **kwargs)

        args = ["--ruby", "unused-ruby", "--output", str(output)]
        if cache_controls:
            args.append("--cache-controls")
        stdout = io.StringIO()
        failure = None
        result = None
        with contextlib.ExitStack() as stack:
            for manager in extra:
                stack.enter_context(manager)
            collection = stack.enter_context(
                patch.object(recorder, "collect", side_effect=response)
            )
            work = stack.enter_context(
                patch.object(recorder.tempfile, "TemporaryDirectory", side_effect=scratch)
            )
            children = stack.enter_context(
                patch.object(
                    recorder.subprocess,
                    "run",
                    side_effect=AssertionError("公開契約testが実childを起動した"),
                )
            )
            stack.enter_context(patch.object(sys, "stdout", stdout))
            try:
                result = recorder.main(args)
            except Exception as error:
                failure = error
        return {
            "failure": failure,
            "result": result,
            "collect": collection.call_count,
            "collect_args": collection.call_args,
            "work": work.call_count,
            "children": children.call_count,
            "stdout": stdout.getvalue(),
        }

    def assert_preflight_rejection(self, observation):
        """占有や予約競合でOracleやworkへ進んでいないことを要求する。"""
        self.assertIsNotNone(observation["failure"])
        self.assertEqual(observation["collect"], 0)
        self.assertEqual(observation["work"], 0)
        self.assertEqual(observation["children"], 0)
        self.assertEqual(observation["stdout"], "")

    def test_existing_file_is_refused_before_scratch_or_collection(self):
        """通常/追加cacheの各公開先で他者のbytesとmtimeを保つ。"""
        for cached in (False, True):
            with self.subTest(cache_controls=cached), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = self.output(root, cached)
                output.write_bytes(b"foreign\x00sentinel\r\n")
                before = (output.read_bytes(), output.stat().st_mtime_ns)
                document = self.document(cached)
                observed = self.invoke(root, output, cached, lambda *_, document=document: document)
                self.assert_preflight_rejection(observed)
                self.assertIsInstance(observed["failure"], ValueError)
                self.assertEqual((output.read_bytes(), output.stat().st_mtime_ns), before)
                self.assertEqual(set(root.iterdir()), {output})

    def test_existing_directory_is_refused_before_scratch_or_collection(self):
        """ディレクトリも占有済みleafとして事前に拒む。"""
        for cached in (False, True):
            with self.subTest(cache_controls=cached), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = self.output(root, cached)
                output.mkdir()
                (output / "sentinel").write_bytes(b"foreign")
                observed = self.invoke(
                    root, output, cached, lambda *_, cached=cached: self.document(cached)
                )
                self.assert_preflight_rejection(observed)
                self.assertEqual((output / "sentinel").read_bytes(), b"foreign")
                self.assertEqual(set(root.iterdir()), {output})

    def test_dangling_leaf_is_refused_without_resolution_or_children(self):
        """OS symlinkを仮定せず、exists=false/is_symlink=trueのAPI分岐を守る。"""
        for cached in (False, True):
            with self.subTest(cache_controls=cached), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = self.output(root, cached)
                target = root / "unoccupied-target"
                document = self.document(cached)
                exists = pathlib.Path.exists
                symlink = pathlib.Path.is_symlink
                resolve = pathlib.Path.resolve
                resolved = []

                def virtual_exists(path, output=output, exists=exists):
                    """dangling leafの存在判定だけをstubする。

                    Returns:
                        bool: leaf以外は実APIの結果。

                    """
                    return False if path == output else exists(path)

                def virtual_symlink(path, output=output, symlink=symlink):
                    """元leafの所有を、解決先の欠如と分ける。

                    Returns:
                        bool: leaf以外は実APIの結果。

                    """
                    return True if path == output else symlink(path)

                def virtual_resolve(
                    path,
                    *args,
                    output=output,
                    resolved=resolved,
                    target=target,
                    resolve=resolve,
                    **kwargs,
                ):
                    """leafを解決してから検査する回帰を検出する。

                    Returns:
                        Path: leafの仮想解決先または実APIの結果。

                    """
                    if path == output:
                        resolved.append(path)
                        return target
                    return resolve(path, *args, **kwargs)

                extra = (
                    patch.object(pathlib.Path, "exists", virtual_exists),
                    patch.object(pathlib.Path, "is_symlink", virtual_symlink),
                    patch.object(pathlib.Path, "resolve", virtual_resolve),
                )
                observed = self.invoke(
                    root, output, cached, lambda *_, document=document: document, extra
                )
                self.assert_preflight_rejection(observed)
                self.assertEqual(resolved, [])
                self.assertFalse(output.exists())
                self.assertFalse(target.exists())
                self.assertEqual(list(root.iterdir()), [])

    def test_reservation_race_is_refused_before_scratch_or_collection(self):
        """事前確認直後に別所有者が現れたら、排他予約で子0のまま拒む。"""
        for cached in (False, True):
            with self.subTest(cache_controls=cached), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = self.output(root, cached)
                document = self.document(cached)
                original = pathlib.Path.open
                peer = []

                def competing_open(
                    path, mode="r", *args, output=output, original=original, peer=peer, **kwargs
                ):
                    """元leafの予約直前に他者の公開を差し込む。

                    Returns:
                        IO: 排他openが拒否しなければ元APIの結果。

                    """
                    if path == output and mode == "xb":
                        with original(path, "wb") as stream:
                            stream.write(b"race-owner\x00\r\n")
                        peer.append((output.read_bytes(), output.stat().st_mtime_ns))
                    return original(path, mode, *args, **kwargs)

                observed = self.invoke(
                    root,
                    output,
                    cached,
                    lambda *_, document=document: document,
                    (patch.object(pathlib.Path, "open", competing_open),),
                )
                self.assert_preflight_rejection(observed)
                self.assertIsInstance(observed["failure"], FileExistsError)
                self.assertEqual(len(peer), 1)
                self.assertEqual((output.read_bytes(), output.stat().st_mtime_ns), peer[0])
                self.assertEqual(set(root.iterdir()), {output})

    def test_collection_or_parse_failure_keeps_empty_reservation_without_success(self):
        """採取失敗を成功表示せず、新規0B予約を失敗の証拠として残す。"""
        for cached in (False, True):
            for failure in (ValueError("child failure"), json.JSONDecodeError("parse", "", 0)):
                with self.subTest(cache_controls=cached, failure=type(failure).__name__):
                    with tempfile.TemporaryDirectory() as folder:
                        root = pathlib.Path(folder)
                        output = self.output(root, cached)
                        observed = self.invoke(root, output, cached, failure)
                        self.assertIs(observed["failure"], failure)
                        self.assertEqual(observed["collect"], 1)
                        self.assertEqual(observed["work"], 1)
                        self.assertEqual(observed["children"], 0)
                        self.assertEqual(observed["stdout"], "")
                        self.assertTrue(output.is_file())
                        self.assertEqual(output.read_bytes(), b"")

    def test_partial_write_failure_keeps_owned_bytes_without_success(self):
        """途中書込みの予約fileを自動削除せず、不完全な記録は成功扱いしない。"""
        for cached in (False, True):
            with self.subTest(cache_controls=cached), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = self.output(root, cached)
                document = self.document(cached)
                encoded = (json.dumps(document, ensure_ascii=False, indent=2) + "\n").encode()
                original = pathlib.Path.open

                class PartialWrite:
                    """本物の予約handleへ先頭だけを書き、I/O失敗を差し込む。"""

                    def __init__(self, stream):
                        self.stream = stream

                    def __enter__(self):
                        return self

                    def __exit__(self, *_args):
                        self.stream.close()

                    def write(self, data):
                        """部分bytesをflushしてから中断する。

                        Raises:
                            OSError: 検証用の途中書込み失敗。

                        """
                        self.stream.write(data[:7])
                        self.stream.flush()
                        raise OSError("partial publication")

                def partial_open(
                    path,
                    mode="r",
                    *args,
                    output=output,
                    original=original,
                    wrapper=PartialWrite,
                    **kwargs,
                ):
                    """予約handleだけを書込み失敗用wrapperへ包む。

                    Returns:
                        IO: 予約先のwrapperまたは通常handle。

                    """
                    stream = original(path, mode, *args, **kwargs)
                    return wrapper(stream) if path == output and mode == "xb" else stream

                observed = self.invoke(
                    root,
                    output,
                    cached,
                    lambda *_, document=document: document,
                    (patch.object(pathlib.Path, "open", partial_open),),
                )
                self.assertIsInstance(observed["failure"], OSError)
                self.assertEqual(observed["collect"], 1)
                self.assertEqual(observed["work"], 1)
                self.assertEqual(observed["children"], 0)
                self.assertEqual(observed["stdout"], "")
                self.assertEqual(output.read_bytes(), encoded[:7])

    def test_success_reserves_first_and_keeps_fixture_bytes_and_other_output(self):
        """active公開先だけを予約し、既存の反対modeのfixtureを変更しない。"""
        for cached in (False, True):
            with self.subTest(cache_controls=cached), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = self.output(root, cached)
                other = self.output(root, not cached)
                other.write_bytes(b"other-mode\x00\r\n")
                before = (other.read_bytes(), other.stat().st_mtime_ns)
                document = self.document(cached)
                encoded = (json.dumps(document, ensure_ascii=False, indent=2) + "\n").encode()
                reserved = []

                def collected(
                    ruby,
                    work,
                    cache_controls,
                    *,
                    reserved=reserved,
                    output=output,
                    cached=cached,
                    document=document,
                ):
                    """collect到達時に先に予約されたfileと元modeを確認する。

                    Returns:
                        dict: 一切書き換えていない参照fixture値。

                    """
                    reserved.append(output.is_file() and output.read_bytes() == b"")
                    self.assertEqual(ruby, "unused-ruby")
                    self.assertTrue(work.is_dir())
                    self.assertEqual(cache_controls, cached)
                    return document

                observed = self.invoke(root, output, cached, collected)
                self.assertIsNone(observed["failure"])
                self.assertEqual(observed["result"], 0)
                self.assertEqual(observed["collect"], 1)
                self.assertEqual(observed["work"], 1)
                self.assertEqual(observed["children"], 0)
                self.assertEqual(reserved, [True])
                self.assertEqual(output.read_bytes(), encoded)
                self.assertEqual(json.loads(output.read_bytes()), document)
                self.assertEqual((other.read_bytes(), other.stat().st_mtime_ns), before)
                self.assertEqual(
                    observed["stdout"],
                    f"cases={len(document['cases'])} "
                    f"sha256={recorder.hashlib.sha256(encoded).hexdigest()}\n",
                )


if __name__ == "__main__":
    unittest.main()
