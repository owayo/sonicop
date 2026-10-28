"""Windows originの採取範囲・投影・新規出力の契約を検査する。"""

import copy
import json
import os
import stat
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_config_namespace_origins as recorder


class OriginRecorderContract(unittest.TestCase):
    """別表記を同一視した期待や欠けた行列を記録へ混ぜない。"""

    def test_fixture_has_every_measured_condition(self):
        """空や重複の条件を24成功と数えない。"""
        fixture = json.loads(recorder.TEMPLATE.read_text(encoding="utf-8"))
        recorder._validate_template(fixture)
        ids = [case["id"] for group in fixture["groups"] for case in group["cases"]]
        self.assertEqual(len(ids), 24)
        self.assertEqual(len(set(ids)), 24)

    def test_incomplete_unknown_or_reordered_conditions_fail(self):
        """件数だけの一致で別の条件を埋め合わせさせない。"""
        fixture = json.loads(recorder.TEMPLATE.read_text(encoding="utf-8"))
        mutations = [
            lambda value: value.update(schema=2),
            lambda value: value.update(groups=[]),
            lambda value: value["groups"][0].update(cases=[]),
            lambda value: value["groups"][0]["cases"][0].update(id="unknown"),
            lambda value: value["groups"][0]["cases"].reverse(),
            lambda value: value["groups"][0]["cases"].append(value["groups"][0]["cases"][0]),
        ]
        for mutate in mutations:
            broken = copy.deepcopy(fixture)
            mutate(broken)
            with self.assertRaises(ValueError):
                recorder._validate_template(broken)

    def test_projection_changes_only_explicit_root_tokens(self):
        """警告本文の似た文字列や任意の絶対pathを消さない。"""
        value = {"text": "<short>/base.yml: keep C:/other/file.yml", "items": ["<long>/a.rb"]}
        actual = recorder._transform(value, {"<short>": "C:/SHORT~1", "<long>": "C:/long name"})
        self.assertEqual(actual["text"], "C:/SHORT~1/base.yml: keep C:/other/file.yml")
        self.assertEqual(actual["items"], ["C:/long name/a.rb"])

    def test_relative_targets_use_string_spelling_without_realpath(self):
        """同一fileへ到達する別aliasでも、本家の絶対表示を保つ。"""
        self.assertEqual(recorder._target("C:/SHORT~1", "nested/a.rb"), "nested/a.rb")
        self.assertEqual(recorder._target("C:/SHORT~1", "C:/long name/a.rb"), "C:/long name/a.rb")

    def test_hybrid_root_retains_short_parents(self):
        """末尾だけ展開されたoriginをcanonicalなlong全体へ置き換えない。"""
        with patch.object(
            recorder,
            "_windows_spelling",
            side_effect=["C:/long parent/long child", "C:/LONGPA~1/LONGCH~1"],
        ):
            self.assertEqual(recorder._roots(Path("unused"))["<hybrid>"], "C:/LONGPA~1/long child")
        with patch.object(recorder, "_windows_spelling", return_value="C:/same"):
            with self.assertRaises(ValueError):
                recorder._roots(Path("unused"))

    def test_existing_and_dangling_destinations_are_rejected_before_collection(self):
        """Win32 symlink作成権限によらず、lstatで見える既存entryを守る。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            original_lstat = os.lstat
            for index in range(2):
                destinations = [root / f"{index}-work", root / f"{index}-out"]

                def _lstat(path, *args, **kwargs):
                    if Path(path) == destinations[index]:
                        return SimpleNamespace(st_mode=stat.S_IFLNK)
                    return original_lstat(path, *args, **kwargs)

                args = [
                    "recorder",
                    "--ruby",
                    "unused",
                    "--work",
                    str(destinations[0]),
                    "--out",
                    str(destinations[1]),
                ]
                with (
                    patch("sys.argv", args),
                    patch("os.lstat", side_effect=_lstat),
                    patch.object(recorder, "_collect") as collect,
                ):
                    self.assertFalse(destinations[index].exists())
                    with self.assertRaises(ValueError):
                        recorder.main()
                    collect.assert_not_called()
                destinations[index].write_bytes(b"keep")
                with patch("sys.argv", args), patch.object(recorder, "_collect") as collect:
                    with self.assertRaises(ValueError):
                        recorder.main()
                    collect.assert_not_called()
                self.assertEqual(destinations[index].read_bytes(), b"keep")


if __name__ == "__main__":
    unittest.main()
