"""順序 collector が偽成功・不完全な実行を拒否する陰性回帰。"""

import copy
import json
import os
import pathlib
import tempfile
import types
import unittest
from unittest.mock import patch

import record_fail_fast_order as recorder


def _case():
    return recorder.scenarios()[0]


def _report():
    return {
        "metadata": {"rubocop_version": "1.89.0"},
        "files": [{"path": "a.rb", "offenses": [{"cop_name": "Layout/SpaceAroundOperators"}]}],
        "summary": {"offense_count": 1, "target_file_count": 2, "inspected_file_count": 1},
    }


class RecordingGuards(unittest.TestCase):
    """候補の出力を使わず、完了検査自体の失敗条件を固定する。"""

    def test_child_environment_removes_only_the_four_oracle_injections(self):
        """親ENVを変えず、GEM/PATH/SystemRootを保った子のENVだけを作る。"""
        poison = {
            key: "poison"
            for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
        }
        kept = {key: f"keep-{key}" for key in ("GEM_HOME", "GEM_PATH", "PATH", "SYSTEMROOT")}
        with patch.dict(os.environ, poison | kept):
            before = os.environ.copy()
            with patch.object(recorder.subprocess, "run") as run:
                recorder._run(["ruby"], pathlib.Path("."))
            child = run.call_args.kwargs["env"]
            self.assertTrue(all(key not in child for key in poison))
            self.assertTrue(all(child[key] == value for key, value in kept.items()))
            self.assertEqual(os.environ.copy(), before)

    def test_positive_listing_and_report(self):
        """構成した全pathと正のsummaryが揃う場合だけ受理する。"""
        case = _case()
        self.assertEqual(
            recorder.validate_listing(0, b"a.rb\r\nb.rb\r\n", b"", case), ["a.rb", "b.rb"]
        )
        report, after = recorder.validate_report(
            1, json.dumps(_report()).encode(), b"", case, ["a.rb", "b.rb"]
        )
        self.assertEqual(report, _report())
        self.assertEqual(after, "")

    def test_listing_rejects_empty_missing_extra_and_duplicates(self):
        """target集合を件数だけで成功にしない。"""
        for output in (b"", b"a.rb\n", b"a.rb\nb.rb\nc.rb\n", b"a.rb\nb.rb\na.rb\n"):
            with self.subTest(output=output), self.assertRaises(ValueError):
                recorder.validate_listing(0, output, b"", _case())

    def test_listing_rejects_exit_and_stderr(self):
        """異常終了やstderr付きの部分出力を使わない。"""
        for status, stderr in ((1, b""), (2, b""), (0, b"warning")):
            with self.subTest(status=status, stderr=stderr), self.assertRaises(ValueError):
                recorder.validate_listing(status, b"a.rb\nb.rb\n", stderr, _case())

    def test_report_rejects_zero_inspections(self):
        """files空・target0・exit0という偽CLI成功を拒否する。"""
        report = {
            "files": [],
            "summary": {"offense_count": 0, "target_file_count": 0, "inspected_file_count": 0},
        }
        with self.assertRaises(ValueError):
            recorder.validate_report(0, json.dumps(report).encode(), b"", _case(), ["a.rb", "b.rb"])

    def test_report_rejects_wrong_order_and_duplicates(self):
        """別targetの同じ診断や二重記録を成功に数えない。"""
        for rows in ([{"path": "b.rb", "offenses": [1]}], _report()["files"] * 2):
            report = _report() | {"files": rows}
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                recorder.validate_report(
                    1, json.dumps(report).encode(), b"", _case(), ["a.rb", "b.rb"]
                )

    def test_report_rejects_inconsistent_summaries(self):
        """offense/target/inspectedの各fieldを独立に検査する。"""
        for field in ("offense_count", "target_file_count", "inspected_file_count"):
            report = copy.deepcopy(_report())
            report["summary"][field] += 1
            with self.subTest(field=field), self.assertRaises(ValueError):
                recorder.validate_report(
                    1, json.dumps(report).encode(), b"", _case(), ["a.rb", "b.rb"]
                )

    def test_report_rejects_exit_stderr_and_trailing_output(self):
        """JSONが読めても内部異常や余分な出力を許可しない。"""
        for status, stderr, tail in ((2, b"", b""), (1, b"error", b""), (1, b"", b"{}")):
            with self.subTest(status=status, tail=tail), self.assertRaises(ValueError):
                recorder.validate_report(
                    status, json.dumps(_report()).encode() + tail, stderr, _case(), ["a.rb", "b.rb"]
                )

    def test_sort_rejects_incomplete_recordings(self):
        """720組という正の期待集合を満たさないsort採取を拒否する。"""
        for status, data in ((1, b"[]"), (0, b"[]"), (0, b"[{}]")):
            result = types.SimpleNamespace(returncode=status, stdout=data, stderr=b"")
            with (
                self.subTest(status=status, data=data),
                patch.object(recorder, "_run", return_value=result),
                self.assertRaises(ValueError),
            ):
                recorder._sort_records(
                    "ruby", pathlib.Path("reference.rb"), pathlib.Path("."), recorder.RUNTIME
                )

    def test_runtime_rejects_wrong_version(self):
        """Rubyまたはgemの版が不一致なら最初に止める。"""
        result = types.SimpleNamespace(
            returncode=0,
            stdout=json.dumps(recorder.RUNTIME | {"ruby": "4.0.8", "long_bits": 32}).encode(),
            stderr=b"",
        )
        with patch.object(recorder, "_run", return_value=result), self.assertRaises(ValueError):
            recorder._runtime("ruby", pathlib.Path("reference.rb"), pathlib.Path("."))

    def test_sort_rejects_numerically_unsorted_full_permutation(self):
        """件数とpermutationが揃っていても、数値順が壊れた採取を拒否する。"""

        def response(_command, _cwd, data):
            inputs = json.loads(data)
            rows = [
                {
                    "keys": keys,
                    "indices": sorted(range(len(keys)), key=keys.__getitem__),
                    "uniform": True,
                }
                for keys in inputs
            ]
            broken = next(row for row in rows if len(set(row["keys"])) > 1)
            broken["indices"].reverse()
            return types.SimpleNamespace(returncode=0, stdout=json.dumps(rows).encode(), stderr=b"")

        with patch.object(recorder, "_run", side_effect=response), self.assertRaises(ValueError):
            recorder._sort_records(
                "ruby", pathlib.Path("reference.rb"), pathlib.Path("."), recorder.RUNTIME
            )

    def test_unknown_names_do_not_succeed_with_empty_cases(self):
        """誤ったselectorで全件を除外しても正常完了にしない。"""
        with (
            tempfile.TemporaryDirectory() as temporary,
            patch.object(recorder, "_runtime", return_value=recorder.RUNTIME),
            self.assertRaises(ValueError),
        ):
            recorder.collect(
                "ruby", pathlib.Path(temporary), names=["missing"], collect_sorts=False
            )


if __name__ == "__main__":
    unittest.main()
