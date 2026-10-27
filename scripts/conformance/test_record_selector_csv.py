"""候補の結果を使わず、参照collectorの環境と完了検査を確認する。"""

import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_selector_csv as recorder


class RecorderContract(unittest.TestCase):
    """不完全な参照実行を期待へ混ぜる経路を拒否する。"""

    def test_ambient_ruby_options_are_removed_without_losing_runtime_paths(self):
        """外部のRuby設定で記録対象が変わらないことを守る。"""
        environment = {name: "ambient" for name in recorder.ENV_OPTIONS}
        environment.update({"GEM_PATH": "gems", "PATH": "tools", "SYSTEMROOT": "system"})
        with patch.dict(os.environ, environment, clear=True):
            actual = recorder._environment()
        self.assertEqual(actual, {"GEM_PATH": "gems", "PATH": "tools", "SYSTEMROOT": "system"})

    def test_expected_paths_and_complete_inspections_are_required(self):
        """件数だけ一致する別対象や未完走を成功へ数えない。"""
        report = {
            "files": [{"path": "input.rb", "offenses": []}],
            "summary": {"target_file_count": 1, "inspected_file_count": 1, "offense_count": 0},
        }
        recorder._validate_report(report, ["input.rb"])
        with self.assertRaises(ValueError):
            recorder._validate_report(report, ["other.rb"])
        report["summary"]["inspected_file_count"] = 0
        with self.assertRaises(ValueError):
            recorder._validate_report(report, ["input.rb"])

    def test_no_target_is_explicit_and_not_a_missing_inspection(self):
        """意図した対象ゼロと未検査を区別する。"""
        report = {
            "files": [],
            "summary": {"target_file_count": 0, "inspected_file_count": 0, "offense_count": 0},
        }
        recorder._validate_report(report, [])
        with self.assertRaises(ValueError):
            recorder._validate_report(report, ["input.rb"])

    def test_matrix_ids_and_counts_are_fixed(self):
        """重複した入力で本来の条件が脱落することを防ぐ。"""
        jobs = [
            (form["id"], mode)
            for form in recorder._forms()
            for mode in form.get("modes", ["file", "stdin"])
        ]
        self.assertEqual(len(jobs), 114)
        self.assertEqual(len(set(jobs)), 114)
        badge_jobs = [
            (form["id"], mode) for form in recorder._badge_forms() for mode in ("file", "stdin")
        ]
        self.assertEqual(len(badge_jobs), 32)
        self.assertEqual(len(set(badge_jobs)), 32)
        directory_jobs = [
            (form["id"], mode) for form in recorder._directory_forms() for mode in form["modes"]
        ]
        self.assertEqual(len(directory_jobs), 6)
        self.assertEqual(len(set(directory_jobs)), 6)

    def test_directory_inputs_are_prepared_before_starting_the_reference(self):
        """入れ子の原本と空ディレクトリを、参照の起動前に用意する。"""
        with tempfile.TemporaryDirectory() as root:
            args = SimpleNamespace(work=root, ruby="ruby", launcher="reference.rb")
            for index, form in enumerate(recorder._directory_forms()):

                def before_start(*argv, prepared=form, **kwargs):
                    directory = Path(kwargs["cwd"])
                    for name in prepared["directories"]:
                        self.assertTrue((directory / name).is_dir())
                    for name, source in prepared["inputs"].items():
                        self.assertEqual((directory / name).read_bytes(), source.encode())
                    raise RuntimeError("stop-before-reference")

                with patch.object(recorder.subprocess, "run", side_effect=before_start):
                    with self.assertRaisesRegex(RuntimeError, "stop-before-reference"):
                        recorder._record((index, form, "stdin"), args)

    def test_formatter_slots_cannot_disappear_or_appear_unrequested(self):
        """一部の保存先だけで完走扱いしない。"""
        outputs = {"first.json": {}, "second.json": {}}
        recorder._validate_outputs(outputs, True)
        with self.assertRaises(ValueError):
            recorder._validate_outputs(outputs, False)
        outputs["second.json"] = None
        with self.assertRaises(ValueError):
            recorder._validate_outputs(outputs, True)


if __name__ == "__main__":
    unittest.main()
