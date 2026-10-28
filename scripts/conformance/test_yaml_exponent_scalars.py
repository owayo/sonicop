"""指数scalar採取器の親環境保持を検証する。"""

import os
import unittest
from pathlib import Path
from unittest.mock import patch

from record_yaml_exponent_scalars import child_environment, collect


class EnvironmentTests(unittest.TestCase):
    """参照版の起動条件を保ち、不明な採取指定を実行前に拒否する。"""

    def test_unknown_case_is_rejected_before_starting_ruby(self):
        """既知名が混在しても、未知名のある採取ではRubyを呼ばない。"""
        for selected in [{"missing-case"}, {"plain-exponent", "missing-case"}]:
            with self.subTest(selected=selected):
                with patch("record_yaml_exponent_scalars.run") as oracle:
                    with self.assertRaisesRegex(ValueError, "missing-case"):
                        collect(Path("unused-ruby"), selected)
                    oracle.assert_not_called()

    def test_only_four_child_keys_are_removed(self):
        """Rubyへの注入変数だけを除き、親とOS・Gemの起動条件を保つ。"""
        parent = {
            key: "poison"
            for key in ["RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"]
        }
        preserved = {
            "GEM_HOME": "gem-home",
            "GEM_PATH": "gem-path",
            "PATH": "bin",
            "SystemRoot": "os",
            "OTHER": "keep",
        }
        parent.update(preserved)
        before = dict(parent)
        self.assertEqual(child_environment(parent), preserved)
        self.assertEqual(parent, before)

    def test_windows_name_case_does_not_reintroduce_a_poison(self):
        """Windowsの環境名の大文字小文字を別の注入経路にしない。"""
        parent = {"rubyopt": "poison", "PATH": "bin"}
        expected = {"PATH": "bin"} if os.name == "nt" else dict(parent)
        self.assertEqual(child_environment(parent), expected)
        self.assertEqual(parent, {"rubyopt": "poison", "PATH": "bin"})


if __name__ == "__main__":
    unittest.main()
