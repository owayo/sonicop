"""期待値収集の子環境を実プロセスで確かめ、親の設定汚染を記録へ混ぜない。"""

import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

_SCRIPT = Path(__file__).resolve().parents[1] / "record_autogenconf_console.py"
_SPEC = importlib.util.spec_from_file_location("autogenconf_collector", _SCRIPT)
_COLLECTOR = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_COLLECTOR)


def _child_environment(keys):
    child = (
        "import json,os,sys; "
        "sys.stdout.write(json.dumps({key:os.environ.get(key) for key in " + repr(keys) + "}))"
    )
    result = _COLLECTOR._run(
        [sys.executable, "-c", child],
        {"name": "child-environment", "files": {}, "args": []},
    )
    return json.loads(result["stdout"])


class AutogenconfCollectorEnvironmentTest(unittest.TestCase):
    """Ruby が起動する前の環境契約を、mock の引数ではなく子の観測値で守る。"""

    def test_poison_options_are_absent_in_child_and_parent_is_unchanged(self):
        """読み込みと対象版を変える4変数、および色の無効化を親から引き継がない。"""
        poison = {
            "RUBYOPT": "-rsonicop_missing_library",
            "RUBYLIB": "sonicop-unexpected-load-path",
            "RUBOCOP_OPTS": "--only Unknown/Cop",
            "RUBOCOP_TARGET_RUBY_VERSION": "0.0",
            "NO_COLOR": "parent-disables-colors",
        }
        with mock.patch.dict(os.environ, poison):
            parent = dict(os.environ)
            observed = _child_environment(list(poison))
            self.assertEqual(observed, {key: None for key in poison})
            self.assertEqual(dict(os.environ), parent)

    def test_runtime_search_paths_and_parent_sentinel_reach_child_unchanged(self):
        """環境の全消去で gem・DLL 探索や通常の親変数まで失わない。"""
        with tempfile.TemporaryDirectory(prefix="sonicop-env-contract-") as temporary:
            root = Path(temporary)
            preserved = {
                "GEM_HOME": str(root / "gem-home-sentinel"),
                "GEM_PATH": str(root / "gem-path-sentinel"),
                "PATH": str(root / "path-sentinel") + os.pathsep + os.environ["PATH"],
                "SystemRoot": os.environ.get("SystemRoot", "system-root-sentinel"),
                "OS": os.environ.get("OS", "os-sentinel"),
                "SONICOP_COLLECTOR_PARENT_SENTINEL": "親環境の値を保持する",
            }
            with mock.patch.dict(os.environ, preserved):
                parent = dict(os.environ)
                self.assertEqual(_child_environment(list(preserved)), preserved)
                self.assertEqual(dict(os.environ), parent)


