"""warning renameの有限集合・API限定投影・原本不変を独立負対照で検査する。"""

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

import record_renamed_config as recorder


class RenamedConfigContract(unittest.TestCase):
    """候補出力を使わず、不完全な本家採取を成功へ混ぜない。"""

    def test_fixed_matrix_schema_and_int_bool_types(self):
        """36CLI/4APIと3追補を分け、空・順序差・bool件数を拒む。"""
        data = recorder._template()
        recorder.validate_fixture(data)
        self.assertEqual(len(recorder.case_inputs()), 36)
        self.assertEqual(len(data["runtime_api"]), 4)
        self.assertEqual(len(recorder._order_template()[1]), 3)
        for change in ("empty", "order", "schema", "count", "api_bool"):
            broken = copy.deepcopy(data)
            if change == "empty":
                broken["cases"] = []
            elif change == "order":
                broken["cases"].reverse()
            elif change == "schema":
                broken["schema"] = True
            elif change == "count":
                broken["primary"]["all_rules_by_class"]["RuboCop::ConfigObsoletion::SplitCop"] = (
                    True
                )
            else:
                broken["runtime_api"][0]["values"]["first"]["Enabled"] = 1
            with self.assertRaises(ValueError):
                recorder.validate_fixture(broken)

    def test_complete_json_identity_paths_offenses_and_integer_summary(self):
        """空target対照以外の完了欠落や隠れたoffense・bool件数を拒む。"""
        form = recorder.case_inputs()[0]
        row = recorder._template()["cases"][0]
        report = json.loads(bytes.fromhex(row["stdout_hex"]))
        recorder._report(json.dumps(report).encode(), form)
        for field, value in [
            ("target_file_count", True),
            ("inspected_file_count", True),
            ("offense_count", True),
            ("target_file_count", 0),
        ]:
            changed = copy.deepcopy(report)
            changed["summary"][field] = value
            with self.assertRaises(ValueError):
                recorder._report(json.dumps(changed).encode(), form)
        for files in ([], report["files"] * 2, [{"path": "other.rb", "offenses": []}]):
            changed = copy.deepcopy(report)
            changed["files"] = files
            with self.assertRaises(ValueError):
                recorder._report(json.dumps(changed).encode(), form)
        changed = copy.deepcopy(report)
        changed["metadata"]["ruby_engine"] = "sonicop"
        with self.assertRaises(ValueError):
            recorder._report(json.dumps(changed).encode(), form)
        changed = copy.deepcopy(report)
        changed["files"][0]["offenses"] = []
        with self.assertRaises(ValueError):
            recorder._report(json.dumps(changed).encode(), form)
        zero = next(row for row in recorder._template()["cases"] if row["id"] == "old-false")
        zero_form = next(row for row in recorder.case_inputs() if row["id"] == "old-false")
        changed = json.loads(bytes.fromhex(zero["stdout_hex"]))
        changed["summary"]["offense_count"] = False
        with self.assertRaises(ValueError):
            recorder._report(json.dumps(changed).encode(), zero_form)

    def test_environment_and_real_cli_rows_preserve_parent(self):
        """ENV4/ambient色だけを子copyから除き、GEM/PATH/OSと親を保持する。"""
        row = recorder._template()["cases"][0]
        form = recorder.case_inputs()[0]
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
                "NO_COLOR": "ambient",
                "KEEP_SENTINEL": "keep",
            },
        ):
            before = dict(os.environ)
            with patch.object(recorder.subprocess, "run", return_value=child) as run:
                self.assertEqual(recorder.record("ruby", form), row)
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
            self.assertEqual(run.call_args.kwargs["env"], {**expected, **form["env"]})

    def test_original_bytes_mtime_and_unexpected_out_are_independent(self):
        """正しい他channelを保ち、本文単独・mtime単独・out追加を拒む。"""
        form = recorder.case_inputs()[0]
        row = recorder._template()["cases"][0]
        for kind in ("bytes", "mtime", "out"):

            def child(*args, kind=kind, **kwargs):
                root = kwargs["cwd"]
                path = root / "a.rb"
                status = path.stat()
                if kind == "bytes":
                    path.write_bytes(b"changed\n")
                    os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns))
                    self.assertEqual(path.stat().st_mtime_ns, status.st_mtime_ns)
                elif kind == "mtime":
                    os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns + 2_000_000_000))
                else:
                    (root / "report.json").write_bytes(b"unexpected")
                return SimpleNamespace(
                    returncode=row["exit"],
                    stdout=bytes.fromhex(row["stdout_hex"]),
                    stderr=bytes.fromhex(row["stderr_hex"]),
                )

            with patch.object(recorder.subprocess, "run", side_effect=child):
                with self.assertRaises(ValueError):
                    recorder.record("ruby", form)

    def test_api_projection_changes_only_the_eight_exact_fields(self):
        """全配列や任意pathを正規化せず、4APIの2箇所だけを投影する。"""
        changes = 0
        for row in recorder._template()["runtime_api"]:
            root = Path("/controlled/api")
            value = copy.deepcopy(row["values"])
            for phase in ("raw_before", "raw_after"):
                value[phase]["Naming/PredicatePrefix"]["Exclude"][0] = (
                    root.as_posix() + "/spec/**/*"
                )
            before = copy.deepcopy(value)
            projected = recorder._public_api(value, root)
            self.assertEqual(value, before)
            self.assertEqual(projected, row["values"])
            for phase in ("raw_before", "raw_after"):
                changed = copy.deepcopy(value)
                changed[phase]["Naming/PredicatePrefix"]["Exclude"][0] = "/other/spec/**/*"
                with self.assertRaises(ValueError):
                    recorder._public_api(changed, root)
                changes += 1
            value["raw_before"]["Naming/PredicatePrefix"]["Exclude"].append("/other/spec/**/*")
            self.assertEqual(
                recorder._public_api(value, root)["raw_before"]["Naming/PredicatePrefix"][
                    "Exclude"
                ][1],
                "/other/spec/**/*",
            )
        self.assertEqual(changes, 8)

    def test_api_actual_values_and_stderr_are_regenerated_without_source_mutation(self):
        """String/Classの全値を実child結果から使い、API原本/outも検査する。"""
        row = recorder._template()["runtime_api"][0]
        form = next(form for form in recorder.case_inputs() if form["id"] == row["id"])
        for mutate in (None, "out", "bytes", "mtime"):

            def child(*args, mutate=mutate, **kwargs):
                root = kwargs["cwd"]
                value = copy.deepcopy(row["values"])
                for phase in ("raw_before", "raw_after"):
                    value[phase]["Naming/PredicatePrefix"]["Exclude"][0] = (
                        root.as_posix() + "/spec/**/*"
                    )
                if mutate == "out":
                    (root / "report.json").write_bytes(b"unexpected")
                elif mutate in {"bytes", "mtime"}:
                    source = root / "config.yml"
                    status = source.stat()
                    if mutate == "bytes":
                        source.write_bytes(b"changed\n")
                        os.utime(source, ns=(status.st_atime_ns, status.st_mtime_ns))
                        self.assertEqual(source.stat().st_mtime_ns, status.st_mtime_ns)
                    else:
                        os.utime(
                            source, ns=(status.st_atime_ns, status.st_mtime_ns + 2_000_000_000)
                        )
                return SimpleNamespace(
                    returncode=0,
                    stdout=json.dumps(value).encode(),
                    stderr=bytes.fromhex(row["stderr_hex"]),
                )

            with patch.object(recorder.subprocess, "run", side_effect=child):
                if mutate:
                    with self.assertRaises(ValueError):
                        recorder.record_api("ruby", form)
                else:
                    self.assertEqual(recorder.record_api("ruby", form), row)

    def test_changed_api_inputs_are_rejected_before_child_start(self):
        """同じAPI IDでもfiles/args/envの変更を採取成功へ持ち込まない。"""
        form = next(form for form in recorder.case_inputs() if form["id"] == recorder.API_IDS[0])
        for key, value in [("files", {}), ("args", []), ("env", {})]:
            changed = copy.deepcopy(form)
            changed[key] = value
            with patch.object(recorder.subprocess, "run") as child:
                with self.assertRaises(ValueError):
                    recorder.record_api("ruby", changed)
                child.assert_not_called()

    def test_joined_warning_rows_and_out_preservation(self):
        """追補3は本家の出力reportを使い、fatal時のout保持も独立に検査する。"""
        data, forms = recorder._order_template()
        for row, form in zip(data["cases"], forms, strict=True):

            def child(*args, row=row, **kwargs):
                if not row["out_preserved"]:
                    (kwargs["cwd"] / "report.json").write_bytes(bytes.fromhex(row["out_hex"]))
                return SimpleNamespace(
                    returncode=row["exit"],
                    stdout=bytes.fromhex(row["stdout_hex"]),
                    stderr=bytes.fromhex(row["stderr_hex"]),
                )

            with patch.object(recorder.subprocess, "run", side_effect=child):
                self.assertEqual(recorder.record_order("ruby", form), row)
        row, form = data["cases"][-1], forms[-1]

        def mtime_only(*args, **kwargs):
            path = kwargs["cwd"] / "report.json"
            status = path.stat()
            os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns + 2_000_000_000))
            return SimpleNamespace(
                returncode=row["exit"],
                stdout=bytes.fromhex(row["stdout_hex"]),
                stderr=bytes.fromhex(row["stderr_hex"]),
            )

        with patch.object(recorder.subprocess, "run", side_effect=mtime_only):
            with self.assertRaises(ValueError):
                recorder.record_order("ruby", form)

    def test_fresh_alias_dangling_guards_precede_children(self):
        """既存entry・dangling・dotdot・Windows大小差を独立に拒む。"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = (root / "work", root / "output.json")
            original_lstat = os.lstat

            def invoke(destinations):
                with patch.object(
                    sys,
                    "argv",
                    [
                        "record",
                        "--ruby",
                        "ruby",
                        "--work-dir",
                        str(destinations[0]),
                        "--output",
                        str(destinations[1]),
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
            aliases = [(root / "absent" / ".." / "same", root / "same")]
            if os.name == "nt":
                aliases.append((root / "Same", root / "same"))
            for destinations in aliases:
                self.assertTrue(all(not os.path.lexists(path) for path in destinations))
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaises(ValueError):
                        invoke(destinations)
                    child.assert_not_called()


if __name__ == "__main__":
    unittest.main()
