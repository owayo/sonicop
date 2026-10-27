"""debug参照記録が不完全な実行や診断の正規化を成功へ混ぜないことを守る。"""

import json
import os
import unittest
from unittest.mock import patch

import record_debug_streams as recorder


class DebugRecorderContract(unittest.TestCase):
    """実行環境、完了件数、動的metadataの投影範囲を検証する。"""

    def test_runtime_paths_survive_without_ambient_options(self):
        """外部のRuby設定を消しつつGemPathとOS環境を保持する。"""
        environment = {key: "ambient" for key in recorder.AMBIENT}
        environment.update({"GEM_PATH": "gems", "PATH": "tools", "SYSTEMROOT": "system"})
        with patch.dict(os.environ, environment, clear=True):
            actual = recorder._environment()
        self.assertEqual(actual, {"GEM_PATH": "gems", "PATH": "tools", "SYSTEMROOT": "system"})

    def test_incomplete_or_different_paths_are_rejected(self):
        """件数だけが同じ別対象を比較成功へしない。"""
        report = {
            "files": [{"path": "a.rb", "offenses": []}],
            "summary": {"target_file_count": 1, "inspected_file_count": 1, "offense_count": 0},
        }
        recorder._validate_report(report, 1, ["a.rb"])
        with self.assertRaises(ValueError):
            recorder._validate_report(report, 1, ["b.rb"])
        report["summary"]["inspected_file_count"] = 0
        with self.assertRaises(ValueError):
            recorder._validate_report(report, 1, ["a.rb"])

    def test_projection_does_not_rewrite_json_diagnostics(self):
        """pathを含む診断も、動的stdout pathと同一視して消さない。"""
        value = {
            "metadata": {
                "rubocop_version": "1.89.0",
                "ruby_engine": "ruby",
                "ruby_version": "4.0.7",
                "ruby_platform": "x64-mingw-ucrt",
            },
            "files": [
                {
                    "path": "a.rb",
                    "offenses": [
                        {"message": "/project/diagnostic", "location": {"last_column": 9}}
                    ],
                }
            ],
            "summary": {},
        }
        raw = "Scanning /project/a.rb\r\n" + json.dumps(value, separators=(",", ":"))
        projected = recorder._common_projection(raw, "/project")
        self.assertEqual(projected["text"], "Scanning <cwd>/a.rb\r\n<JSON[0]>")
        self.assertEqual(projected["reports"][0]["files"], value["files"])

    def test_timer_projection_requires_the_last_output_position(self):
        """末尾以外の同じ文言をensure終了として扱わない。"""
        projected = recorder._common_projection("Finished in 0.25 seconds\r\n", "/project")
        self.assertEqual(projected["timer_count"], 1)
        self.assertEqual(projected["text"], "Finished in <elapsed> seconds\r\n")
        projected = recorder._common_projection("Finished in 0.25 seconds\r\nafter\r\n", "/project")
        self.assertEqual(projected["timer_count"], 0)

    def test_finite_matrix_keeps_unique_conditions(self):
        """重複したケースで元の条件が欠けることを防ぐ。"""
        forms = recorder._forms()
        self.assertEqual(len(forms), 27)
        self.assertEqual(len({form["id"] for form in forms}), 27)
        self.assertEqual(sum(form["coverage"] == "interface" for form in forms), 21)


if __name__ == "__main__":
    unittest.main()
