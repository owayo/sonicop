"""非Unicode引数collectorの子環境隔離と親環境保持の回帰。"""

import os
import unittest
from unittest.mock import patch

from record_non_unicode_cli_arguments import _environment


class NonUnicodeEnvironmentTest(unittest.TestCase):
    """Ruby/nativeへ渡すcopyだけから4キーを除く。"""

    def test_child_copy_keeps_parent_gems_path_and_os_values(self):
        """親とGem/PATH/SystemRootを保ち、子の変更も親へ戻さない。"""
        parent = {
            "RUBYOPT": "-rpoison.rb",
            "RUBYLIB": "poison-lib",
            "RUBOCOP_OPTS": "--poison-option",
            "RUBOCOP_TARGET_RUBY_VERSION": "0.1",
            "GEM_HOME": "expected-home",
            "GEM_PATH": "expected-gems",
            "PATH": "expected-path",
            "SystemRoot": "expected-os",
            "KEEP": "sentinel",
        }
        with patch.dict(os.environ, parent, clear=True):
            before = dict(os.environ)
            child = _environment()
            self.assertEqual(
                child,
                {
                    key: value
                    for key, value in before.items()
                    if key
                    not in {
                        "RUBYOPT",
                        "RUBYLIB",
                        "RUBOCOP_OPTS",
                        "RUBOCOP_TARGET_RUBY_VERSION",
                    }
                },
            )
            child["KEEP"] = "child-only-change"
            self.assertEqual(dict(os.environ), before)


if __name__ == "__main__":
    unittest.main()
