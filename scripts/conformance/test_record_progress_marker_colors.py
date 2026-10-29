"""Progress採取器の全raw・完了・物理bytes/属性・保存先を独立に保護する。"""

import copy
import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_progress_marker_colors as recorder

_REAL_RUN = subprocess.run


def _dangling_entry(path, target):
    root = path.parent.resolve()
    if not path.resolve(strict=False).is_relative_to(root) or not target.resolve(
        strict=False
    ).is_relative_to(root.parent):
        raise ValueError("専用tempdir外へlinkを作成できません")
    try:
        path.symlink_to(target, target_is_directory=True)
    except OSError:
        if os.name != "nt" or any(c in str(path) + str(target) for c in '%"\r\n'):
            raise
        done = _REAL_RUN(
            f'cmd.exe /d /v:off /c mklink /J "{path}" "{target}"',
            capture_output=True,
            check=False,
        )
        if done.returncode:
            raise ValueError("real dangling junctionを作成できません")
    if not os.path.lexists(path) or path.exists():
        raise ValueError("dangling entryの対照が成立していません")


def _remove_entry(path):
    if path.is_symlink():
        path.unlink()
    else:
        path.rmdir()


def _answer(case, index, root):
    run = case["runs"][index]
    for name, raw in run["after_hex"].items():
        path = root / name
        expected = bytes.fromhex(raw)
        if path.read_bytes() != expected:
            path.write_bytes(expected)
    for name, raw in run["artifacts_hex"].items():
        (root / name).write_bytes(bytes.fromhex(raw))
    return SimpleNamespace(
        returncode=run["exit"],
        stdout=bytes.fromhex(run["stdout_hex"]),
        stderr=bytes.fromhex(run["stderr_hex"]),
    )


def _fixture():
    return recorder._template()


