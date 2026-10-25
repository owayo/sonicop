"""空stdin collectorのRuby子だけを隔離し、親とGem探索を維持する契約。"""

import json
import os
import subprocess
import unittest
from unittest import mock

import record_empty_stdin as recorder

POISON = {
    "RUBYOPT": "-r own-poison",
    "RUBYLIB": "own-poison-lib",
    "RUBOCOP_OPTS": "--only Layout/TrailingWhitespace",
    "RUBOCOP_TARGET_RUBY_VERSION": "2.7",
}
RETAINED = {
    "GEM_HOME": "gem-home-sentinel",
    "GEM_PATH": "gem-path-sentinel",
    "PATH": "path-sentinel",
    "SystemRoot": "system-root-sentinel",
    "COLLECTOR_SENTINEL": "parent-sentinel",
}


class EmptyStdinEnvironmentTest(unittest.TestCase):
    def test_child_copy_removes_only_four_options_without_changing_the_parent(self):
        with mock.patch.dict(os.environ, POISON | RETAINED, clear=True):
            parent = dict(os.environ)
            child = recorder.child_environment()
            self.assertEqual(
                child,
                {key: value for key, value in parent.items() if key not in POISON},
            )
            self.assertEqual(dict(os.environ), parent)
            self.assertIsNot(child, os.environ)

    def test_default_inspection_uses_the_isolated_child_copy(self):
        case = next(
            row for row in recorder.specifications() if row["name"] == "empty-long-crlf"
        )
        result = subprocess.CompletedProcess([], 0, b'{"metadata":{},"files":[]}', b"")
        with mock.patch.dict(os.environ, POISON | RETAINED, clear=True):
            parent = dict(os.environ)
            with mock.patch.object(
                recorder.subprocess, "run", return_value=result
            ) as run:
                recorder.run("ruby", "reference.rb", case)
            self.assertEqual(run.call_count, 1)
            self.assertEqual(
                run.call_args.kwargs["env"],
                {key: value for key, value in parent.items() if key not in POISON},
            )
            self.assertEqual(dict(os.environ), parent)

    def test_inspection_and_correction_rerun_share_the_supplied_copy(self):
        case = next(
            row for row in recorder.specifications() if row["name"] == "correct-safe"
        )
        environments = []

        def run(command, **kwargs):
            environments.append(kwargs["env"])
            formatter = command[command.index("--format") + 1]
            stdout = (
                json.dumps({"metadata": {}, "files": []}).encode()
                if formatter == "json"
                else b"report\r\n" + b"=" * 20 + b"\r\nx = 1\r\n"
            )
            return subprocess.CompletedProcess(command, 0, stdout, b"")

        with mock.patch.dict(os.environ, POISON | RETAINED, clear=True):
            parent = dict(os.environ)
            child = recorder.child_environment()
            with mock.patch.object(recorder.subprocess, "run", side_effect=run):
                recorder.run("ruby", "reference.rb", case, child)
            # 補正後の再実行も同じcopyを使い、親やGem探索用の変数を破壊しない。
            self.assertEqual(len(environments), 2)
            self.assertTrue(all(env is child for env in environments))
            self.assertEqual(dict(os.environ), parent)


if __name__ == "__main__":
    unittest.main()
