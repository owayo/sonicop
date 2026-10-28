"""未知設定採取器の完了判定・ENV境界を有限の負対照で検査する。"""

import copy
import json
import os
import stat
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_unknown_config_cops as recorder


def fixture():
    """固定した実Oracle記録を、候補を呼ばずに単体対照へ渡す。

    Returns:
        dict: 公開記録の入力と実参照出力。

    """
    return json.loads(
        (
            Path(__file__).resolve().parents[2] / "tests/fixtures/unknown_config_cops.json"
        ).read_bytes()
    )


class UnknownConfigCopsTest(unittest.TestCase):
    """空reportを受理せず、子にだけ明示条件を渡す。"""

    def test_finite_scenarios_have_unique_identity_and_scope(self):
        """未対応145の2条件は元rawを残し、strict40へ混ぜない。"""
        cases = recorder.scenarios()
        self.assertEqual(len(cases), 42)
        self.assertEqual(len({case["id"] for case in cases}), 42)
        self.assertEqual(sum(case["coverage"] == "strict" for case in cases), 40)
        self.assertEqual(sum(case["coverage"] == "deferred_145" for case in cases), 2)
        recorder._selected_inputs(None)
        for only in ([], ["unknown"], ["unknown-map", "unknown-map"]):
            with self.assertRaises(ValueError):
                recorder._selected_inputs(only)
        for broken in ([], cases[:-1], cases[:-1] + [cases[0]], list(reversed(cases))):
            with patch.object(recorder, "scenarios", return_value=broken):
                with self.assertRaises(ValueError):
                    recorder._selected_inputs(None)
        self.assertTrue(all(case["args"].count("--cache") == 1 for case in cases))

    def test_fixture_inputs_and_trace_scope_match_the_recorded_matrices(self):
        """元42と追補4の由来を分け、どちらの入力も空や別条件へすり替えない。"""
        reference = fixture()
        self.assertEqual(reference["schema"], 1)
        for form, case in zip(recorder._selected_inputs(None), reference["cases"], strict=True):
            self.assertEqual(form, {key: case[key] for key in form})
        trace = json.loads(
            (Path(__file__).parents[2] / "tests/fixtures/unknown_config_trace.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(trace["schema"], 1)
        for form, case in zip(recorder._trace_inputs(), trace["cases"], strict=True):
            self.assertEqual(form, {key: case[key] for key in form})
        self.assertEqual(len(trace["cases"]), 4)
        for only in ([], ["unknown"], [recorder.TRACE_IDS[0], recorder.TRACE_IDS[0]]):
            with self.assertRaises(ValueError):
                recorder._trace_inputs(only)

    def test_fresh_output_and_work_reject_existing_and_dangling_entries(self):
        """本家を起動する前にlstatでentryを確認し、参照先の無いsymlinkも守る。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            original_lstat = os.lstat
            for index in range(2):
                destinations = [root / f"{index}-work", root / f"{index}-out"]

                def _lstat(path, *args, **kwargs):
                    if Path(path) == destinations[index]:
                        return SimpleNamespace(st_mode=stat.S_IFLNK)
                    return original_lstat(path, *args, **kwargs)

                with patch("os.lstat", side_effect=_lstat):
                    self.assertFalse(destinations[index].exists())
                    with self.assertRaises(ValueError):
                        recorder._fresh_paths(*destinations)
                destinations[index].write_bytes(b"keep")
                with self.assertRaises(ValueError):
                    recorder._fresh_paths(*destinations)
                self.assertEqual(destinations[index].read_bytes(), b"keep")

    def test_wrong_version_incomplete_and_duplicate_catalogs_are_rejected(self):
        """版や集合の件数だけを偽って完了扱いにしない。"""
        api = fixture()["api"]
        recorder.validate_api(api)
        for key in ["ruby", "gems", "registry", "departments", "legacy", "default_keys"]:
            changed = copy.deepcopy(api)
            changed[key] = []
            with self.assertRaises(ValueError):
                recorder.validate_api(changed)
        changed = copy.deepcopy(api)
        changed["registry"][0] = changed["registry"][1]
        with self.assertRaises(ValueError):
            recorder.validate_api(changed)

    def test_empty_duplicate_wrong_path_and_false_counts_are_rejected(self):
        """検査対象と全summaryが揃わないJSONを拒む。"""
        expected = next(
            case["report"]
            for case in fixture()["cases"]
            if case.get("report", {}).get("summary", {}).get("offense_count", 0) > 0
        )
        valid = {
            "metadata": {
                "rubocop_version": "1.89.0",
                "ruby_engine": "ruby",
                "ruby_version": "4.0.7",
            },
            **expected,
        }
        self.assertEqual(recorder.completed_report(json.dumps(valid).encode()), expected)
        invalid = []
        for metadata in (
            {},
            {"rubocop_version": "1.89.0", "ruby_engine": "sonicop", "ruby_version": "4.0.7"},
        ):
            changed = copy.deepcopy(valid)
            changed["metadata"] = metadata
            invalid.append(changed)
        for key, value in [
            ("target_file_count", 0),
            ("inspected_file_count", 0),
            ("inspected_file_count", True),
            ("offense_count", -1),
        ]:
            changed = copy.deepcopy(valid)
            changed["summary"][key] = value
            invalid.append(changed)
        for files in [[], valid["files"] * 2, [{"path": "wrong.rb", "offenses": []}]]:
            changed = copy.deepcopy(valid)
            changed["files"] = files
            invalid.append(changed)
        for changed in invalid:
            with self.assertRaises(ValueError):
                recorder.completed_report(json.dumps(changed).encode())

    def test_record_copies_environment_and_preserves_exact_reference_bytes(self):
        """親ENVを保ち、除去4keyと明示色入力だけを子で切り替える。"""
        expected = next(case for case in fixture()["cases"] if case["id"] == "unknown-map")
        case = next(case for case in recorder.scenarios() if case["id"] == "unknown-map")
        child = SimpleNamespace(
            returncode=expected["exit"],
            stdout=bytes.fromhex(expected["stdout_hex"]),
            stderr=bytes.fromhex(expected["stderr_hex"]),
        )
        keys = {"RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"}
        with patch.dict(os.environ, {**{key: "poison" for key in keys}, "KEEP_SENTINEL": "keep"}):
            before = dict(os.environ)
            with patch.object(recorder.subprocess, "run", return_value=child) as run:
                self.assertEqual(recorder.record("ruby", case), expected)
            self.assertEqual(dict(os.environ), before)
            self.assertEqual(
                run.call_args.kwargs["env"],
                {**{key: value for key, value in before.items() if key not in keys}, **case["env"]},
            )
            self.assertIn("gem 'rubocop', '1.89.0'", run.call_args.args[0][2])

    def test_rejection_with_stdout_artifact_empty_error_or_bad_exit_is_rejected(self):
        """Error後にformatterを作った偽CLIや異常終了を拒む。"""
        case = recorder.scenarios()[0]
        for child in [
            SimpleNamespace(returncode=3, stdout=b"", stderr=b"error"),
            SimpleNamespace(returncode=2, stdout=b"{}", stderr=b"error"),
            SimpleNamespace(returncode=2, stdout=b"", stderr=b""),
        ]:
            with (
                patch.object(recorder.subprocess, "run", return_value=child),
                self.assertRaises(ValueError),
            ):
                recorder.record("ruby", case)

        def fake_cli(*args, **kwargs):
            (kwargs["cwd"] / "report.json").write_bytes(b"{}")
            return SimpleNamespace(returncode=2, stdout=b"", stderr=b"error")

        with (
            patch.object(recorder.subprocess, "run", side_effect=fake_cli),
            self.assertRaises(ValueError),
        ):
            recorder.record("ruby", case)

        def change_only_mtime(*args, **kwargs):
            path = kwargs["cwd"] / "a.rb"
            stat = path.stat()
            os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns + 2_000_000_000))
            return SimpleNamespace(returncode=2, stdout=b"", stderr=b"error")

        with (
            patch.object(recorder.subprocess, "run", side_effect=change_only_mtime),
            self.assertRaises(ValueError),
        ):
            recorder.record("ruby", case)


if __name__ == "__main__":
    unittest.main()