class ProgressMarkerContract(unittest.TestCase):
    """仕様版の全streamを保ち、自己整合性だけの偽成功を拒否する。"""

    def test_fixed_schema_inputs_runtime_and_nineteen_runs(self):
        """空・欠落・重複・順序・bool・出典・入力・run数の変更を拒否する。"""
        data = _fixture()
        recorder.validate_fixture(data)
        self.assertEqual(tuple(row["id"] for row in data["cases"]), recorder.IDS)
        self.assertEqual(sum(len(row["runs"]) for row in data["cases"]), 19)
        for kind in (
            "empty",
            "duplicate",
            "order",
            "schema",
            "scope",
            "source",
            "input",
            "run",
            "count",
            "exit",
        ):
            broken = copy.deepcopy(data)
            if kind == "empty":
                broken["cases"] = []
            elif kind == "duplicate":
                broken["cases"][-1] = broken["cases"][0]
            elif kind == "order":
                broken["cases"].reverse()
            elif kind == "schema":
                broken["schema"] = True
            elif kind == "scope":
                broken["cases"][0]["coverage"] = "limited"
            elif kind == "source":
                broken["runtime"]["source_sha256"].popitem()
            elif kind == "input":
                broken["cases"][0]["args"].append("other.rb")
            elif kind == "run":
                next(row for row in broken["cases"] if row["id"] == "cache")["runs"].pop()
            elif kind == "count":
                broken["cases"][0]["run_count"] = True
            else:
                broken["cases"][0]["runs"][0]["exit"] = False
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                recorder.validate_fixture(broken)

    def test_missing_color_wrong_max_and_zero_completion_are_not_evidence(self):
        """marker本体と付属fieldを一緒に改変しても、固定実参照との違いを拒む。"""
        for kind in ("color", "max", "completion", "stream"):
            broken = _fixture()
            case = next(row for row in broken["cases"] if row["id"] == "mixed-corrected-high")
            run = case["runs"][0]
            raw = bytes.fromhex(run["stdout_hex"])
            marker = bytes.fromhex(run["marks_hex"])
            if kind == "color":
                plain = recorder.ANSI.sub(b"", marker)
                run["stdout_hex"] = raw.replace(marker, plain, 1).hex()
                run["marks_hex"] = plain.hex()
            elif kind == "max":
                wrong = b"\x1b[35mW\x1b[0m"
                run["stdout_hex"] = raw.replace(marker, wrong, 1).hex()
                run["marks_hex"] = wrong.hex()
            elif kind == "completion":
                run["stdout_hex"] = raw.replace(b"1 file inspected", b"0 files inspected").hex()
            else:
                run["stdout_hex"], run["stderr_hex"] = run["stderr_hex"], run["stdout_hex"]
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                recorder.validate_fixture(broken)

    def test_child_environment_is_independent_and_cache_is_explicit(self):
        """ENV4と色だけを制御し、Gem/OS探索と親環境を保持する。"""
        with patch.dict(
            os.environ,
            {
                "RUBYOPT": "bad",
                "RUBYLIB": "bad",
                "RUBOCOP_OPTS": "bad",
                "RUBOCOP_TARGET_RUBY_VERSION": "bad",
                "NO_COLOR": "yes",
                "GEM_HOME": "keep",
                "KEEP_SENTINEL": "retained",
            },
        ):
            before = dict(os.environ)
            intentional = {"CLICOLOR_FORCE": "1", "XDG_CACHE_HOME": "isolated"}
            child = recorder.child_environment(intentional)
            self.assertEqual(dict(os.environ), before)
            expected = {
                key: value
                for key, value in before.items()
                if (key.upper() if os.name == "nt" else key)
                not in {
                    "RUBYOPT",
                    "RUBYLIB",
                    "RUBOCOP_OPTS",
                    "RUBOCOP_TARGET_RUBY_VERSION",
                    "NO_COLOR",
                }
            }
            expected.update(TERM="dumb", **intentional)
            self.assertEqual(child, expected)

    def test_all_eighteen_positive_cases_bind_nineteen_private_runs(self):
        """正しい補正bytesとFile出力を伴う実raw対照で、19応答と属性を束縛する。"""
        calls = 0
        with tempfile.TemporaryDirectory() as temporary:
            for case in _fixture()["cases"]:
                root = Path(temporary) / case["id"]
                index = 0

                def child(*args, **kwargs):
                    nonlocal index, calls
                    answer = _answer(case, index, kwargs["cwd"])
                    index += 1
                    calls += 1
                    return answer

                with patch.object(recorder.subprocess, "run", side_effect=child):
                    self.assertEqual(
                        recorder.record("ruby", case, root, Path("reference.rb")), case
                    )
                for index in range(case["run_count"]):
                    private = json.loads((root / f"private-run{index}.json").read_bytes())
                    self.assertEqual(private["stdout_hex"], case["runs"][index]["stdout_hex"])
                    self.assertEqual(private["exit"], case["runs"][index]["exit"])
                    for name in case["files"]:
                        self.assertEqual(
                            private["inputs_after"][name]["bytes_hex"],
                            case["runs"][index]["after_hex"][name],
                        )
                        self.assertEqual(
                            private["inputs_before"][name]["mode"],
                            private["inputs_after"][name]["mode"],
                        )
        self.assertEqual(calls, 19)

    def test_unknown_changed_case_and_occupied_root_precede_child(self):
        """未知条件と同IDの変更、既存case rootを子起動前に拒否する。"""
        with tempfile.TemporaryDirectory() as temporary:
            for kind in ("unknown", "args", "occupied"):
                case = copy.deepcopy(_fixture()["cases"][0])
                root = Path(temporary) / kind
                if kind == "unknown":
                    case["id"] = "other"
                elif kind == "args":
                    case["args"].append("other.rb")
                else:
                    root.mkdir()
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.record("ruby", case, root, Path("reference.rb"))
                child.assert_not_called()

    def test_source_bytes_mtime_mode_and_artifact_each_fail_independently(self):
        """正しいstreamのまま一つの保持条件だけを破り、他guardで隠さない。"""
        with tempfile.TemporaryDirectory() as temporary:
            for kind in ("bytes", "mtime", "mode", "artifact"):
                case = _fixture()["cases"][0]
                root = Path(temporary) / kind

                def child(*args, kind=kind, **kwargs):
                    result = _answer(case, 0, root)
                    path = root / "good.rb"
                    before = path.stat()
                    if kind == "bytes":
                        path.write_bytes(b"y=1\n")
                        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
                    elif kind == "mtime":
                        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns + 2_000_000_000))
                    elif kind == "mode":
                        path.chmod(stat.S_IMODE(before.st_mode) & ~0o222)
                    else:
                        (root / "progress.txt").write_bytes(b"unexpected")
                    return result

                try:
                    with (
                        patch.object(recorder.subprocess, "run", side_effect=child),
                        self.assertRaises(ValueError),
                    ):
                        recorder.record("ruby", case, root, Path("reference.rb"))
                finally:
                    (root / "good.rb").chmod(0o666)

    def test_corrected_physical_bytes_and_separate_artifact_are_exact(self):
        """補正CRLFをformatter改行へ流用せず、既存artifactの本文差も拒む。"""
        with tempfile.TemporaryDirectory() as temporary:
            for kind in ("corrected-C", "separate-simple"):
                case = next(row for row in _fixture()["cases"] if row["id"] == kind)
                root = Path(temporary) / kind

                def child(*args, **kwargs):
                    result = _answer(case, 0, root)
                    path = root / ("good.rb" if kind == "corrected-C" else "progress.txt")
                    path.write_bytes(path.read_bytes().replace(b"\r\n", b"\n"))
                    return result

                with (
                    patch.object(recorder.subprocess, "run", side_effect=child),
                    self.assertRaises(ValueError),
                ):
                    recorder.record("ruby", case, root, Path("reference.rb"))

    def test_fresh_alias_ancestor_existing_and_real_dangling_guard(self):
        """既存entryと未作成aliasを別々に検査し、両方向の親子も事前拒否する。"""
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            for index in range(2):
                paths = [base / "work", base / "output"]
                paths[index].write_bytes(b"KEEP")
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", *paths)
                child.assert_not_called()
                self.assertEqual(paths[index].read_bytes(), b"KEEP")
                paths[index].unlink()
            aliases = [
                (base / "same", base / "same"),
                (base / "parent/../same", base / "same"),
                (base / "parent/work", base / "parent"),
                (base / "parent", base / "parent/output"),
            ]
            if os.name == "nt":
                aliases.append((base / "CASE", base / "case"))
            for work, output in aliases:
                self.assertFalse(os.path.lexists(work))
                self.assertFalse(os.path.lexists(output))
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", work, output)
                child.assert_not_called()
            entry = base / "output"
            _dangling_entry(entry, base / "missing")
            try:
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", base / "work", entry)
                child.assert_not_called()
            finally:
                _remove_entry(entry)

    def test_real_new_dangling_artifact_is_not_absence(self):
        """lexists=True/exists=Falseを無出力成功にせず、targetを読まない。"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "case"
            case = _fixture()["cases"][0]

            def child(*args, **kwargs):
                result = _answer(case, 0, root)
                _dangling_entry(root / "progress.txt", root / "missing")
                return result

            try:
                with (
                    patch.object(recorder.subprocess, "run", side_effect=child),
                    self.assertRaises(ValueError),
                ):
                    recorder.record("ruby", case, root, Path("reference.rb"))
            finally:
                _remove_entry(root / "progress.txt")

    def test_runtime_failure_partial_collection_and_exclusive_output_fail_closed(self):
        """実版/出典エラーと途中失敗では公開せず、既存保存先も上書きしない。"""
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            for index, kind in enumerate(("exit", "stderr", "source", "partial")):
                runtime = copy.deepcopy(recorder.RUNTIME)
                if kind == "source":
                    runtime["source_sha256"].popitem()
                done = SimpleNamespace(
                    returncode=1 if kind == "exit" else 0,
                    stdout=b"" if kind == "exit" else json.dumps(runtime).encode(),
                    stderr=b"error" if kind == "stderr" else b"",
                )
                output = base / f"output{index}"
                with (
                    patch.object(recorder.subprocess, "run", return_value=done),
                    patch.object(
                        recorder, "record", side_effect=ValueError("incomplete")
                    ) as record,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", base / f"work{index}", output)
                if kind != "partial":
                    record.assert_not_called()
                self.assertFalse(os.path.lexists(output))
            output = base / "existing"
            output.write_bytes(b"KEEP")
            with self.assertRaises(FileExistsError):
                recorder._write_json(output, {})
            self.assertEqual(output.read_bytes(), b"KEEP")


if __name__ == "__main__":
    unittest.main()
