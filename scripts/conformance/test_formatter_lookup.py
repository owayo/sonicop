"""formatterの参照採取が未完了や原本変更を成功へ変えないことを検査する。"""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from record_formatter_lookup import VERSIONS, _record, main


class FormatterLookupReferenceTest(unittest.TestCase):
    """Python -Oでも採取条件を省略しない。"""

    def test_incomplete_reference_is_rejected(self):
        """終了・出力・版・原本・File初期化の欠落を個別に拒否する。"""
        for defect in (
            "exit",
            "stdout",
            "header",
            "versions",
            "source",
            "config",
            "first-file",
            "later-file",
        ):
            with self.subTest(defect=defect), tempfile.TemporaryDirectory() as temporary:
                project = Path(temporary) / "project"

                def run(*args, defect=defect, project=project, **kwargs):
                    versions = dict(VERSIONS)
                    if defect == "versions":
                        versions["ruby"] = "unexpected"
                    (project / "versions.json").write_text(json.dumps(versions))
                    frame_path = "formatter_set.rb"
                    (project / "formatter_source.json").write_text(json.dumps(frame_path))
                    if defect != "first-file":
                        (project / "first.txt").write_bytes(b"")
                    if defect == "later-file":
                        (project / "second.txt").write_bytes(b"")
                    if defect in ("source", "config"):
                        (project / ("a.rb" if defect == "source" else "config.yml")).write_bytes(
                            b"changed"
                        )
                    return subprocess.CompletedProcess(
                        args[0],
                        0 if defect == "exit" else 2,
                        stdout=b"unexpected" if defect == "stdout" else b"",
                        stderr=(
                            b"invalid\n"
                            if defect == "header"
                            else b'Formatter "unknown_formatter" not found.'
                            + (b"\r\n" if os.name == "nt" else b"\n")
                        )
                        + b"formatter_set.rb:1:in 'reference'\n",
                    )

                with patch("record_formatter_lookup.subprocess.run", side_effect=run):
                    with self.assertRaises(RuntimeError):
                        _record(
                            "unused-ruby",
                            project,
                            ("case", ["-f", "unknown_formatter", "-o", "first.txt"]),
                            {},
                        )

    def test_existing_recording_is_kept(self):
        """保存済みの期待値を採取前に上書きから守る。"""
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "recording.json"
            output.write_bytes(b"original")
            with (
                patch("sys.argv", ["record", str(output)]),
                patch("record_formatter_lookup.subprocess.run") as child,
            ):
                with self.assertRaisesRegex(RuntimeError, "already exists"):
                    main()
                child.assert_not_called()
            self.assertEqual(output.read_bytes(), b"original")


if __name__ == "__main__":
    unittest.main()
