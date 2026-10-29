"""表示filter4追補の型・完了・原本・保存先guardを検査する。"""

import copy
import json
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_late_config_display_filter as recorder


class DisplayFilterContract(unittest.TestCase):
    """主38へ混ぜず、陽性1とfiltered3を実参照で固定する。"""

    def test_fixture_identity_counts_and_offenses_are_exact(self):
        """bool件数・誤engine・不完全なoffenseを通常0完了へ混ぜない。"""
        value = recorder._template()
        recorder.validate_fixture(value)
        self.assertEqual(len(recorder.case_inputs()), 4)
        expected = value["positive_control"]
        report = json.loads(bytes.fromhex(expected["stdout_hex"]))
        for key in ("offense_count", "target_file_count", "inspected_file_count"):
            broken = copy.deepcopy(report)
            broken["summary"][key] = True
            with self.assertRaises(ValueError):
                recorder._report(json.dumps(broken).encode(), expected)
        for mutation in ("offense", "engine", "paths"):
            broken = copy.deepcopy(report)
            if mutation == "offense":
                broken["files"][0]["offenses"] = []
            elif mutation == "engine":
                broken["metadata"]["ruby_engine"] = "sonicop"
            else:
                broken["files"] = []
            with self.assertRaises(ValueError):
                recorder._report(json.dumps(broken).encode(), expected)
        broken = copy.deepcopy(value)
        broken["cases"][0]["exit"] = False
        with self.assertRaises(ValueError):
            recorder.validate_fixture(broken)

    def test_record_reproduces_all_four_actual_rows(self):
        """陽性1とfiltered3は実rawの全fieldを再生成する。"""
        rows = [recorder._template()["positive_control"], *recorder._template()["cases"]]
        for form, row in zip(recorder.case_inputs(), rows, strict=True):
            child = SimpleNamespace(
                returncode=row["exit"], stdout=bytes.fromhex(row["stdout_hex"]), stderr=b""
            )
            with (
                tempfile.TemporaryDirectory() as folder,
                patch.object(recorder.subprocess, "run", return_value=child),
            ):
                self.assertEqual(recorder.record("ruby", form, Path(folder) / "case"), row)

    def test_source_bytes_mtime_and_out_are_independent_negatives(self):
        """正しいCLI出力を保ち、本文だけ/mtimeだけ/outだけの変更を独立に拒む。"""
        form = recorder.case_inputs()[1]
        row = recorder._template()["cases"][0]
        for mutation in ("bytes", "mtime", "out"):
            with tempfile.TemporaryDirectory() as folder:
                work = Path(folder) / "case"

                def child(*args, **kwargs):
                    path = work / "a.rb"
                    before = path.stat()
                    if mutation == "bytes":
                        path.write_bytes(b"changed")
                        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
                    elif mutation == "mtime":
                        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns + 1_000_000_000))
                    else:
                        (work / "report.json").write_bytes(b"changed")
                    return SimpleNamespace(
                        returncode=row["exit"], stdout=bytes.fromhex(row["stdout_hex"]), stderr=b""
                    )

                with (
                    patch.object(recorder.subprocess, "run", side_effect=child),
                    self.assertRaises(ValueError),
                ):
                    recorder.record("ruby", form, work)

    def test_input_and_fresh_alias_guards_precede_child(self):
        """同IDの入力差と保存先aliasを、子process起動前に拒む。"""
        form = recorder.case_inputs()[0]
        form["args"] = []
        with (
            tempfile.TemporaryDirectory() as folder,
            patch.object(recorder.subprocess, "run") as run,
        ):
            root = Path(folder)
            with self.assertRaises(ValueError):
                recorder.record("ruby", form, root / "case")
            with self.assertRaises(ValueError):
                recorder._fresh_paths(root / "work", root / "sub" / ".." / "work")
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
