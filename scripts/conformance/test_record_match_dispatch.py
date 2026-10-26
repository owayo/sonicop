"""本家recorderが対象未検査や候補VMを成功扱いしないことを確認する。"""

import copy
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_match_dispatch as recorder


class RecorderContract(unittest.TestCase):
    """実本家のinspect記録を境界の正常入力に使う。"""

    def setUp(self):
        """候補の現在値を期待へ混ぜず、本家記録を正常系の基準にする。"""
        path = Path(__file__).resolve().parents[2] / "tests/fixtures/match_with_lvasgn.json"
        self.fixture = json.loads(path.read_bytes())
        self.group = self.fixture["groups"][0]
        self.report = copy.deepcopy(self.group["report"])
        self.report["metadata"] = {
            "rubocop_version": "1.89.0",
            "ruby_engine": "ruby",
            "ruby_version": "4.0.7",
            "ruby_platform": "x64-mingw-ucrt",
        }

    def _record(self, *, code=None):
        with tempfile.TemporaryDirectory(prefix="sonicop-match-recorder-") as directory:
            args = SimpleNamespace(work=directory, ruby="ruby", launcher="reference.rb")
            completed = SimpleNamespace(
                returncode=self.group["exit"] if code is None else code,
                stderr=b"",
                stdout=json.dumps(self.report).encode(),
            )
            with patch.object(recorder.subprocess, "run", return_value=completed) as execute:
                recorded = recorder._record_group((0, self.group), args=args, fixture=self.fixture)
                self.last_invocation = execute.call_args
                return recorded

    def test_recorded_inspection_round_trips_without_using_stored_outcomes(self):
        """保存済みの結果を書き戻すだけの再生成を成功扱いしない。"""
        self.assertEqual(self._record(), self.group)

    def test_ambient_options_do_not_change_the_recorded_reference_conditions(self):
        """起動元の注釈や require を除き、固定 gem と Ruby の所在を保つ。"""
        ambient = {
            "RUBYOPT": "-rambient_only",
            "RUBYLIB": "ambient-libraries",
            "RUBOCOP_OPTS": "--display-cop-names",
            "RUBOCOP_TARGET_RUBY_VERSION": "4.1",
            "GEM_HOME": "pinned-gems",
            "GEM_PATH": "pinned-gem-path",
            "PATH": "pinned-ruby-bin",
            "SYSTEMROOT": "platform-systemroot",
        }
        with patch.dict(recorder.os.environ, ambient):
            self.assertEqual(self._record(), self.group)
            child_environment = self.last_invocation.kwargs["env"]
            for name in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
                self.assertNotIn(name, child_environment)
                self.assertEqual(recorder.os.environ[name], ambient[name])
            for name in ("GEM_HOME", "GEM_PATH", "PATH", "SYSTEMROOT"):
                self.assertEqual(child_environment[name], ambient[name])

    def test_cli_failure_is_rejected_even_with_valid_json(self):
        """本家が途中で失敗した JSON を完了した参照へ採用しない。"""
        with self.assertRaises(ValueError):
            self._record(code=2)

    def test_a_different_target_is_rejected(self):
        """予定外のファイルの検査を、入力を測った証拠へ混ぜない。"""
        self.report["files"][0]["path"] = "unrelated.rb"
        with self.assertRaises(ValueError):
            self._record()

    def test_zero_inspected_files_is_rejected(self):
        """対象があるのに未検査の出力を期待値として固定しない。"""
        self.report["summary"]["inspected_file_count"] = 0
        with self.assertRaises(ValueError):
            self._record()

    def test_candidate_metadata_cannot_be_recorded_as_the_reference(self):
        """候補の診断を本家の仕様として固定する取り違えを拒む。"""
        self.report["metadata"]["ruby_engine"] = "sonicop"
        with self.assertRaises(ValueError):
            self._record()


if __name__ == "__main__":
    unittest.main()
