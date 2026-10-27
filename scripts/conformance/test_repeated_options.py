"""cache/configの参照採取で、未完了や原本変更を成功へ変えない。"""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from record_repeated_options import VERSIONS, _record, main


class RepeatedOptionsReferenceTest(unittest.TestCase):
    """本家の成功記録だけを保存し、既存参照を保持する。"""

    def test_incomplete_or_mutated_reference_is_rejected(self):
        """終了・stderr・版・対象完了・bytes/statの各欠落を個別に拒否する。"""
        for defect in (
            "exit",
            "stderr",
            "versions",
            "unfinished",
            "paths",
            "source",
            "config",
            "mtime",
        ):
            with self.subTest(defect=defect), tempfile.TemporaryDirectory() as temporary:
                project = Path(temporary) / "project"

                def run(arguments, *, defect=defect, project=project, **kwargs):
                    versions = dict(VERSIONS)
                    if defect == "versions":
                        versions["ruby"] = "unexpected"
                    (project / "versions.json").write_text(json.dumps(versions))
                    if defect in ("source", "config"):
                        name = "case.rb" if defect == "source" else ".rubocop.yml"
                        (project / name).write_bytes(b"changed")
                    if defect == "mtime":
                        source = project / "case.rb"
                        os.utime(source, ns=(source.stat().st_mtime_ns + 2_000_000_000,) * 2)
                    report = {
                        "metadata": {},
                        "summary": {
                            "target_file_count": 1,
                            "inspected_file_count": 0 if defect == "unfinished" else 1,
                        },
                        "files": [{"path": "other.rb" if defect == "paths" else "case.rb"}],
                    }
                    return subprocess.CompletedProcess(
                        arguments,
                        2 if defect == "exit" else 0,
                        json.dumps(report).encode(),
                        b"warning" if defect == "stderr" else b"",
                    )

                with patch("record_repeated_options.subprocess.run", side_effect=run):
                    with self.assertRaises(RuntimeError):
                        _record(
                            "unused-ruby",
                            project,
                            {"id": "case", "kind": "report", "options": ["--cache", "false"]},
                        )

    def test_redirected_report_can_use_only_one_output_slot(self):
        """formatter前のoutを使わず、後のFileだけへ書く本家の有効形を受け取る。"""
        with tempfile.TemporaryDirectory() as temporary:
            project = Path(temporary) / "project"
            report = {
                "metadata": {},
                "summary": {"target_file_count": 1, "inspected_file_count": 1},
                "files": [{"path": "case.rb"}],
            }

            def run(arguments, **kwargs):
                (project / "versions.json").write_text(json.dumps(VERSIONS))
                (project / "second.json").write_text(json.dumps(report))
                return subprocess.CompletedProcess(arguments, 0, b"", b"")

            with patch("record_repeated_options.subprocess.run", side_effect=run):
                row = _record(
                    "unused-ruby", project, {"id": "case", "kind": "report", "options": []}
                )
            self.assertIsNone(row["report"])
            self.assertIsNone(row["outputs"]["first.json"])
            self.assertEqual(row["outputs"]["second.json"]["files"], report["files"])

    def test_existing_recording_is_kept_before_launch(self):
        """保存済みの参照は子プロセスの起動前に上書きから守る。"""
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "reference.json"
            output.write_bytes(b"original")
            with (
                patch("sys.argv", ["record", str(output)]),
                patch("record_repeated_options.subprocess.run") as child,
            ):
                with self.assertRaisesRegex(RuntimeError, "already exists"):
                    main()
                child.assert_not_called()
            self.assertEqual(output.read_bytes(), b"original")

    def test_duplicate_cases_are_rejected_before_launch(self):
        """重複IDで途中まで採取してしまうことを防ぐ。"""
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "reference.json"
            with (
                patch("sys.argv", ["record", str(output)]),
                patch("record_repeated_options._cases", return_value=[{"id": "same"}] * 2),
                patch("record_repeated_options.subprocess.run") as child,
            ):
                with self.assertRaisesRegex(RuntimeError, "duplicate"):
                    main()
                child.assert_not_called()
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
