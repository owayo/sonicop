"""directory stdin collector が親の Ruby オプションで測定を変えないことを検査する。"""

import json
import os
import pathlib
import subprocess
import unittest
from unittest import mock

import record_stdin_directory as recorder

POISON = {
    "RUBYOPT": "-r own-poison",
    "RUBYLIB": "own-poison-lib",
    "RUBOCOP_OPTS": "--only Layout/TrailingWhitespace",
    "RUBOCOP_TARGET_RUBY_VERSION": "3.3",
}
RETAINED = {
    "GEM_HOME": "gem-home-sentinel",
    "GEM_PATH": "gem-path-sentinel",
    "PATH": "path-sentinel",
    "SystemRoot": "system-root-sentinel",
    "COLLECTOR_SENTINEL": "parent-sentinel",
}


class StdinDirectoryEnvironmentTest(unittest.TestCase):
    def test_child_copy_removes_only_four_options_and_preserves_the_parent(self):
        with mock.patch.dict(os.environ, POISON | RETAINED, clear=True):
            parent = dict(os.environ)
            child = recorder.child_environment()
            self.assertEqual(
                child, {key: value for key, value in parent.items() if key not in POISON}
            )
            self.assertEqual(dict(os.environ), parent)
            self.assertIsNot(child, os.environ)

    def case(self):
        return {
            "name": "environment-control",
            "config": recorder.BASE,
            "files": {"src/a.rb": b"disk_a=9\n".hex()},
            "source_hex": b"x=1\n".hex(),
            "kind": "json",
            "args": [],
            "stdin": "src",
            "paths": ["src/a.rb"],
            "only": "Layout/SpaceAroundOperators",
            "json_only": False,
            "mode": "-a",
            "policy": "original",
            "runtime": True,
        }

    def report(self):
        return {
            "files": [{"path": "src/a.rb", "offenses": []}],
            "summary": {
                "target_file_count": 1,
                "inspected_file_count": 1,
                "offense_count": 0,
            },
        }

    def test_record_passes_the_isolated_copy_to_ruby(self):
        with mock.patch.dict(os.environ, POISON | RETAINED, clear=True):
            parent = dict(os.environ)
            result = subprocess.CompletedProcess([], 0, json.dumps(self.report()).encode(), b"")
            with mock.patch.object(recorder.subprocess, "run", return_value=result) as run:
                recorder.record(["ruby", "reference.rb"], self.case())
            self.assertEqual(run.call_count, 1)
            self.assertEqual(
                run.call_args.kwargs["env"],
                {key: value for key, value in parent.items() if key not in POISON},
            )
            self.assertEqual(dict(os.environ), parent)

    def test_echo_inspection_and_runtime_children_share_the_same_isolated_copy(self):
        environments = []

        def run(command, **kwargs):
            environments.append(kwargs["env"])
            if command[-1] == "value.rb":
                return subprocess.CompletedProcess(command, 0, b"same-value", b"")
            (pathlib.Path(kwargs["cwd"]) / "report.json").write_text(
                json.dumps(self.report()), encoding="utf-8"
            )
            return subprocess.CompletedProcess(command, 0, b"=" * 20 + b"\r\nx = 1\r\n", b"")

        with mock.patch.dict(os.environ, POISON | RETAINED, clear=True):
            parent = dict(os.environ)
            with mock.patch.object(recorder.subprocess, "run", side_effect=run):
                recorder.record_echo(["ruby", "reference.rb"], self.case())
            # 補正、原本再検査、3つのMRI値を同じcopyで起動し、親やGem探索設定へ触れない。
            self.assertEqual(len(environments), 5)
            self.assertTrue(all(env is environments[0] for env in environments))
            self.assertEqual(
                environments[0], {key: value for key, value in parent.items() if key not in POISON}
            )
            self.assertEqual(dict(os.environ), parent)


if __name__ == "__main__":
    unittest.main()
