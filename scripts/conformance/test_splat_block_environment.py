"""splat採取器の全子プロセスへ渡す環境の契約を検査する。"""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import record_splat_block as recorder

OVERRIDES = {"RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"}
PARENT = {
    "RUBYOPT": "-rpoison.rb",
    "RUBYLIB": "poison-library",
    "RUBOCOP_OPTS": "--except Lint/Syntax",
    "RUBOCOP_TARGET_RUBY_VERSION": "0.1",
    "GEM_HOME": "expected-home",
    "GEM_PATH": "expected-gems",
    "PATH": "expected-path",
    "SystemRoot": "expected-os",
    "KEEP": "sentinel",
}


class SplatBlockEnvironmentTest(unittest.TestCase):
    """参照へ無関係な設定を注入せず、親と探索環境を保つ。"""

    def test_child_copy_drops_four_overrides_and_preserves_parent(self):
        """Gem/PATH/OSの探索値を残し、子copyの変更を親へ戻さない。"""
        with patch.dict(os.environ, PARENT, clear=True):
            before = dict(os.environ)
            child = recorder._environment()
            self.assertEqual(child, {k: v for k, v in before.items() if k not in OVERRIDES})
            child["KEEP"] = "child-only-change"
            self.assertEqual(dict(os.environ), before)

    def test_all_child_boundaries_receive_the_isolated_copy(self):
        """metadata・版照会・検査・補正前後の実値を、実mainの境界で検査する。"""
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            candidate, output = directory / "native", directory / "record.json"
            candidate.write_bytes(b"candidate identity")
            calls = []
            with patch.dict(os.environ, PARENT, clear=True):
                before = dict(os.environ)
                expected = {k: v for k, v in before.items() if k not in OVERRIDES}

                def check_environment(environment, kind):
                    self.assertEqual(environment, expected)
                    calls.append((kind, id(environment)))

                def check_output(arguments, *, env):
                    check_environment(env, "metadata" if "-e" in arguments else "version")
                    return b"{}" if "-e" in arguments else b"test-version\n"

                def run(arguments, *, env, **kwargs):
                    inspection = "-f" in arguments
                    check_environment(env, "inspection" if inspection else "runtime")
                    paths = [p for p in arguments if p.endswith(".rb")]
                    body = (
                        json.dumps(
                            {
                                "summary": {
                                    "target_file_count": len(paths),
                                    "inspected_file_count": len(paths),
                                },
                                "files": [{"path": p, "offenses": []} for p in paths],
                            }
                        ).encode()
                        if inspection
                        else b"runtime value\n"
                    )
                    return subprocess.CompletedProcess(arguments, 0, body, b"")

                with (
                    patch(
                        "sys.argv",
                        [
                            "record",
                            "--ruby",
                            str(directory / "ruby"),
                            "--sonicop",
                            str(candidate),
                            "--output",
                            str(output),
                        ],
                    ),
                    patch.dict(recorder.SOURCES, {"control": ("control", "p 1\n")}, clear=True),
                    patch.object(recorder.subprocess, "check_output", side_effect=check_output),
                    patch.object(recorder.subprocess, "run", side_effect=run),
                ):
                    recorder.main()
                self.assertEqual(dict(os.environ), before)
            kinds = [kind for kind, _ in calls]
            self.assertEqual(kinds.count("metadata"), 1)
            self.assertEqual(kinds.count("version"), 1)
            self.assertEqual(kinds.count("inspection"), 5)
            self.assertEqual(kinds.count("runtime"), 12)
            self.assertEqual(len({identity for _, identity in calls}), 1)
            self.assertEqual(len(json.loads(output.read_bytes())["controls"]), 4)
            self.assertEqual(candidate.read_bytes(), b"candidate identity")


if __name__ == "__main__":
    unittest.main()
