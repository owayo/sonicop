"""検査しない偽 CLI や途中で欠けた記録を、本家の期待値として採用しない。"""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import record_windows_corrections as recorder


class WindowsCorrectionRecorderTest(unittest.TestCase):
    def test_child_env_removes_only_oracle_injections_and_preserves_parent(self):
        poison = {
            key: "poison"
            for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
        }
        kept = {key: f"keep-{key}" for key in ("GEM_HOME", "GEM_PATH", "PATH", "SYSTEMROOT")}
        with patch.dict(os.environ, poison | kept):
            before = os.environ.copy()
            child = recorder.oracle_child_env()
            self.assertTrue(all(key not in child for key in poison))
            self.assertTrue(all(child[key] == value for key, value in kept.items()))
            child["PATH"] = "changed-child-only"
            self.assertEqual(os.environ.copy(), before)

    def test_inspection_and_value_children_receive_the_isolated_copy(self):
        poison = {
            key: "poison"
            for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
        }
        with patch.dict(os.environ, poison):
            before = os.environ.copy()
            completed = subprocess.CompletedProcess([], 0, json.dumps(self.report()).encode(), b"")
            with patch.object(recorder.subprocess, "run", return_value=completed) as run:
                recorder.execute(["reference"], ".", ["input.rb"], ["input.rb"])
                self.assertEqual(run.call_args.kwargs["env"], recorder.oracle_child_env())
            with (
                tempfile.TemporaryDirectory() as directory,
                patch.object(recorder.subprocess, "run", return_value=completed) as run,
            ):
                recorder.runtime("ruby", Path(directory), b"p 1\n")
                self.assertEqual(run.call_args.kwargs["env"], recorder.oracle_child_env())
            self.assertEqual(os.environ.copy(), before)

    def report(self, paths=("input.rb",), offenses=0):
        files = [{"path": path, "offenses": []} for path in paths]
        if offenses:
            files[0]["offenses"] = [{"cop_name": "Layout/SpaceAroundOperators"}]
        return {
            "metadata": {"rubocop_version": "1.89.0"},
            "files": files,
            "summary": {
                "target_file_count": len(paths),
                "inspected_file_count": len(paths),
                "offense_count": offenses,
            },
        }

    def invoke(self, report, expected=("input.rb",), exit_code=0, stderr=b""):
        completed = subprocess.CompletedProcess([], exit_code, json.dumps(report).encode(), stderr)
        with patch.object(recorder.subprocess, "run", return_value=completed):
            return recorder.execute(["reference"], ".", ["input.rb"], expected)

    def test_accepts_complete_success_and_ordinary_offense_exit(self):
        for paths, count, exit_code in [
            (("input.rb",), 0, 0),
            (("input.rb",), 1, 1),
            (("first.rb", "second.rb"), 1, 0),
        ]:
            with self.subTest(paths=paths, exit=exit_code):
                result = self.invoke(self.report(paths, count), paths, exit_code)
                self.assertEqual(result["exit"], exit_code)
                self.assertNotIn("metadata", result["report"])

    def test_rejects_zero_targets_and_incomplete_inspections(self):
        rows = [self.report(())]
        incomplete = self.report(("input.rb", "second.rb"))
        incomplete["summary"]["inspected_file_count"] = 1
        rows.append(incomplete)
        for row in rows:
            with self.subTest(report=row), self.assertRaises(ValueError):
                self.invoke(row, ("input.rb", "second.rb"))

    def test_rejects_duplicate_unexpected_and_missing_paths(self):
        for paths in [("input.rb", "input.rb"), ("input.rb", "other.rb"), ("input.rb",)]:
            with self.subTest(paths=paths), self.assertRaises(ValueError):
                self.invoke(self.report(paths), ("input.rb", "second.rb"))

    def test_rejects_inconsistent_or_nonnumeric_summaries(self):
        for key, value in [
            ("offense_count", 0),
            ("offense_count", True),
            ("inspected_file_count", True),
            ("target_file_count", "1"),
        ]:
            row = self.report(offenses=1)
            row["summary"][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.invoke(row)

    def test_rejects_abnormal_exits_stderr_and_malformed_reports(self):
        for exit_code in [-1, 2, 9]:
            with self.subTest(exit=exit_code), self.assertRaises(ValueError):
                self.invoke(self.report(), exit_code=exit_code)
        for report in [
            [],
            {},
            {"files": [None], "summary": {}},
            {"files": [{"path": None}], "summary": {}},
        ]:
            with self.subTest(report=report), self.assertRaises(ValueError):
                self.invoke(report)
        with self.assertRaises(ValueError):
            self.invoke(self.report(), stderr=b"internal error")

    def test_rejects_empty_or_duplicate_expected_requests(self):
        for paths in [(), ("input.rb", "input.rb")]:
            with self.subTest(paths=paths), self.assertRaises(ValueError):
                self.invoke(self.report(), paths)

    def test_existing_reference_fixture_has_complete_path_sets(self):
        fixture = json.loads(
            (
                Path(__file__).resolve().parents[2] / "tests/fixtures/windows_corrections.json"
            ).read_text(encoding="utf-8")
        )
        expected_runs = []
        for section, paths in [
            ("groups", tuple(fixture["sources"])),
            ("tail_groups", tuple(fixture["tail_cases"])),
            ("removals", ("input.rb",)),
        ]:
            expected_runs.extend(
                (run, paths) for group in fixture[section] for run in group["runs"]
            )
        expected_runs.extend((run, (run["name"],)) for run in fixture["stdin"])
        expected_runs.extend((case["run"], ("input.rb",)) for case in fixture["continuations"])
        expected_runs.extend((run, ("input.rb",)) for run in fixture["composite"]["runs"])
        for index, (run, paths) in enumerate(expected_runs):
            with self.subTest(run=index):
                self.invoke(run["report"], paths, run["exit"])


if __name__ == "__main__":
    unittest.main()
