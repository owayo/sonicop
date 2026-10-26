"""子Rubyの条件だけを固定し、呼出元の実行環境を維持する契約。"""

import json
import os
import subprocess
import unittest
from pathlib import Path
from unittest import mock

import record_multibyte_syntax as recorder

ENV_KEYS = ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")


class RubyEnvironmentTest(unittest.TestCase):
    """参照結果を変える4keyと、起動に必要な親のkeyを区別する。"""

    def test_child_removes_only_ambient_ruby_overrides(self):
        """gem/OSの環境を削りすぎず、子copyの変更も親へ戻さない。"""
        parent = {
            **dict.fromkeys(ENV_KEYS, "poison"),
            "GEM_HOME": "kept-gem-home",
            "GEM_PATH": "kept-gem-path",
            "PATH": "kept-path",
            "SystemRoot": "kept-system-root",
            "SONICOP_ENV_SENTINEL": "kept-parent-value",
        }
        with mock.patch.dict(os.environ, parent, clear=True):
            before = dict(os.environ)
            child = recorder.ruby_environment()
            for name in ENV_KEYS:
                self.assertFalse(name in child, f"ambient override retained: {name}")
            self.assertEqual(
                child, {k: v for k, v in before.items() if k not in ENV_KEYS}
            )
            child["SONICOP_ENV_SENTINEL"] = "child-only"
            self.assertEqual(dict(os.environ), before)

    def test_every_ruby_call_uses_the_clean_child_environment(self):
        """7回すべてへ同じ除去契約を渡し、測定値やfixtureは変更しない。"""
        fixture = json.loads(
            (
                Path(__file__).resolve().parents[1]
                / "tests/fixtures/multibyte_syntax_reports.json"
            ).read_text(encoding="utf-8")
        )
        calls = []

        def reference(command, **kwargs):
            environment = kwargs["env"]
            for name in ENV_KEYS:
                self.assertFalse(
                    name in environment, f"ambient override retained: {name}"
                )
            expected = {k: v for k, v in os.environ.items() if k not in ENV_KEYS}
            self.assertEqual(set(environment), set(expected))
            for name, value in expected.items():
                self.assertTrue(
                    environment.get(name) == value, f"changed inherited key: {name}"
                )
            self.assertIsNot(environment, os.environ)
            group = fixture["groups"][len(calls)]
            calls.append(command)
            return subprocess.CompletedProcess(
                command,
                group["exit"],
                json.dumps(
                    {"files": group["files"], "summary": group["summary"]}
                ).encode(),
                b"",
            )

        # fixtureを再採取せず、全7呼出しへのenv結線をsubprocessの置換で調べる。
        with mock.patch.dict(os.environ, dict.fromkeys(ENV_KEYS, "poison")):
            before = dict(os.environ)
            with mock.patch.object(recorder.subprocess, "run", side_effect=reference):
                actual = recorder.record("ruby")
            self.assertTrue(dict(os.environ) == before, "parent environment changed")
        self.assertEqual(len(calls), 7)
        self.assertEqual(actual, fixture)


if __name__ == "__main__":
    unittest.main()
