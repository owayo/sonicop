"""namespace参照記録の完了・環境・比較範囲を検証する。"""

import copy
import os
import stat
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_config_cop_namespaces as recorder


class NamespaceRecorderContract(unittest.TestCase):
    """未知残差や空の偽reportを、値解決一致へ数えない。"""

    def test_environment_keeps_os_and_gem_paths(self):
        """Ruby注入の4変数だけを子copyから除く。"""
        parent = {
            name: "poison"
            for name in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
        }
        parent.update({"GEM_PATH": "gems", "PATH": "tools", "SYSTEMROOT": "system"})
        with patch.dict(os.environ, parent, clear=True):
            self.assertEqual(
                recorder.reference_environment(),
                {"GEM_PATH": "gems", "PATH": "tools", "SYSTEMROOT": "system"},
            )

    def test_finite_counts_exclude_the_known_unknown_gap(self):
        """42行を42成功へ読み替えることを防ぐ。"""
        forms = recorder.case_inputs()
        recorder._validate_cases(forms)
        self.assertEqual(len({case["id"] for case in forms}), 42)
        self.assertEqual(
            [case["id"] for case in forms if case["coverage"] == "known_residual"], ["unknown_bare"]
        )

    def test_missing_or_duplicate_input_fails_closed(self):
        """件数だけの一致では別条件を埋め合わせさせない。"""
        forms = recorder.case_inputs()
        unknown = copy.deepcopy(forms)
        unknown[0]["id"] = "unknown-input"
        unknown_coverage = copy.deepcopy(forms)
        unknown_coverage[0]["coverage"] = "unknown-coverage"
        for broken in (
            [],
            forms[:-1],
            forms[:-1] + [forms[0]],
            list(reversed(forms)),
            unknown,
            unknown_coverage,
        ):
            with self.assertRaises(ValueError):
                recorder._validate_cases(broken)

    def test_metadata_and_completion_are_required(self):
        """他toolや未検査を通常JSONとして取り込まない。"""
        report = {
            "metadata": {
                "rubocop_version": "1.89.0",
                "ruby_engine": "ruby",
                "ruby_version": "4.0.7",
            },
            "files": [{"path": "a.rb", "offenses": []}],
            "summary": {"target_file_count": 1, "inspected_file_count": 1, "offense_count": 0},
        }
        recorder._report(recorder.json.dumps(report), ["a.rb"])
        for key in ("target_file_count", "inspected_file_count"):
            broken = copy.deepcopy(report)
            broken["summary"][key] = 0
            with self.assertRaises(ValueError):
                recorder._report(recorder.json.dumps(broken), ["a.rb"])
        report["metadata"]["ruby_engine"] = "sonicop"
        with self.assertRaises(ValueError):
            recorder._report(recorder.json.dumps(report), ["a.rb"])

    def test_fixture_inputs_are_the_complete_recorded_matrix(self):
        """採取scriptと参照fixtureの入力が別々に欠落することを防ぐ。"""
        fixture = recorder.json.loads(
            (Path(__file__).parents[2] / "tests/fixtures/config_cop_namespaces.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(fixture["schema"], 1)
        recorder._validate_cases(fixture["cases"])
        for form, case in zip(recorder.case_inputs(), fixture["cases"], strict=True):
            self.assertEqual(form, {key: case[key] for key in form})

    def test_output_and_work_entries_cannot_be_overwritten(self):
        """既存entryを採取前に拒否し、原本へ到達させない。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for index in range(3):
                destinations = [root / f"{index}-{name}" for name in ("work", "out", "legacy")]
                destinations[index].write_bytes(b"keep")
                args = [
                    "recorder",
                    "--ruby",
                    "unused",
                    "--work",
                    str(destinations[0]),
                    "--out",
                    str(destinations[1]),
                    "--legacy-out",
                    str(destinations[2]),
                ]
                with patch("sys.argv", args), patch.object(recorder, "_registry") as registry:
                    with self.assertRaises(ValueError):
                        recorder.main()
                    registry.assert_not_called()
                self.assertEqual(destinations[index].read_bytes(), b"keep")

    def test_dangling_destinations_are_rejected_without_following_them(self):
        """Windowsのsymlink作成権限によらず、参照先が無いentryの拒否を検査する。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            original_lstat = os.lstat
            for index in range(3):
                destinations = [root / f"link-{index}-{name}" for name in ("work", "out", "legacy")]

                def _lstat(path, *args, **kwargs):
                    if Path(path) == destinations[index]:
                        return SimpleNamespace(st_mode=stat.S_IFLNK)
                    return original_lstat(path, *args, **kwargs)

                args = [
                    "recorder",
                    "--ruby",
                    "unused",
                    "--work",
                    str(destinations[0]),
                    "--out",
                    str(destinations[1]),
                    "--legacy-out",
                    str(destinations[2]),
                ]
                with (
                    patch("sys.argv", args),
                    patch("os.lstat", side_effect=_lstat),
                    patch.object(recorder, "_registry") as registry,
                ):
                    self.assertFalse(destinations[index].exists())
                    self.assertTrue(os.path.lexists(destinations[index]))
                    with self.assertRaises(ValueError):
                        recorder.main()
                    registry.assert_not_called()
                self.assertFalse(any(path.exists() for path in destinations))

    def test_only_the_ambiguous_error_origin_is_projected(self):
        """警告文の任意部分をcwd文字列と同一視して消さない。"""
        directory = Path("/project")
        raw = b"config.yml: Warning: keep /project literal\r\nError: Ambiguous used in /project/config.yml needs department qualifier.\r\n"
        projected = recorder._public_stderr(raw, directory, "ambiguity_bytes")
        self.assertIn("keep /project literal", projected)
        self.assertIn(" used in <cwd>/config.yml needs department qualifier.", projected)
        with self.assertRaises(ValueError):
            recorder._public_stderr(raw[:-2] + b"\n", directory, "ambiguity_bytes")
        with self.assertRaises(ValueError):
            recorder._public_stderr(b"wrong error", directory, "ambiguity_bytes")


if __name__ == "__main__":
    unittest.main()
