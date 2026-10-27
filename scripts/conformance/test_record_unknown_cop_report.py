"""採取器が親の参照オプションを隔離し、実入力の書き換えを拒むことを検査する。"""

import importlib.util
import json
import os
import subprocess
import unittest
from pathlib import Path
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    "record_unknown_cop_report", Path(__file__).with_name("record_unknown_cop_report.py")
)
COLLECTOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(COLLECTOR)


class EnvironmentContract(unittest.TestCase):
    def test_four_poisoned_options_are_removed_from_an_independent_child_copy(self):
        poisoned = {key: "poison-" + key for key in COLLECTOR.ENV_OPTIONS}
        sentinels = {
            "GEM_HOME": "keep-gems",
            "GEM_PATH": "keep-gem-paths",
            "PATH": "keep-path",
            "SystemRoot": "keep-system",
            "SONICOP_COLLECTOR_SENTINEL": "keep-other",
        }
        with mock.patch.dict(os.environ, {**poisoned, **sentinels}):
            parent = dict(os.environ)
            child = COLLECTOR._environment()
            self.assertTrue(all(key not in child for key in poisoned))
            self.assertEqual(child, {k: v for k, v in parent.items() if k not in poisoned})
            child["GEM_HOME"] = "child-only"
            self.assertEqual(dict(os.environ), parent)

    def test_real_launch_boundary_receives_the_isolated_environment(self):
        def launch(arguments, **options):
            self.assertEqual(arguments[:2], ["fixed-ruby", "-e"])
            self.assertTrue(all(key not in options["env"] for key in COLLECTOR.ENV_OPTIONS))
            self.assertEqual(options["env"]["SONICOP_COLLECTOR_SENTINEL"], "preserved")
            self.assertEqual(options["input"], b"x=1\r\n")
            self.assertTrue(options["capture_output"])
            runtime = {"ruby": "4.0.7", "parser": "3.3.12.0", "prism": "1.8.1", "rainbow": "3.1.1"}
            Path(arguments[3]).write_text(json.dumps(runtime), encoding="utf-8", newline="\n")
            return subprocess.CompletedProcess(arguments, 2, b"report", b"failure\r\n")

        with mock.patch.dict(
            os.environ,
            {
                **dict.fromkeys(COLLECTOR.ENV_OPTIONS, "poison"),
                "SONICOP_COLLECTOR_SENTINEL": "preserved",
            },
        ):
            parent = dict(os.environ)
            with mock.patch.object(COLLECTOR.subprocess, "run", side_effect=launch) as run:
                case, _ = COLLECTOR._record("fixed-ruby", COLLECTOR.SCENARIOS[0])
            self.assertEqual(run.call_count, 1)
            self.assertEqual(case["reference"]["stderr_hex"], b"failure\r\n".hex())
            self.assertEqual(dict(os.environ), parent)

    def test_source_mutation_is_a_recording_failure(self):
        def launch(arguments, **options):
            Path(arguments[3]).write_text("{}", encoding="utf-8", newline="\n")
            (options["cwd"] / "input.rb").write_bytes(b"changed")
            return subprocess.CompletedProcess(arguments, 2, b"", b"")

        with mock.patch.object(COLLECTOR.subprocess, "run", side_effect=launch):
            with self.assertRaisesRegex(ValueError, "原本が変わった"):
                COLLECTOR._record("fixed-ruby", COLLECTOR.SCENARIOS[0])

    def test_directory_inputs_are_ready_before_the_real_launch_boundary(self):
        def launch(arguments, **options):
            cwd = options["cwd"]
            self.assertTrue((cwd / "directory").is_dir())
            self.assertEqual((cwd / "directory/a.rb").read_bytes(), b"physical = true\n")
            self.assertEqual((cwd / "directory/b.rb").read_bytes(), b"physical = false\n")
            raise RuntimeError("prepared-directory-boundary")

        with mock.patch.object(COLLECTOR.subprocess, "run", side_effect=launch):
            with self.assertRaisesRegex(RuntimeError, "prepared-directory-boundary"):
                COLLECTOR._record("fixed-ruby", COLLECTOR.DIRECTORY_SCENARIOS[1])


if __name__ == "__main__":
    unittest.main()
