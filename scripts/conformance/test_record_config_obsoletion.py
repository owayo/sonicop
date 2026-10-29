"""fatal49/初期22と、未実装カテゴリを混同させない記録契約。"""

import copy
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_config_obsoletion as recorder


def fixture():
    """実本家の記録だけを単体対照として渡す。

    Returns:
        dict: 22条件と51API表の記録。

    """
    return json.loads(
        (Path(__file__).resolve().parents[2] / "tests/fixtures/config_obsoletion.json").read_text(
            encoding="utf-8"
        )
    )


class ObsoletionRecorderContract(unittest.TestCase):
    """本家由来の表と実CLI数を独立に検査する。"""

    def test_table_preserves_severity_and_real_order(self):
        """warning2をfatalへ格上げせず、重複や欠落も拒否する。"""
        primary = fixture()["primary"]
        rows = recorder.renamed_data(primary)
        self.assertEqual(len(rows), 51)
        self.assertEqual(sum(row["severity"] == "error" for row in rows), 49)
        self.assertEqual(
            [row["old_name"] for row in rows], [row["old_name"] for row in primary["rules"]]
        )
        for broken in (primary["rules"][:-1], primary["rules"][:-1] + [primary["rules"][0]]):
            with self.assertRaises(ValueError):
                recorder.renamed_data({**primary, "rules": broken})

    def test_matrix_is_finite_and_excludes_other_rule_categories(self):
        """残件のAPI個数を実CLI成功件数へ加算しない。"""
        data = fixture()
        built = recorder._fixture(data["primary"], data["cases"])
        self.assertEqual(built["coverage"]["actual_cli_cases"], 22)
        self.assertFalse(built["coverage"]["warning_merge_implemented"])
        for broken in (data["cases"][:-1], list(reversed(data["cases"]))):
            with self.assertRaises(ValueError):
                recorder._fixture(data["primary"], broken)

    def test_normal_json_must_be_the_real_complete_report(self):
        """候補identityや未検査の空JSONを正常対照として取り込まない。"""
        case = next(row for row in fixture()["cases"] if row["id"] == "canonical-negative")
        report = json.loads(bytes.fromhex(case["stdout_hex"]))
        recorder._report(json.dumps(report))
        for mutation in ("identity", "count", "path", "offenses"):
            broken = copy.deepcopy(report)
            if mutation == "identity":
                broken["metadata"]["ruby_engine"] = "sonicop"
            elif mutation == "count":
                broken["summary"]["inspected_file_count"] = 0
            elif mutation == "offenses":
                broken["files"][0]["offenses"] = [{"message": "unexpected"}]
            else:
                broken["files"][0]["path"] = "wrong.rb"
            with self.assertRaises(ValueError):
                recorder._report(json.dumps(broken))

        for key, value in [
            ("inspected_file_count", True),
            ("target_file_count", True),
            ("offense_count", False),
        ]:
            broken = copy.deepcopy(report)
            broken["summary"][key] = value
            with self.assertRaises(ValueError):
                recorder._report(json.dumps(broken))

    def test_record_uses_env4_copy_without_losing_os_or_gem_paths(self):
        """親を変えず、実際のsubprocess呼出しへ同じ除去契約を渡す。"""
        row = fixture()["cases"][0]
        keys = ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
        environment = {
            **dict.fromkeys(keys, "poison"),
            "GEM_PATH": "gems",
            "PATH": "tools",
            "SYSTEMROOT": "os",
        }
        calls = []

        def run(_command, **kwargs):
            calls.append(kwargs["env"])
            self.assertEqual(
                kwargs["env"],
                {"GEM_PATH": "gems", "PATH": "tools", "SYSTEMROOT": "os", "CLICOLOR_FORCE": "0"},
            )
            return subprocess.CompletedProcess(
                [], row["exit"], bytes.fromhex(row["stdout_hex"]), bytes.fromhex(row["stderr_hex"])
            )

        with (
            tempfile.TemporaryDirectory() as directory,
            patch.dict(os.environ, environment, clear=True),
        ):
            before = dict(os.environ)
            with patch.object(recorder.subprocess, "run", side_effect=run):
                actual = recorder._record(
                    0, recorder.case_inputs()[0], "ruby", Path("reference.rb"), Path(directory)
                )
            self.assertEqual(actual, {**row, "env": {"CLICOLOR_FORCE": "0"}})
            self.assertEqual(dict(os.environ), before)
        self.assertEqual(len(calls), 1)

    def test_failure_and_source_mutation_fail_closed(self):
        """正しいchannelと復元したmtimeで隠した本文差も、完了記録へ取り込まない。"""
        row = fixture()["cases"][0]

        def run(_command, **kwargs):
            source = kwargs["cwd"] / "a.rb"
            status = source.stat()
            source.write_bytes(b"changed\n")
            os.utime(source, ns=(status.st_atime_ns, status.st_mtime_ns))
            self.assertEqual(source.stat().st_mtime_ns, status.st_mtime_ns)
            return subprocess.CompletedProcess(
                [], row["exit"], bytes.fromhex(row["stdout_hex"]), bytes.fromhex(row["stderr_hex"])
            )

        with (
            tempfile.TemporaryDirectory() as directory,
            patch.object(recorder.subprocess, "run", side_effect=run),
        ):
            with self.assertRaises(ValueError):
                recorder._record(
                    0, recorder.case_inputs()[0], "ruby", Path("reference.rb"), Path(directory)
                )

    def test_input_identity_and_order_fixture_are_preserved(self):
        """先行22と追補4の入力・出典を分け、欠落や選択の誤りも拒否する。"""
        for order, name, ids in [
            (False, "config_obsoletion", recorder.CASE_IDS),
            (True, "config_obsoletion_order", recorder.ORDER_IDS),
        ]:
            data = json.loads(
                (Path(__file__).parents[2] / f"tests/fixtures/{name}.json").read_bytes()
            )
            self.assertEqual(data["schema"], 1)
            forms = recorder._selected_inputs(order=order)
            self.assertEqual(tuple(form["id"] for form in forms), ids)
            for form, row in zip(forms, data["cases"], strict=True):
                self.assertEqual(form, {key: row[key] for key in form})
            for only in ([], ["unknown"], [ids[0], ids[0]]):
                with self.assertRaises(ValueError):
                    recorder._selected_inputs(only, order)
        forms = recorder.case_inputs()
        for bad in ([], forms[:-1], forms[:-1] + [forms[0]], list(reversed(forms))):
            with patch.object(recorder, "case_inputs", return_value=bad):
                with self.assertRaises(ValueError):
                    recorder._selected_inputs()

    def test_primary_runtime_source_and_table_guard_fail_closed(self):
        """候補identity・不完全なsource証拠・別カテゴリを実Oracleへ混ぜない。"""
        primary = fixture()["primary"]
        for kind in (
            "valid",
            "ruby",
            "gems",
            "source",
            "category",
            "category-bool",
            "severity",
            "message",
        ):
            value = copy.deepcopy(primary)
            if kind == "ruby":
                value["ruby"] = "0.0"
            elif kind == "gems":
                value["gems"]["rubocop"] = "0.0"
            elif kind == "source":
                value["sources"].pop("LICENSE.txt")
            elif kind == "category":
                value["all_rules_by_class"]["RuboCop::ConfigObsoletion::RemovedCop"] = 0
            elif kind == "category-bool":
                value["all_rules_by_class"]["RuboCop::ConfigObsoletion::SplitCop"] = True
            elif kind == "severity":
                value["rules"][0]["warning"] = True
            elif kind == "message":
                value["rules"][0]["message"] = "invented"
            done = subprocess.CompletedProcess([], 0, json.dumps(value).encode(), b"")
            with patch.object(recorder.subprocess, "run", return_value=done):
                if kind == "valid":
                    self.assertEqual(recorder._primary("ruby", Path(".")), primary)
                else:
                    with self.assertRaises(ValueError):
                        recorder._primary("ruby", Path("."))

    def test_all_output_entries_are_fresh_before_the_child_starts(self):
        """workと二つのoutはdangling symlinkでも上書きせず、同じpathも拒否する。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            destinations = [root / "work", root / "fixture.json", root / "rules.json"]
            for target in destinations:
                original_lstat = os.lstat

                def _lstat(path, *args, **kwargs):
                    if Path(path) == target:
                        return SimpleNamespace(st_mode=stat.S_IFLNK)
                    return original_lstat(path, *args, **kwargs)

                with patch("os.lstat", side_effect=_lstat):
                    with self.assertRaises(ValueError):
                        recorder._fresh_paths(*destinations)
                target.write_bytes(b"keep")
                with (
                    patch.object(
                        sys,
                        "argv",
                        [
                            "recorder",
                            "--ruby",
                            "ruby",
                            "--work-dir",
                            str(destinations[0]),
                            "--output",
                            str(destinations[1]),
                            "--rules-output",
                            str(destinations[2]),
                        ],
                    ),
                    patch.object(recorder.subprocess, "run") as run,
                ):
                    with self.assertRaises(ValueError):
                        recorder.main()
                    run.assert_not_called()
                self.assertEqual(target.read_bytes(), b"keep")
                target.unlink()
            with self.assertRaises(ValueError):
                recorder._fresh_paths(destinations[0], destinations[0], destinations[2])

    def test_exit_artifact_and_mtime_mutations_are_not_complete_records(self):
        """本文が同じmtime変更・out先行作成・任意の失敗も拒否する。"""
        form = recorder.case_inputs()[0]
        row = fixture()["cases"][0]
        for kind in ("exit", "stdout", "stderr", "out", "mtime"):

            def run(_command, **kwargs):
                cwd = kwargs["cwd"]
                if kind == "out":
                    (cwd / "report.json").write_bytes(b"unexpected")
                if kind == "mtime":
                    path = cwd / "a.rb"
                    status = path.stat()
                    os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns + 1_000_000_000))
                return subprocess.CompletedProcess(
                    [],
                    0 if kind == "exit" else 2,
                    b"unexpected" if kind == "stdout" else b"",
                    b"arbitrary" if kind == "stderr" else bytes.fromhex(row["stderr_hex"]),
                )

            with (
                tempfile.TemporaryDirectory() as directory,
                patch.object(recorder.subprocess, "run", side_effect=run),
            ):
                with self.assertRaises(ValueError):
                    recorder._record(0, form, "ruby", Path("reference.rb"), Path(directory))


if __name__ == "__main__":
    unittest.main()