class AutogenconfCollectorPublicationTest(unittest.TestCase):
    """保存済みの33+2値だけをstubに使い、出力予約と失敗時の所有境界を守る。"""

    def setUp(self):
        """固定の参照値と、各契約が所有する一時pathを分ける。"""
        self.temporary = tempfile.TemporaryDirectory(prefix="autogenconf-publication-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        fixture = Path(__file__).resolve().parents[2] / "tests/fixtures/autogenconf_console.json"
        self.expected = fixture.read_bytes()
        self.data = json.loads(self.expected)
        self.rows = {
            case["name"]: case for case in [*self.data["cases"], *self.data["todo_controls"]]
        }
        self.calls = []
        self.native = self.root / "native-before-not-executed.exe"

    @staticmethod
    def encoded(value):
        """OS既定のCRLFを比較へ混ぜず、参照容器をLFで出す。

        Returns:
            UTF-8のJSON容器bytes。

        """
        return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")

    def paths(self, output, native=True):
        """native出力は要求された場合だけ所有集合へ含める。

        Returns:
            予約順に並べた出力path。

        """
        result = [output, output.with_suffix(".console-only.json")]
        if native:
            result.append(output.parent / "native_todo_before.json")
        return result

    def record(self, command, case):
        """新たな参照起動をせず、元33+2の入力と全値をstubで保つ。

        Returns:
            保存済み参照値の独立copy。

        """
        self.calls.append(command[0])
        row = self.rows[case["name"]]
        for key, value in case.items():
            self.assertEqual(value, row[key])
        return json.loads(json.dumps(row))

    def invoke(self, output, native=True, run=None):
        """公開CLI引数を保ち、実子だけを固定記録のstubへ置き換える。"""
        argv = [
            "collector",
            "--ruby",
            "reference-not-executed",
            "--output",
            str(output),
        ]
        if native:
            argv.extend(["--native-before", str(self.native)])
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(_COLLECTOR, "_run", side_effect=run or self.record),
            mock.patch("builtins.print") as completed,
        ):
            self.completed = completed
            _COLLECTOR.main()

    def test_all_values_and_LF_containers_with_optional_native_output(self):
        """再採取へ読み替えず、元の全値を2/3出力で同じく保つ。"""
        for native in (False, True):
            with self.subTest(native=native):
                output = self.root / str(native) / "new-parent" / "fixture.json"
                self.calls.clear()
                self.invoke(output, native=native)
                self.assertEqual(output.read_bytes(), self.expected)
                self.assertEqual(
                    output.with_suffix(".console-only.json").read_bytes(),
                    self.encoded(self.data["cases"]),
                )
                baseline = output.parent / "native_todo_before.json"
                if native:
                    self.assertEqual(
                        baseline.read_bytes(), self.encoded(self.data["todo_controls"])
                    )
                else:
                    self.assertFalse(baseline.exists())
                self.assertEqual(len(self.calls), 37 if native else 35)
                self.assertNotIn(b"\r", output.read_bytes())
                self.completed.assert_called_once_with(
                    "recorded 33 console cases and 2 todo controls"
                )

    def test_unrequested_native_leaf_is_not_reserved_or_overwritten(self):
        """未指定の任意出力は既存でも所有・上書きしない。"""
        output = self.root / "fixture.json"
        baseline = output.parent / "native_todo_before.json"
        baseline.write_bytes(b"other owner")
        before = baseline.stat().st_mtime_ns
        self.invoke(output, native=False)
        self.assertEqual(baseline.read_bytes(), b"other owner")
        self.assertEqual(baseline.stat().st_mtime_ns, before)

    def test_each_existing_leaf_is_rejected_before_any_work_or_child(self):
        """最後のleafに他者がいても、先の出力を作る前に拒む。"""
        for index in range(3):
            with self.subTest(index=index):
                folder = self.root / str(index)
                folder.mkdir()
                output = folder / "fixture.json"
                paths = self.paths(output)
                paths[index].write_bytes(b"other owner")
                before = paths[index].stat().st_mtime_ns
                with mock.patch.object(_COLLECTOR.tempfile, "TemporaryDirectory") as temporary:
                    with self.assertRaises(FileExistsError):
                        self.invoke(output)
                    temporary.assert_not_called()
                self.assertFalse(self.calls)
                self.assertEqual(paths[index].read_bytes(), b"other owner")
                self.assertEqual(paths[index].stat().st_mtime_ns, before)
                self.assertTrue(all(not path.exists() for path in paths if path != paths[index]))

    def test_dangling_leaf_API_stub_is_rejected_without_resolve_or_open(self):
        """leafを解決してtargetへ予約が逸れる回帰を防ぐ。"""
        real_exists, real_symlink = Path.exists, Path.is_symlink
        for index in range(3):
            with self.subTest(index=index):
                output = self.root / (str(index) + ".json")
                selected = self.paths(output)[index]
                with (
                    mock.patch.object(
                        Path,
                        "exists",
                        autospec=True,
                        side_effect=lambda p, selected=selected: (
                            False if p == selected else real_exists(p)
                        ),
                    ),
                    mock.patch.object(
                        Path,
                        "is_symlink",
                        autospec=True,
                        side_effect=lambda p, selected=selected: p == selected or real_symlink(p),
                    ),
                    mock.patch.object(Path, "resolve", side_effect=AssertionError("leaf resolve")),
                    mock.patch.object(Path, "open", side_effect=AssertionError("leaf open")),
                    mock.patch.object(_COLLECTOR.tempfile, "TemporaryDirectory") as temporary,
                ):
                    with self.assertRaises(FileExistsError):
                        self.invoke(output)
                    temporary.assert_not_called()
                self.assertFalse(self.calls)
                self.assertTrue(all(not p.exists() for p in self.paths(output)))

    def test_reservation_race_keeps_peer_and_previously_reserved_empty_leaves(self):
        """競合後は他者と自分の予約物の両方を消さない。"""
        real_open = Path.open
        for index in range(3):
            with self.subTest(index=index):
                folder = self.root / str(index)
                folder.mkdir()
                output = folder / "fixture.json"
                paths = self.paths(output)
                selected = paths[index]

                def opening(path, mode="r", *args, selected=selected, **kwargs):
                    """事前検査後の占有だけを再現し、実参照を起動しない。

                    Returns:
                        排他的に開けた場合だけ実stream。

                    """
                    if path == selected and mode == "xb":
                        with real_open(path, "wb") as stream:
                            stream.write(b"racing owner")
                    return real_open(path, mode, *args, **kwargs)

                with (
                    mock.patch.object(Path, "open", autospec=True, side_effect=opening),
                    mock.patch.object(_COLLECTOR.tempfile, "TemporaryDirectory") as temporary,
                ):
                    with self.assertRaises(FileExistsError):
                        self.invoke(output)
                    temporary.assert_not_called()
                self.assertFalse(self.calls)
                self.assertEqual(selected.read_bytes(), b"racing owner")
                self.assertTrue(all(p.read_bytes() == b"" for p in paths[:index]))
                self.assertTrue(all(not p.exists() for p in paths[index + 1 :]))

    def test_late_todo_failure_keeps_console_and_empty_reserved_final_native(self):
        """console完了後の失敗をfinalの成功へ数えない。"""
        output = self.root / "fixture.json"
        todo = {row["name"] for row in self.data["todo_controls"]}

        def failed(command, case):
            """元consoleは返し、TODOだけを失敗させる。

            Returns:
                保存済みconsole値。

            Raises:
                RuntimeError: 意図したTODOの失敗。

            """
            if case["name"] in todo:
                raise RuntimeError("mock TODO failure")
            return self.record(command, case)

        with self.assertRaisesRegex(RuntimeError, "mock TODO failure"):
            self.invoke(output, run=failed)
        self.assertEqual(output.read_bytes(), b"")
        self.assertEqual(
            output.with_suffix(".console-only.json").read_bytes(),
            self.encoded(self.data["cases"]),
        )
        self.assertEqual((output.parent / "native_todo_before.json").read_bytes(), b"")
        self.completed.assert_not_called()

    def test_failed_final_write_keeps_partial_bytes_and_other_complete_outputs(self):
        """書込途中の調査可能なbytesを、例外時にも消さない。"""
        output = self.root / "fixture.json"
        real_open = Path.open

        class Partial:
            """一部だけ実際に残るI/O失敗を再現する。"""

            def __init__(self, stream):
                """予約経路を変えず実streamを包む。"""
                self.stream = stream

            def __enter__(self):
                """ExitStackの通常close経路を使う。

                Returns:
                    失敗を注入するstreamの包み。

                """
                return self

            def __exit__(self, *error):
                """partialをflush/closeし、失敗を成功へ変えない。

                Returns:
                    実streamの終了可否。

                """
                return self.stream.__exit__(*error)

            def write(self, value):
                """一部のbytesを残した後でI/O失敗を返す。

                Raises:
                    OSError: 書込途中の意図した失敗。

                """
                self.stream.write(value[:17])
                raise OSError("mock partial write")

        def opening(path, mode="r", *args, **kwargs):
            """finalのstreamだけを失敗させ、他の出力を保つ。

            Returns:
                通常streamか、finalへの失敗注入用の包み。

            """
            stream = real_open(path, mode, *args, **kwargs)
            return Partial(stream) if path == output and mode == "xb" else stream

        with mock.patch.object(Path, "open", autospec=True, side_effect=opening):
            with self.assertRaisesRegex(OSError, "mock partial write"):
                self.invoke(output)
        self.assertEqual(output.read_bytes(), self.expected[:17])
        self.assertEqual(
            output.with_suffix(".console-only.json").read_bytes(),
            self.encoded(self.data["cases"]),
        )
        self.assertEqual(
            (output.parent / "native_todo_before.json").read_bytes(),
            self.encoded(self.data["todo_controls"]),
        )
        self.completed.assert_not_called()

    def test_aliasing_output_names_are_rejected_before_parent_creation(self):
        """同じleafを二重予約して自分自身と競合しない。"""
        output = self.root / "missing-parent" / "native_todo_before.json"
        with mock.patch.object(_COLLECTOR.tempfile, "TemporaryDirectory") as temporary:
            with self.assertRaisesRegex(ValueError, "output leaves overlap"):
                self.invoke(output)
            temporary.assert_not_called()
        self.assertFalse(self.calls)
        self.assertFalse(output.parent.exists())


if __name__ == "__main__":
    unittest.main()
