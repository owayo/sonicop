"""色付け collector の子環境と親環境の分離契約。"""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from record_formatter_colors import VERSIONS, _environment, _record, main


class FormatterColorEnvironmentTest(unittest.TestCase):
    """固定版を変える4キー以外の探索環境を保つ。"""

    def test_child_copy_drops_four_overrides_and_keeps_parent(self):
        """Gem/PATH/OSの値を保持し、親の辞書を変更しない。"""
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
                    not in {"RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"}
                },
            )
            child["KEEP"] = "child-only-change"
            self.assertEqual(dict(os.environ), before)


class FormatterColorRecordingTest(unittest.TestCase):
    """最適化実行でも本家の失敗を成功記録へ変えない。"""

    def test_invalid_reference_is_rejected(self):
        """終了・stderr・版・完了・入力保持をそれぞれ拒否する。"""
        for defect in ("exit", "stderr", "versions", "unfinished", "source", "mtime", "config"):
            with self.subTest(defect=defect), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary) / "case"

                def run(*args, **kwargs):
                    versions = dict(VERSIONS)
                    if defect == "versions":
                        versions["ruby"] = "unexpected"
                    (root / "versions.json").write_text(json.dumps(versions))
                    source = root / "case.rb"
                    if defect == "source":
                        source.write_bytes(b"changed\n")
                    if defect == "mtime":
                        os.utime(source, ns=(1_700_000_000_000_000_000,) * 2)
                    if defect == "config":
                        (root / "config.yml").write_bytes(b"changed\n")
                    return subprocess.CompletedProcess(
                        args[0],
                        2 if defect == "exit" else 1,
                        stdout=b"" if defect == "unfinished" else b"1 file inspected\r\n",
                        stderr=b"failed" if defect == "stderr" else b"",
                    )

                with patch("record_formatter_colors.subprocess.run", side_effect=run):
                    with self.assertRaises(RuntimeError):
                        _record(
                            "unused-ruby", root, ("case", "convention", [], b"x=1\n"), "simple", {}
                        )

    def test_existing_output_is_kept_without_launching_reference(self):
        """保存済みの参照は再採取で上書きしない。"""
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "reference.json"
            output.write_bytes(b"original")
            with (
                patch("sys.argv", ["record", "--output", str(output)]),
                patch("record_formatter_colors.collect") as collector,
            ):
                with self.assertRaisesRegex(RuntimeError, "already exists"):
                    main()
                collector.assert_not_called()
            self.assertEqual(output.read_bytes(), b"original")


if __name__ == "__main__":
    unittest.main()
