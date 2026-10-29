"""廃止cop採取器の版・完了集合・ENV4・原本不変を有限の負対照で検査する。"""

import copy
import json
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_obsolete_cop_categories as recorder


def fixture():
    """候補を呼ばずに、保存済みの本家API/CLIを単体対照へ渡す。

    Returns:
        dict: 保存済みの実APIと40条件の本家出力。

    """
    return json.loads(
        (
            Path(__file__).resolve().parents[2] / "tests/fixtures/obsolete_cop_categories.json"
        ).read_bytes()
    )


class ObsoleteCopCategoriesTest(unittest.TestCase):
    """入力を保存し、不完全なOracle採取を正常扱いしない。"""

    def test_primary_and_catalogue_require_version_order_and_all_rules(self):
        """16種類だけを順序込みで採用し、誤版・重複・空metadataを拒む。"""
        primary = fixture()["primary"]
        recorder.validate_primary(primary)
        data = recorder.catalogue(primary)
        self.assertEqual(len(data), 16)
        self.assertEqual(
            [row["kind"] for row in data], ["removed"] * 13 + ["split", "extracted", "extracted"]
        )
        for key, value in [
            ("ruby", "wrong"),
            ("gems", {}),
            ("sources", {}),
            ("rules", []),
            ("counts", {}),
            ("counts", {**primary["counts"], "SplitCop": True}),
        ]:
            changed = copy.deepcopy(primary)
            changed[key] = value
            with self.assertRaises(ValueError):
                recorder.validate_primary(changed)
        for field, value in [
            ("index", 52),
            ("old_name", primary["rules"][1]["old_name"]),
            ("warning", True),
            ("warning", 0),
            ("index", 51.0),
        ]:
            changed = copy.deepcopy(primary)
            changed["rules"][0][field] = value
            with self.assertRaises(ValueError):
                recorder.validate_primary(changed)

    def test_scenarios_are_finite_unique_and_cover_every_catalogue_entry(self):
        """本家16種類に加える条件を40へ固定し、通知を生むparallel指定を混ぜない。"""
        primary = fixture()["primary"]
        cases = recorder.cases(primary)
        self.assertEqual(len(cases), 40)
        self.assertEqual(len({case["id"] for case in cases}), 40)
        self.assertTrue(all(case["args"].count("--cache") == 1 for case in cases))
        self.assertTrue(all("--no-parallel" not in case["args"] for case in cases))
        for case, rule in zip(cases[:16], primary["rules"], strict=True):
            self.assertIn(rule["old_name"] + ":\n  Enabled: false\n", case["files"]["config.yml"])

    def test_report_rejects_empty_wrong_duplicate_or_false_completion(self):
        """正常対照はpath・正の完了件数・全summaryが揃ったJSONだけとする。"""
        case = next(case for case in fixture()["cases"] if "report" in case)
        valid = json.loads(bytes.fromhex(case["stdout_hex"]))
        self.assertEqual(recorder.completed_report(json.dumps(valid).encode()), case["report"])
        invalid = []
        for files in [[], valid["files"] * 2, [{"path": "other.rb", "offenses": []}]]:
            changed = copy.deepcopy(valid)
            changed["files"] = files
            invalid.append(changed)
        for key, value in [
            ("target_file_count", 0),
            ("inspected_file_count", 0),
            ("inspected_file_count", True),
            ("target_file_count", True),
            ("offense_count", False),
            ("offense_count", -1),
        ]:
            changed = copy.deepcopy(valid)
            changed["summary"][key] = value
            invalid.append(changed)
        for key, value in [("ruby_engine", "sonicop"), ("rubocop_version", "0")]:
            changed = copy.deepcopy(valid)
            changed["metadata"][key] = value
            invalid.append(changed)
        changed = copy.deepcopy(valid)
        changed["files"][0]["offenses"] = [{"message": "hidden"}]
        changed["summary"]["offense_count"] = 1
        invalid.append(changed)
        for changed in invalid:
            with self.assertRaises(ValueError):
                recorder.completed_report(json.dumps(changed).encode())

    def test_late_error_document_is_preserved_but_never_counted_as_completed(self):
        """後段設定エラーの全JSONを保持し、files空を正常完走へ流用しない。"""
        case = next(case for case in fixture()["cases"] if case["phase"] == "late_rejection")
        raw = bytes.fromhex(case["stdout_hex"])
        self.assertEqual(recorder.late_rejection_report(raw), case["rejection_report"])
        with self.assertRaises(ValueError):
            recorder.completed_report(raw)
        for key, value in [
            ("target_file_count", 0),
            ("target_file_count", True),
            ("inspected_file_count", 1),
            ("offense_count", 1),
        ]:
            changed = json.loads(raw)
            changed["summary"][key] = value
            with self.assertRaises(ValueError):
                recorder.late_rejection_report(json.dumps(changed).encode())

    def test_child_copy_preserves_parent_and_all_other_environment_keys(self):
        """大小差を含むENV4だけを子から除き、GEM/PATH/OSを保持する。"""
        expected = fixture()["cases"][0]
        primary = fixture()["primary"]
        case = recorder.cases(primary)[0]
        child = SimpleNamespace(
            returncode=expected["exit"],
            stdout=bytes.fromhex(expected["stdout_hex"]),
            stderr=bytes.fromhex(expected["stderr_hex"]),
        )
        with patch.dict(
            os.environ,
            {
                "rubyopt": "bad",
                "RuByLiB": "bad",
                "RUBOCOP_OPTS": "bad",
                "RUBOCOP_TARGET_RUBY_VERSION": "bad",
                "KEEP_SENTINEL": "keep",
            },
        ):
            before = dict(os.environ)
            with patch.object(recorder.subprocess, "run", return_value=child) as run:
                self.assertEqual(recorder.record("ruby", case), expected)
            self.assertEqual(dict(os.environ), before)
            self.assertEqual(
                run.call_args.kwargs["env"],
                {
                    **{
                        key: value
                        for key, value in before.items()
                        if (key.upper() if os.name == "nt" else key)
                        not in {"RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"}
                    },
                    **case["env"],
                },
            )
            self.assertEqual(run.call_args.kwargs["env"]["KEEP_SENTINEL"], "keep")

    def test_invalid_exit_stdout_artifact_and_mtime_changes_are_rejected(self):
        """拒否前のformatter生成や、本文が同じでも原本mtimeの変更を見逃さない。"""
        case = recorder.cases(fixture()["primary"])[0]
        for child in [
            SimpleNamespace(returncode=3, stdout=b"", stderr=b"bad"),
            SimpleNamespace(returncode=2, stdout=b"{}", stderr=b"bad"),
            SimpleNamespace(returncode=2, stdout=b"", stderr=b""),
        ]:
            with (
                patch.object(recorder.subprocess, "run", return_value=child),
                self.assertRaises(ValueError),
            ):
                recorder.record("ruby", case)

        row = fixture()["cases"][0]
        for kind in ("out", "mtime", "bytes"):

            def mutation(*args, **kwargs):
                root = kwargs["cwd"]
                path = root / "a.rb"
                status = path.stat()
                if kind == "out":
                    (root / "report.json").write_bytes(b"unexpected")
                elif kind == "bytes":
                    path.write_bytes(b"y=2\n")
                    os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns))
                    self.assertEqual(path.stat().st_mtime_ns, status.st_mtime_ns)
                else:
                    os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns + 2_000_000_000))
                return SimpleNamespace(
                    returncode=row["exit"],
                    stdout=bytes.fromhex(row["stdout_hex"]),
                    stderr=bytes.fromhex(row["stderr_hex"]),
                )

            with patch.object(recorder.subprocess, "run", side_effect=mutation):
                with self.assertRaises(ValueError):
                    recorder.record("ruby", case)

    def test_fixed_selection_coverage_and_original_inputs_are_preserved(self):
        """元40のrawを保持し、40 strictを空集合や重複で置き換えない。"""
        data = fixture()
        primary = data["primary"]
        self.assertEqual(data["schema"], 1)
        self.assertEqual(recorder._fixture(primary, data["cases"]), data)
        self.assertEqual(sum(row["coverage"] == "strict" for row in data["cases"]), 40)
        self.assertEqual(
            [row["id"] for row in data["cases"] if row["phase"] == "late_rejection"],
            ["nested-discovery"],
        )
        for form, row in zip(recorder._selected_inputs(primary), data["cases"], strict=True):
            self.assertEqual(form, {key: row[key] for key in form})
        for only in ([], ["unknown"], ["catalogue-00", "catalogue-00"]):
            with self.assertRaises(ValueError):
                recorder._selected_inputs(primary, only)
        for broken in (
            [],
            data["cases"][:-1],
            list(reversed(data["cases"])),
            data["cases"][:-1] + [data["cases"][0]],
        ):
            with self.assertRaises(ValueError):
                recorder._fixture(primary, broken)
        forms = recorder.cases(primary)
        with patch.object(recorder, "cases", return_value=forms[:-1]):
            with self.assertRaises(ValueError):
                recorder._selected_inputs(primary)

    def test_all_destinations_are_fresh_before_any_child_starts(self):
        """既存entry・dangling・dotdot・Windows case差の同一保存先を守る。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            paths = [root / "work", root / "out.json", root / "rules.json"]
            original_lstat = os.lstat

            def invoke(destinations):
                work, output, rules = destinations
                with patch.object(
                    sys,
                    "argv",
                    [
                        "record",
                        "--ruby",
                        "ruby",
                        "--work-dir",
                        str(work),
                        "--output",
                        str(output),
                        "--rules-output",
                        str(rules),
                    ],
                ):
                    recorder.main()

            for target in paths:

                def _lstat(path, *args, **kwargs):
                    if Path(path) == target:
                        return SimpleNamespace(st_mode=stat.S_IFLNK)
                    return original_lstat(path, *args, **kwargs)

                with patch("os.lstat", side_effect=_lstat):
                    with self.assertRaises(ValueError):
                        recorder._fresh_paths(*paths)
                target.write_bytes(b"KEEP")
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaises(ValueError):
                        invoke(paths)
                    child.assert_not_called()
                self.assertEqual(target.read_bytes(), b"KEEP")
                target.unlink()
            aliases = [
                (root / "absent" / ".." / "same", root / "same", paths[2]),
                (paths[0], paths[1], paths[1]),
            ]
            if os.name == "nt":
                aliases.append((root / "Same", root / "same", paths[2]))
            for destinations in aliases:
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaises(ValueError):
                        invoke(destinations)
                    child.assert_not_called()


if __name__ == "__main__":
    unittest.main()
