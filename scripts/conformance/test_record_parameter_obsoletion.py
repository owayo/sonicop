"""廃止parameter採取器の有限集合・型・完了・原本保存を負対照で検査する。"""

import copy
import json
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_parameter_obsoletion as recorder


def fixture():
    """候補を呼ばず、固定版の実API/CLI保存値を対照にする。

    Returns:
        dict: 元41条件とAPI60/420probe。

    """
    return json.loads(
        (
            Path(__file__).resolve().parents[2] / "tests/fixtures/parameter_obsoletion.json"
        ).read_bytes()
    )


class ParameterObsoletionTest(unittest.TestCase):
    """偽の完了・bool/int混同・原本変更を採取成功にしない。"""

    def test_primary_requires_all_rules_order_identity_types_and_420_probes(self):
        """60順と30/30を型付きで検査し、空APIや最低版probe欠落を拒む。"""
        primary = fixture()["primary"]
        data = recorder.parameter_data(primary)
        self.assertEqual(len(data), 60)
        self.assertEqual(sum(row["severity"] == "warning" for row in data), 30)
        for key, value in [("rules", []), ("ruby", "wrong"), ("gems", {}), ("sources", {})]:
            changed = copy.deepcopy(primary)
            changed[key] = value
            with self.assertRaises(ValueError):
                recorder.parameter_data(changed)
        for key, value in [
            ("index", 67.0),
            ("warning", 0),
            ("cop", "Wrong/Cop"),
            ("probes", []),
            ("metadata", {}),
            ("message", "wrong"),
        ]:
            changed = copy.deepcopy(primary)
            changed["rules"][0][key] = value
            with self.assertRaises(ValueError):
                recorder.parameter_data(changed)
        for key, value in [
            ("present", 0),
            ("violated", 1),
            ("target", True),
            ("id", "unknown"),
            ("value", False),
        ]:
            changed = copy.deepcopy(primary)
            changed["rules"][0]["probes"][0][key] = value
            with self.assertRaises(ValueError):
                recorder.parameter_data(changed)
        changed = copy.deepcopy(primary)
        changed["rules"][52]["metadata"]["minimum_ruby_version"] = True
        with self.assertRaises(ValueError):
            recorder.parameter_data(changed)

    def test_fixed_41_inputs_schema_coverage_and_selection(self):
        """元rawを保つ41strictを空・重複・並べ替えで置換しない。"""
        data = fixture()
        self.assertEqual(recorder._fixture(data["primary"], data["cases"]), data)
        forms = recorder.case_inputs()
        self.assertEqual(len(forms), 41)
        self.assertEqual(sum(form["coverage"] == "strict" for form in forms), 41)
        for only in ([], ["unknown"], ["warning-style", "warning-style"]):
            with self.assertRaises(ValueError):
                recorder._selected_inputs(only)
        for rows in (
            [],
            data["cases"][:-1],
            list(reversed(data["cases"])),
            data["cases"][:-1] + [data["cases"][0]],
        ):
            with self.assertRaises(ValueError):
                recorder._fixture(data["primary"], rows)
        with patch.object(recorder, "case_inputs", return_value=forms[:-1]):
            with self.assertRaises(ValueError):
                recorder._selected_inputs()

    def test_report_requires_complete_path_all_offenses_and_integer_summary(self):
        """正常JSONは全offenseと1pathの正の完了件数を要求する。"""
        row = next(row for row in fixture()["cases"] if row["id"] == "warning-style")
        valid = json.loads(bytes.fromhex(row["stdout_hex"]))
        recorder.completed_report(json.dumps(valid).encode())
        for files in (
            [],
            valid["files"] * 2,
            [{"path": "wrong.rb", "offenses": []}],
            [{"path": "a.rb", "offenses": [{"message": "hidden"}]}],
        ):
            changed = copy.deepcopy(valid)
            changed["files"] = files
            with self.assertRaises(ValueError):
                recorder.completed_report(json.dumps(changed).encode())
        for key, value in [
            ("target_file_count", 0),
            ("target_file_count", True),
            ("inspected_file_count", True),
            ("offense_count", False),
            ("offense_count", 1),
        ]:
            changed = copy.deepcopy(valid)
            changed["summary"][key] = value
            with self.assertRaises(ValueError):
                recorder.completed_report(json.dumps(changed).encode())
        for key, value in [
            ("ruby_engine", "sonicop"),
            ("rubocop_version", "wrong"),
            ("ruby_version", "wrong"),
        ]:
            changed = copy.deepcopy(valid)
            changed["metadata"][key] = value
            with self.assertRaises(ValueError):
                recorder.completed_report(json.dumps(changed).encode())

    def test_env_child_copy_preserves_parent_and_intentional_version(self):
        """共用ENV4除去後にcase指定だけ戻し、GEM/PATH/OSと親を保つ。"""
        case = next(
            case for case in recorder.case_inputs() if case["id"] == "minimum-environment32"
        )
        row = next(row for row in fixture()["cases"] if row["id"] == case["id"])
        child = SimpleNamespace(
            returncode=row["exit"],
            stdout=bytes.fromhex(row["stdout_hex"]),
            stderr=bytes.fromhex(row["stderr_hex"]),
        )
        with patch.dict(
            os.environ,
            {
                "rubyopt": "bad",
                "RuByLiB": "bad",
                "RUBOCOP_OPTS": "bad",
                "RUBOCOP_TARGET_RUBY_VERSION": "bad",
                "KEEP_SENTINEL": "keep",
            },
        ):
            before = dict(os.environ)
            with patch.object(recorder.subprocess, "run", return_value=child) as run:
                self.assertEqual(recorder.record("ruby", case), row)
            self.assertEqual(dict(os.environ), before)
            expected = {
                key: value
                for key, value in before.items()
                if (key.upper() if os.name == "nt" else key)
                not in {"RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"}
            }
            self.assertEqual(run.call_args.kwargs["env"], {**expected, **case["env"]})

    def test_wrong_exit_stdout_out_and_source_changes_fail_independently(self):
        """他の出力を正しく保ったbytes単独/mtime単独/想定外outも拒む。"""
        row = fixture()["cases"][0]
        case = recorder.case_inputs()[0]
        for code, stdout, stderr in [
            (0, b"", b""),
            (2, b"unexpected", bytes.fromhex(row["stderr_hex"])),
            (2, b"", b"other failure"),
        ]:
            with patch.object(
                recorder.subprocess,
                "run",
                return_value=SimpleNamespace(returncode=code, stdout=stdout, stderr=stderr),
            ):
                with self.assertRaises(ValueError):
                    recorder.record("ruby", case)
        for kind in ("bytes", "mtime", "out"):

            def mutation(*args, kind=kind, **kwargs):
                root = kwargs["cwd"]
                source = root / "a.rb"
                status = source.stat()
                if kind == "bytes":
                    source.write_bytes(b"y=2\n")
                    os.utime(source, ns=(status.st_atime_ns, status.st_mtime_ns))
                    self.assertEqual(source.stat().st_mtime_ns, status.st_mtime_ns)
                elif kind == "mtime":
                    os.utime(source, ns=(status.st_atime_ns, status.st_mtime_ns + 2_000_000_000))
                else:
                    (root / "report.json").write_bytes(b"unexpected")
                return SimpleNamespace(
                    returncode=row["exit"],
                    stdout=bytes.fromhex(row["stdout_hex"]),
                    stderr=bytes.fromhex(row["stderr_hex"]),
                )

            with patch.object(recorder.subprocess, "run", side_effect=mutation):
                with self.assertRaises(ValueError):
                    recorder.record("ruby", case)

    def test_out_sentinel_is_included_in_original_bytes_and_mtime_guard(self):
        """初期fatalが触らないはずの既存outも原本と同じ強さで検査する。"""
        row = next(row for row in fixture()["cases"] if row["id"] == "out-fatal-preserved")
        case = next(case for case in recorder.case_inputs() if case["id"] == row["id"])

        def mutation(*args, **kwargs):
            output = kwargs["cwd"] / "report.json"
            status = output.stat()
            output.write_bytes(b"CHANGED!!\n")
            os.utime(output, ns=(status.st_atime_ns, status.st_mtime_ns))
            return SimpleNamespace(
                returncode=row["exit"],
                stdout=bytes.fromhex(row["stdout_hex"]),
                stderr=bytes.fromhex(row["stderr_hex"]),
            )

        with patch.object(recorder.subprocess, "run", side_effect=mutation):
            with self.assertRaises(ValueError):
                recorder.record("ruby", case)

    def test_all_destinations_and_aliases_are_rejected_before_child_start(self):
        """fresh3保存先を既存entry・dangling・dotdot・Windows大小差から守る。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            paths = [root / "work", root / "out.json", root / "rules.json"]
            original_lstat = os.lstat

            def invoke(destinations, selection=()):
                work, output, rules = destinations
                with patch.object(
                    sys,
                    "argv",
                    [
                        "record",
                        "--ruby",
                        "ruby",
                        "--work-dir",
                        str(work),
                        "--output",
                        str(output),
                        "--rules-output",
                        str(rules),
                        *selection,
                    ],
                ):
                    recorder.main()

            for target in paths:

                def _lstat(path, *args, target=target, **kwargs):
                    if Path(path) == target:
                        return SimpleNamespace(st_mode=stat.S_IFLNK)
                    return original_lstat(path, *args, **kwargs)

                with patch("os.lstat", side_effect=_lstat):
                    with self.assertRaises(ValueError):
                        recorder._fresh_paths(*paths)
                target.write_bytes(b"KEEP")
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaises(ValueError):
                        invoke(paths)
                    child.assert_not_called()
                self.assertEqual(target.read_bytes(), b"KEEP")
                target.unlink()
            aliases = [
                (root / "absent" / ".." / "same", root / "same", paths[2]),
                (paths[0], paths[1], paths[1]),
            ]
            if os.name == "nt":
                aliases.append((root / "Same", root / "same", paths[2]))
            for destinations in aliases:
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaises(ValueError):
                        invoke(destinations)
                    child.assert_not_called()
            for selection in (
                ["--case"],
                ["--case", "unknown"],
                ["--case", "warning-style", "warning-style"],
            ):
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaises(ValueError):
                        invoke(paths, selection)
                    child.assert_not_called()


if __name__ == "__main__":
    unittest.main()
