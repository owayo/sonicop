"""設定cache参照採取の全完了と、保持・cache観測の独立した失敗を検査する。"""

import copy
import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_configured_result_cache as recorder


def _answer(mutate=None):
    counts = {}
    data = recorder._template()
    rows = {row["id"]: row for row in data["cases"]}

    def child(argv, **kwargs):
        root = Path(kwargs["cwd"])
        if argv[2] == recorder.API:
            return SimpleNamespace(
                returncode=0, stdout=json.dumps(data["primary"]).encode(), stderr=b""
            )
        repeat = counts.get(root.name, 0)
        counts[root.name] = repeat + 1
        expected = rows[root.name]["results"][repeat]
        if expected["cache_exists"]:
            (root / "cache").mkdir(exist_ok=True)
            (root / "cache/data").write_bytes(b"cache")
        output = SimpleNamespace(
            returncode=expected["exit"],
            stdout=bytes.fromhex(expected["stdout_hex"]),
            stderr=bytes.fromhex(expected["stderr_hex"]),
        )
        if mutate is not None:
            mutate(root, repeat, output)
        return output

    return child


class ConfiguredCacheRecorderContract(unittest.TestCase):
    """正しい全応答へ一つの変化だけを加え、未完了の公開JSONを作らせない。"""

    def test_fixed_inputs_types_sources_raw_and_scope(self):
        """21条件の順序・型・全raw・保持・scope変更を拒否する。"""
        original = recorder._template()
        recorder.validate_fixture(original)
        for mutation in (
            "empty",
            "order",
            "schema",
            "primary",
            "files",
            "raw",
            "exit",
            "cache",
            "held",
            "scope",
        ):
            value = copy.deepcopy(original)
            if mutation == "empty":
                value["cases"] = []
            elif mutation == "order":
                value["cases"].reverse()
            elif mutation == "schema":
                value["schema"] = True
            elif mutation == "primary":
                value["primary"]["source_sha256"]["lib/rubocop/runner.rb"] = "0" * 64
            elif mutation == "files":
                value["cases"][0]["files"]["good.rb"] = "changed\n"
            elif mutation == "scope":
                value["coverage"]["subset"] = True
            else:
                result = value["cases"][0]["results"][0]
                key, replacement = {
                    "raw": ("stdout_hex", "00"),
                    "exit": ("exit", True),
                    "cache": ("cache_exists", 0),
                    "held": ("source_config_bytes_mtime_mode_equal", 1),
                }[mutation]
                result[key] = replacement
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                recorder.validate_fixture(value)

    def test_complete_mock_requires_primary_and_all_forty_two_cli_runs(self):
        """模擬43応答と原本184観測が揃った場合だけ、元JSONを保存する。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(recorder.subprocess, "run", side_effect=_answer()) as child:
                value = recorder.collect("not-started-ruby", root / "work", root / "out.json")
            self.assertEqual(child.call_count, 43)
            self.assertEqual(value, recorder._template())
            self.assertEqual((root / "out.json").read_bytes(), recorder.FIXTURE.read_bytes())
            journals = list((root / "work").glob("*/private-run-*.json"))
            self.assertEqual(len(journals), 43)
            observed = 0
            for path in journals:
                journal = json.loads(path.read_bytes())
                self.assertEqual(journal["inputs_before"], journal["inputs_after"])
                self.assertIsNone(journal["report_after"])
                observed += len(journal["inputs_before"])
            self.assertEqual(observed, 184)

    def test_environment_isolated_without_changing_parent(self):
        """注入変数とambient cache指定だけを子copyから除去する。"""
        values = {
            "RUBYOPT": "ambient",
            "RUBYLIB": "ambient",
            "RUBOCOP_OPTS": "ambient",
            "RUBOCOP_TARGET_RUBY_VERSION": "ambient",
            "RUBOCOP_CACHE_ROOT": "ambient",
            "NO_COLOR": "1",
            "SONICOP_PROFILE": "1",
            "GEM_HOME": "keep",
            "PATH": "keep",
        }
        with patch.dict(os.environ, values):
            child = recorder._environment(Path("case"))
            self.assertEqual(os.environ["RUBOCOP_CACHE_ROOT"], "ambient")
        for key in values.keys() - {"GEM_HOME", "PATH"}:
            self.assertNotIn(key, child)
        self.assertEqual(child["GEM_HOME"], "keep")
        self.assertEqual(child["PATH"], "keep")
        self.assertEqual(child["XDG_CACHE_HOME"], str(Path("case/cache")))

    def test_fresh_entry_and_overlapping_destinations_reject_before_child(self):
        """既存file・directoryと正規化後の重複を子起動前に拒否する。"""
        for kind in ("file", "directory", "same", "nested"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                work, output = root / "work", root / "out.json"
                if kind == "file":
                    work.write_bytes(b"KEEP")
                elif kind == "directory":
                    output.mkdir()
                elif kind == "same":
                    output = work
                else:
                    work = output / "nested"
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaises(ValueError):
                        recorder.collect("not-started-ruby", work, output)
                    child.assert_not_called()
                if kind == "file":
                    self.assertEqual(work.read_bytes(), b"KEEP")

    @unittest.skipUnless(os.name == "nt", "実NTFS junctionの負対照はWindowsで実行")
    def test_dangling_junction_work_rejects_before_child(self):
        """参照先のない実junctionも保存先entryとして拒否する。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target, work = root / "target", root / "work"
            target.mkdir()
            subprocess.run(
                ["cmd.exe", "/d", "/c", "mklink", "/J", str(work), str(target)],
                capture_output=True,
                check=True,
            )
            target.rmdir()
            self.assertTrue(os.path.lexists(work))
            self.assertFalse(work.exists())
            with patch.object(recorder.subprocess, "run") as child:
                with self.assertRaises(ValueError):
                    recorder.collect("not-started-ruby", work, root / "out.json")
                child.assert_not_called()
            self.assertTrue(os.path.lexists(work))

    def test_primary_and_cli_failures_never_publish_partial_output(self):
        """原sourceの相違と、途中のexit/stderr/未完了reportを成功にしない。"""
        for kind in ("primary", "exit", "stderr", "truncated"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                answer = _answer()

                def child(argv, kind=kind, answer=answer, **kwargs):
                    output = answer(argv, **kwargs)
                    if kind == "primary" and argv[2] == recorder.API:
                        value = json.loads(output.stdout)
                        value["runtime"]["ruby"] = "unexpected"
                        output.stdout = json.dumps(value).encode()
                    elif Path(kwargs["cwd"]).name == "false-cli-default":
                        if kind == "exit":
                            output.returncode = 2
                        elif kind == "stderr":
                            output.stderr = b"unexpected"
                        elif kind == "truncated":
                            output.stdout = output.stdout[:-1]
                    return output

                with patch.object(recorder.subprocess, "run", side_effect=child):
                    with self.assertRaises(ValueError):
                        recorder.collect("not-started-ruby", root / "work", root / "out.json")
                self.assertFalse(os.path.lexists(root / "out.json"))

    def test_source_and_cache_guards_fail_independently_of_correct_raw(self):
        """全rawを保ち、bytes・mtime・modeとcache生成だけの変化を個別に拒否する。"""
        for kind in ("bytes", "mtime", "mode", "cache"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)

                def mutate(case, repeat, output, kind=kind):
                    if case.name != "false-cli-default" or repeat:
                        return
                    path = case / "good.rb"
                    info = path.stat()
                    if kind == "bytes":
                        path.write_bytes(b"x=2\n")
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif kind == "mtime":
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns + 1_000_000_000))
                    elif kind == "mode":
                        path.chmod(info.st_mode ^ stat.S_IWUSR)
                    else:
                        (case / "cache").mkdir()
                        (case / "cache/data").write_bytes(b"unexpected")

                with patch.object(recorder.subprocess, "run", side_effect=_answer(mutate)):
                    with self.assertRaises(ValueError):
                        recorder.collect("not-started-ruby", root / "work", root / "out.json")
                self.assertFalse(os.path.lexists(root / "out.json"))
                journal = json.loads(
                    (root / "work/false-cli-default/private-run-0.json").read_bytes()
                )
                expected = recorder._template()["cases"][0]["results"][0]
                self.assertEqual(journal["stdout_hex"], expected["stdout_hex"])
                if kind == "cache":
                    self.assertEqual(journal["inputs_before"], journal["inputs_after"])
                    self.assertTrue(journal["cache_after"]["exists"])
                else:
                    self.assertNotEqual(journal["inputs_before"], journal["inputs_after"])

    def test_new_report_entries_keep_evidence_and_reject(self):
        """file・directory・dangling symlinkの本文を読まず、生成を拒否する。"""
        for kind in ("file", "directory", "symlink"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)

                def mutate(case, repeat, output, kind=kind):
                    if case.name != "false-cli-default" or repeat:
                        return
                    path = case / "report.json"
                    if kind == "file":
                        path.write_bytes(b"unexpected")
                    elif kind == "directory":
                        path.mkdir()
                    else:
                        try:
                            path.symlink_to(case / "missing")
                        except OSError as error:
                            self.skipTest(f"symlinkを作成できません: {error}")

                with patch.object(recorder.subprocess, "run", side_effect=_answer(mutate)):
                    with self.assertRaises(ValueError):
                        recorder.collect("not-started-ruby", root / "work", root / "out.json")
                self.assertFalse(os.path.lexists(root / "out.json"))
                journal = json.loads(
                    (root / "work/false-cli-default/private-run-0.json").read_bytes()
                )
                self.assertEqual(journal["inputs_before"], journal["inputs_after"])
                self.assertIsNotNone(journal["report_after"])


if __name__ == "__main__":
    unittest.main()
