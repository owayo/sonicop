"""Fuubar の実 Oracle 記録で、未測定の成功を拒む陰性対照。"""

import contextlib
import copy
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import record_fuubar as recorder


class FuubarRecorderTest(unittest.TestCase):
    """空・中断・欠落・別対象を、描画互換の証拠にしない。"""

    def record(self, name="disk-offense"):
        """実参照を壊す試験の入力となる、独立したコピーを返す。

        Returns:
            dict: 実参照記録の独立したコピー。

        """
        path = pathlib.Path(__file__).resolve().parents[2] / "tests/fixtures/fuubar_reports.json"
        document = json.loads(path.read_text(encoding="utf-8"))
        return copy.deepcopy(
            next(record for record in document["cases"] if record["case"]["name"] == name)
        )

    def test_complete_reference_and_explicit_empty_target_are_accepted(self):
        """本物の完了と、明示したゼロ対象を拒まない。"""
        for name in ("disk-offense", "empty", "fail-fast-equal-mtime"):
            with self.subTest(name=name):
                record = self.record(name)
                self.assertEqual(
                    recorder.validate_report(record["report"], record["case"]), record["report"]
                )

    def test_empty_success_for_positive_input_is_rejected(self):
        """正の対象を空集合の成功で代用しない。"""
        record = self.record()
        record["report"]["files"] = []
        record["report"]["summary"] = {
            "offense_count": 0,
            "target_file_count": 0,
            "inspected_file_count": 0,
        }
        with self.assertRaises(ValueError):
            recorder.validate_report(record["report"], record["case"])

    def test_wrong_or_duplicate_paths_are_rejected(self):
        """別対象や重複した検査を完了集合へ混ぜない。"""
        for duplicate in (False, True):
            with self.subTest(duplicate=duplicate):
                record = self.record()
                if duplicate:
                    record["report"]["files"].append(copy.deepcopy(record["report"]["files"][0]))
                else:
                    record["report"]["files"][0]["path"] = "other.rb"
                with self.assertRaises(ValueError):
                    recorder.validate_report(record["report"], record["case"])

    def test_incomplete_fields_and_wrong_cop_are_rejected(self):
        """診断全項目や対象 cop の欠落を捕捉する。"""
        for change in ("message", "location", "cop"):
            with self.subTest(change=change):
                record = self.record()
                offense = record["report"]["files"][0]["offenses"][0]
                if change == "message":
                    del offense[change]
                elif change == "location":
                    del offense[change]["last_column"]
                else:
                    offense["cop_name"] = "Lint/Syntax"
                with self.assertRaises(ValueError):
                    recorder.validate_report(record["report"], record["case"])

    def test_incomplete_summary_is_rejected(self):
        """対象数・完了数・診断数の不整合を受け取らない。"""
        for field in ("offense_count", "target_file_count", "inspected_file_count"):
            with self.subTest(field=field):
                record = self.record()
                record["report"]["summary"][field] = 0
                with self.assertRaises(ValueError):
                    recorder.validate_report(record["report"], record["case"])

    def test_missing_rendered_diagnostic_and_summary_are_rejected(self):
        """bar しか描かない CLI を実診断の記録にしない。"""
        record = self.record()
        for text in ("", "\nProgress: |", "\nProgress: |\n1 file inspected, 1 offense detected\n"):
            with self.subTest(text=text):
                with self.assertRaises(ValueError):
                    recorder.validate_render(text, record["report"])

    def test_error_exits_are_rejected(self):
        """実行エラーを普通の診断差と区別する。"""
        for exit_code in (2, -1):
            result = subprocess.CompletedProcess([], exit_code, b"", b"error")
            with patch.object(recorder.subprocess, "run", return_value=result):
                with self.assertRaises(ValueError):
                    recorder.run(["fake"], pathlib.Path("."), {}, None)

    def test_wrong_runtime_is_rejected_before_measurement(self):
        """固定版をロードできなければ測定へ進まない。"""
        result = subprocess.CompletedProcess([], 0, b"{}", b"")
        with tempfile.TemporaryDirectory() as scratch:
            with patch.object(recorder.subprocess, "run", return_value=result) as run:
                with self.assertRaises(ValueError):
                    recorder.collect("fake", pathlib.Path(scratch))
                self.assertEqual(run.call_count, 1)

    def observe_first_child(self, keys):
        """採取器の版確認へ渡す環境を実子で観測し、全33条件へ進まない。

        Returns:
            dict: 実子が見た指定キーの値。

        """

        class StopAfterProbe(Exception):
            """実子を一つ観測した後は、本家の全採取へ戻らない。"""

        original_run = subprocess.run
        observed = []

        def probe(*_args, **options):
            """mockの引数だけでなく、実プロセスの環境を確かめる。

            Raises:
                StopAfterProbe: 最初の子の環境を観測した直後。

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
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(recorder.subprocess, "run", side_effect=probe):
                with self.assertRaises(StopAfterProbe):
                    recorder.collect("unused-reference", pathlib.Path(directory))
        self.assertTrue(dict(os.environ) == parent, "採取器が親ENVを変更した")
        self.assertEqual(len(observed), 1)
        return observed[0]

    def test_actual_first_child_drops_four_parent_overrides(self):
        """Rubyのrequire注入と本家optionを、実子の版確認へ持ち込まない。"""
        poison = {
            "RUBYOPT": "-rsonicop_fuubar_env_missing",
            "RUBYLIB": "unexpected-load-path",
            "RUBOCOP_OPTS": "--only Unknown/Cop",
            "RUBOCOP_TARGET_RUBY_VERSION": "0.0",
        }
        with patch.dict(os.environ, poison):
            self.assertEqual(self.observe_first_child(list(poison)), {key: None for key in poison})

    def test_actual_first_child_keeps_runtime_paths_and_parent(self):
        """Gem・DLL探索と親の独立した値を、環境の一括消去から守る。"""
        keys = ["GEM_HOME", "GEM_PATH", "PATH", "SystemRoot", "OS", "SONICOP_ENV_PARENT_SENTINEL"]
        with patch.dict(os.environ, {"SONICOP_ENV_PARENT_SENTINEL": "親環境を保持する"}):
            expected = {key: os.environ.get(key) for key in keys}
            self.assertEqual(self.observe_first_child(keys), expected)


class FuubarPublicationTest(unittest.TestCase):
    """採取内容をmockへ固定し、公開先の所有と失敗証拠だけを検査する。"""

    def document(self, selected):
        """元fixtureを変更せず、全件と既存--caseの公開経路へ渡す。"""
        path = pathlib.Path(__file__).resolve().parents[2] / "tests/fixtures/fuubar_reports.json"
        document = json.loads(path.read_text(encoding="utf-8"))
        if selected is not None:
            document["cases"] = [
                case for case in document["cases"] if case["case"]["name"] == selected
            ]
        return document

    def invoke(self, root, output, selected, collected, extra=()):
        """子起動を禁止し、work作成とcollector到達の順を保存する。"""
        temporary = tempfile.TemporaryDirectory
        observed = {"work": 0, "children": 0, "result": None, "failure": None}
        stdout = io.StringIO()

        def work(*_args, **_kwargs):
            """事前拒否より先にscratchが確保された場合も検出する。"""
            observed["work"] += 1
            return temporary(prefix="owned-work-", dir=root)

        def no_child(*_args, **_kwargs):
            """Oracleを起動する誤った経路を、この契約では成功させない。"""
            observed["children"] += 1
            raise AssertionError("publication contract must not start a child")

        argv = ["--ruby", "unused-ruby", "--output", str(output)]
        if selected is not None:
            argv.extend(["--case", selected])
        with contextlib.ExitStack() as stack:
            for manager in extra:
                stack.enter_context(manager)
            stack.enter_context(
                patch.object(recorder.tempfile, "TemporaryDirectory", side_effect=work)
            )
            collect = stack.enter_context(patch.object(recorder, "collect", side_effect=collected))
            stack.enter_context(patch.object(recorder.subprocess, "run", side_effect=no_child))
            stack.enter_context(contextlib.redirect_stdout(stdout))
            try:
                observed["result"] = recorder.main(argv)
            except Exception as error:
                observed["failure"] = error
            observed["collect"] = collect.call_count
        observed["stdout"] = stdout.getvalue()
        return observed

    def assert_preflight_rejection(self, observed):
        """占有・予約の失敗で子やworkを残してはならない。"""
        self.assertIsNotNone(observed["failure"])
        self.assertIsNone(observed["result"])
        self.assertEqual(observed["work"], 0)
        self.assertEqual(observed["collect"], 0)
        self.assertEqual(observed["children"], 0)
        self.assertEqual(observed["stdout"], "")

    def test_existing_file_is_refused_before_work_or_collection(self):
        """既存bytesとmtimeを保ち、採取前に占有を拒否する。"""
        for selected in (None, "disk-offense"):
            with self.subTest(selected=selected), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = root / "existing.json"
                output.write_bytes(b"foreign\x00\r\n")
                before = (output.read_bytes(), output.stat().st_mtime_ns)
                observed = self.invoke(
                    root, output, selected, lambda *_, selected=selected: self.document(selected)
                )
                self.assert_preflight_rejection(observed)
                self.assertEqual((output.read_bytes(), output.stat().st_mtime_ns), before)
                self.assertEqual(set(root.iterdir()), {output})

    def test_existing_directory_is_refused_before_work_or_collection(self):
        """directoryも占有済みleafであり、中の他者fileを変更しない。"""
        for selected in (None, "disk-offense"):
            with self.subTest(selected=selected), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = root / "existing.json"
                output.mkdir()
                sentinel = output / "foreign"
                sentinel.write_bytes(b"keep")
                before = (sentinel.read_bytes(), sentinel.stat().st_mtime_ns)
                observed = self.invoke(
                    root, output, selected, lambda *_, selected=selected: self.document(selected)
                )
                self.assert_preflight_rejection(observed)
                self.assertEqual((sentinel.read_bytes(), sentinel.stat().st_mtime_ns), before)

    def test_missing_parent_is_refused_before_work_or_collection(self):
        """親を暗黙作成せず、予約できない場合は採取を始めない。"""
        for selected in (None, "disk-offense"):
            with self.subTest(selected=selected), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = root / "missing" / "new.json"
                observed = self.invoke(
                    root, output, selected, lambda *_, selected=selected: self.document(selected)
                )
                self.assert_preflight_rejection(observed)
                self.assertIsInstance(observed["failure"], FileNotFoundError)
                self.assertEqual(list(root.iterdir()), [])

    def test_dangling_leaf_is_refused_without_resolving_or_children(self):
        """実OS symlink成功を仮定せず、元leafのAPI分岐をstubで守る。"""
        for selected in (None, "disk-offense"):
            with self.subTest(selected=selected), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = root / "dangling.json"
                target = root / "unoccupied-target.json"
                exists, symlink, resolve = (
                    pathlib.Path.exists,
                    pathlib.Path.is_symlink,
                    pathlib.Path.resolve,
                )
                resolved = []

                def leaf_exists(path, output=output, exists=exists):
                    return False if path == output else exists(path)

                def leaf_symlink(path, output=output, symlink=symlink):
                    return True if path == output else symlink(path)

                def leaf_resolve(
                    path,
                    *args,
                    output=output,
                    resolved=resolved,
                    target=target,
                    resolve=resolve,
                    **kwargs,
                ):
                    if path == output:
                        resolved.append(path)
                        return target
                    return resolve(path, *args, **kwargs)

                extra = (
                    patch.object(pathlib.Path, "exists", leaf_exists),
                    patch.object(pathlib.Path, "is_symlink", leaf_symlink),
                    patch.object(pathlib.Path, "resolve", leaf_resolve),
                )
                observed = self.invoke(
                    root,
                    output,
                    selected,
                    lambda *_, selected=selected: self.document(selected),
                    extra,
                )
                self.assert_preflight_rejection(observed)
                self.assertEqual(resolved, [])
                self.assertFalse(output.exists())
                self.assertFalse(target.exists())
                self.assertEqual(list(root.iterdir()), [])

    def test_reservation_race_keeps_peer_bytes_and_starts_no_work(self):
        """precheck後の競合をxbで拒否し、他者のbytesとmtimeを保つ。"""
        for selected in (None, "disk-offense"):
            with self.subTest(selected=selected), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = root / "race.json"
                original = pathlib.Path.open
                peer = []

                def competing_open(
                    path, mode="r", *args, output=output, original=original, peer=peer, **kwargs
                ):
                    if path == output and mode == "xb":
                        with original(path, "wb") as stream:
                            stream.write(b"peer\x00\r\n")
                        peer.append((output.read_bytes(), output.stat().st_mtime_ns))
                    return original(path, mode, *args, **kwargs)

                observed = self.invoke(
                    root,
                    output,
                    selected,
                    lambda *_, selected=selected: self.document(selected),
                    (patch.object(pathlib.Path, "open", competing_open),),
                )
                self.assert_preflight_rejection(observed)
                self.assertIsInstance(observed["failure"], FileExistsError)
                self.assertEqual(len(peer), 1)
                self.assertEqual((output.read_bytes(), output.stat().st_mtime_ns), peer[0])

    def test_child_or_parse_failure_keeps_empty_reservation_without_success(self):
        """採取の中断で新0B予約を消したり、成功を表示したりしない。"""
        for selected in (None, "disk-offense"):
            for failure in (ValueError("child failure"), json.JSONDecodeError("parse", "", 0)):
                with self.subTest(selected=selected, failure=type(failure).__name__):
                    with tempfile.TemporaryDirectory() as folder:
                        root = pathlib.Path(folder)
                        output = root / "failed.json"
                        observed = self.invoke(root, output, selected, failure)
                        self.assertIs(observed["failure"], failure)
                        self.assertIsNone(observed["result"])
                        self.assertEqual(observed["collect"], 1)
                        self.assertEqual(observed["work"], 1)
                        self.assertEqual(observed["children"], 0)
                        self.assertEqual(observed["stdout"], "")
                        self.assertTrue(output.is_file())
                        self.assertEqual(output.read_bytes(), b"")
                        self.assertEqual(set(root.iterdir()), {output})

    def test_partial_write_failure_keeps_bytes_without_success(self):
        """同じ予約handleのpartialを保持し、不完全なJSONを完成と呼ばない。"""
        for selected in (None, "disk-offense"):
            with self.subTest(selected=selected), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = root / "partial.json"
                document = self.document(selected)
                encoded = (json.dumps(document, ensure_ascii=False, indent=2) + "\n").encode(
                    "utf-8"
                )
                original = pathlib.Path.open

                class PartialWrite:
                    def __init__(self, stream):
                        self.stream = stream

                    def __enter__(self):
                        return self

                    def __exit__(self, *_args):
                        self.stream.close()

                    def write(self, data):
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
                    stream = original(path, mode, *args, **kwargs)
                    return wrapper(stream) if path == output and mode == "xb" else stream

                observed = self.invoke(
                    root,
                    output,
                    selected,
                    lambda *_, document=document: document,
                    (patch.object(pathlib.Path, "open", partial_open),),
                )
                self.assertIsInstance(observed["failure"], OSError)
                self.assertIsNone(observed["result"])
                self.assertEqual(observed["collect"], 1)
                self.assertEqual(observed["work"], 1)
                self.assertEqual(observed["children"], 0)
                self.assertEqual(observed["stdout"], "")
                self.assertEqual(output.read_bytes(), encoded[:7])

    def test_success_reserves_before_collection_and_keeps_exact_document(self):
        """全件・単独caseとも先に予約し、保存JSON値とLF容器を変更しない。"""
        for selected in (None, "disk-offense"):
            with self.subTest(selected=selected), tempfile.TemporaryDirectory() as folder:
                root = pathlib.Path(folder)
                output = root / "new.json"
                document = self.document(selected)
                encoded = (json.dumps(document, ensure_ascii=False, indent=2) + "\n").encode(
                    "utf-8"
                )
                reserved = []

                def collected(
                    ruby,
                    work,
                    case,
                    *,
                    reserved=reserved,
                    output=output,
                    selected=selected,
                    document=document,
                ):
                    reserved.append(output.is_file() and output.read_bytes() == b"")
                    self.assertEqual(ruby, "unused-ruby")
                    self.assertTrue(work.is_dir())
                    self.assertEqual(case, selected)
                    return document

                observed = self.invoke(root, output, selected, collected)
                self.assertIsNone(observed["failure"])
                self.assertEqual(observed["result"], 0)
                self.assertEqual(observed["collect"], 1)
                self.assertEqual(observed["work"], 1)
                self.assertEqual(observed["children"], 0)
                self.assertEqual(reserved, [True])
                self.assertEqual(output.read_bytes(), encoded)
                self.assertEqual(json.loads(output.read_bytes()), document)
                self.assertEqual(
                    observed["stdout"],
                    f"cases={len(document['cases'])} sha256={recorder.hashlib.sha256(encoded).hexdigest()}\n",
                )


if __name__ == "__main__":
    unittest.main()
