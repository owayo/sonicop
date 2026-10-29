"""missing本文の空成功と、別I/Oエラーの誤分類を参照記録へ混ぜない。"""

import copy
import json
import os
import stat
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_missing_config_errors as recorder


class ReferenceContract(unittest.TestCase):
    """本家の型と本文を分けて固定する収集契約。"""

    def fixture(self):
        """作成済み実Oracleを検証器の正例へ使う。

        Returns:
            Windowsの実CLIとerror class記録。

        """
        path = Path(__file__).resolve().parents[2] / "tests/fixtures/missing_config_errors.json"
        return json.loads(path.read_text(encoding="utf-8"))

    def test_input_matrix_and_io_envelopes_are_fixed(self):
        """17行の空configもI/O envelopeへ昇格し、入力欠落を拒む。"""
        fixture = self.fixture()
        self.assertEqual(fixture["schema"], 1)
        forms = recorder._selected_inputs(None)
        self.assertEqual(len(forms), 17)
        self.assertEqual([form["id"] for form in forms], list(recorder.CASE_IDS))
        for form, case in zip(forms, fixture["cases"], strict=True):
            self.assertEqual(form, {key: case[key] for key in form})
        self.assertEqual(
            [case["id"] for case in forms if case["coverage"] == "directory_envelope"],
            ["directory", "empty"],
        )
        for only in ([], ["unknown"], ["relative", "relative"]):
            with self.assertRaises(ValueError):
                recorder._selected_inputs(only)
        for broken in ([], forms[:-1], forms[:-1] + [forms[0]], list(reversed(forms))):
            with patch.object(recorder, "inputs", return_value=broken):
                with self.assertRaises(ValueError):
                    recorder._selected_inputs(None)

    def test_fresh_output_and_work_keep_existing_and_dangling_entries(self):
        """本家を起動する前にlstatでentryを調べ、参照先の無いsymlinkも守る。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            original_lstat = os.lstat
            for index in range(2):
                destinations = [root / f"{index}-work", root / f"{index}-out"]

                def _lstat(path, *args, **kwargs):
                    if Path(path) == destinations[index]:
                        return SimpleNamespace(st_mode=stat.S_IFLNK)
                    return original_lstat(path, *args, **kwargs)

                with (
                    patch("os.lstat", side_effect=_lstat),
                    patch.object(recorder.subprocess, "run") as run,
                ):
                    with self.assertRaises(ValueError):
                        recorder.record(Path("unused"), destinations[1], destinations[0])
                    run.assert_not_called()
                destinations[index].write_bytes(b"keep")
                with patch.object(recorder.subprocess, "run") as run:
                    with self.assertRaises(ValueError):
                        recorder.record(Path("unused"), destinations[1], destinations[0])
                    run.assert_not_called()
                self.assertEqual(destinations[index].read_bytes(), b"keep")

    def test_early_phase_template_is_complete_and_separate(self):
        """version identityやpending通知の残差を、Missing本文の成功件数へ混ぜない。"""
        path = Path(__file__).parents[2] / "tests/fixtures/missing_config_early_phases.json"
        fixture = json.loads(path.read_text(encoding="utf-8"))
        recorder._validate_early_template(fixture)
        self.assertEqual(len(fixture["cases"]), 8)
        self.assertEqual(sum(case["coverage"] == "missing" for case in fixture["cases"]), 2)
        for cases in (
            [],
            fixture["cases"][:-1],
            list(reversed(fixture["cases"])),
            fixture["cases"][:-1] + [fixture["cases"][0]],
        ):
            broken = {**fixture, "cases": cases}
            with self.assertRaises(ValueError):
                recorder._validate_early_template(broken)

    def test_missing_body_and_other_io_error_are_not_interchangeable(self):
        """ENOENTへの誤分類や空出力が成功として保存されることを防ぐ。"""
        fixture = self.fixture()
        for case in fixture["cases"]:
            error = {"error_class": case["error_class"], "message": case["error_message_template"]}
            stderr = (
                (case["error_message_template"] + "\r\n").encode()
                if case["kind"] == "io"
                else bytes.fromhex(case["stderr_hex"])
            )
            recorder.validate_case(
                case,
                error,
                bytes.fromhex(case["stdout_hex"]),
                stderr,
                case["out_hex"],
                fixture["runtime"]["rainbow_initial"],
            )
        case = next(case for case in fixture["cases"] if case["id"] == "relative")
        error = {"error_class": case["error_class"], "message": case["error_message_template"]}
        with self.assertRaises(ValueError):
            recorder.validate_case(case, error, b"", b"", None, False)
        wrong = {**error, "error_class": "Errno::EISDIR"}
        with self.assertRaises(ValueError):
            recorder.validate_case(
                case, wrong, recorder.NOTICE, bytes.fromhex(case["stderr_hex"]), None, False
            )

    def test_metadata_and_complete_output_are_required(self):
        """版や条件集合の欠落、未完了の通常検査を拒む。"""
        fixture = self.fixture()
        metadata = {**fixture, "classes": [{"id": case["id"]} for case in fixture["cases"]]}
        names = [case["id"] for case in fixture["cases"]]
        recorder.validate_metadata(metadata, names)
        incomplete = copy.deepcopy(metadata)
        incomplete["classes"].pop()
        with self.assertRaises(ValueError):
            recorder.validate_metadata(incomplete, names)
        wrong = copy.deepcopy(metadata)
        wrong["runtime"]["gems"]["rainbow"] = "0.0"
        with self.assertRaises(ValueError):
            recorder.validate_metadata(wrong, names)
        case = next(case for case in fixture["cases"] if case["kind"] == "success")
        report = json.loads(bytes.fromhex(case["out_hex"]))
        report["summary"]["inspected_file_count"] = 0
        with self.assertRaises(ValueError):
            recorder.validate_case(
                case,
                {"error_class": None},
                recorder.NOTICE,
                b"",
                json.dumps(report).encode().hex(),
                False,
            )


if __name__ == "__main__":
    unittest.main()
