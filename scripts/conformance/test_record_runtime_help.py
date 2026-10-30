"""本家helpの全raw・保存先・runtime・原本の改変を採取器が拒否する契約。"""

import copy
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import record_runtime_help as subject
import record_target_ruby_environment as capture


class RuntimeHelpTests(unittest.TestCase):
    """参照の完全性と、実子process以外を使う保存契約を検査する。"""

    def test_reference_tampering_is_rejected_without_assertions(self):
        """型・出典・順序・raw・比較範囲の改変を通常と-Oで拒否する。"""
        original = subject.template()
        for mutate in (
            lambda value: value.update(scope="partial"),
            lambda value: value["cases"].reverse(),
            lambda value: value["cases"][0].update(exit=False),
            lambda value: value["cases"][0].update(stdout_prefix_hex=""),
            lambda value: value["cases"][0].update(stderr_template="hidden"),
            lambda value: value.update(reference_help_hex=""),
            lambda value: value["primary"]["source_sha256"].update({"options.rb": "changed"}),
        ):
            value = copy.deepcopy(original)
            mutate(value)
            with self.assertRaises(ValueError):
                subject.validate(value)

    def test_freshness_precedes_platform_and_processes(self):
        """既存entry・包含関係・親なしを子起動より前に拒否する。"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            held = root / "held"
            held.write_bytes(b"KEEP")
            for work, output in (
                (held, root / "out"),
                (root / "work", held),
                (root / "work", root / "work/out"),
                (root / "out/work", root / "out"),
                (root / "work", root / "missing/out"),
            ):
                with (
                    self.subTest(work=work, output=output),
                    patch.object(subject, "_capture") as run,
                ):
                    with self.assertRaises(ValueError):
                        subject.collect(Path("ruby"), work, output)
                    run.assert_not_called()
                self.assertEqual(held.read_bytes(), b"KEEP")

    def fake_process(self, value, output, fault):
        """全二十六起動を再現し、改変は実際の原本と排他的保存へ作用させる。

        Returns:
            tuple: 起動記録とsubprocess.runの代替。

        """
        calls = []
        rows = {row["id"]: row for row in value["cases"]}

        def run(args, **options):
            root = options["cwd"]
            calls.append(root.name)
            self.assertEqual(args[0], "ruby")
            self.assertEqual(options["input"], b"")
            environment = options["env"]
            for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_TARGET_RUBY_VERSION", "NO_COLOR"):
                self.assertNotIn(key, environment)
            self.assertEqual(environment["TERM"], "dumb")
            if root.name == "runtime":
                self.assertEqual((root / "reference.rb").read_text(encoding="utf8"), subject.API)
                response = copy.deepcopy(value["primary"])
                response["roots"] = {"RUBOCOP_ROOT": "gem", "RUBY_LIB": "library"}
                if fault == "runtime":
                    response["runtime"]["ruby"] = "different"
                return subprocess.CompletedProcess(args, 0, json.dumps(response).encode(), b"")
            row = rows[root.name]
            self.assertEqual(args[1:], ["-e", subject.CLI, "--", *row["args"]])
            for key, expected in row["environment"].items():
                self.assertEqual(environment[key], expected)
            if "RUBOCOP_OPTS" not in row["environment"]:
                self.assertNotIn("RUBOCOP_OPTS", environment)
            errors = (
                row["stderr_template"]
                .replace("<RUBOCOP_ROOT>", "gem")
                .replace("<RUBY_LIB>", "library")
                .encode()
            )
            if fault in ("bytes", "mtime", "mode"):
                original = root / "first.rb"
                if fault == "bytes":
                    original.write_bytes(b"CHANGED")
                elif fault == "mtime":
                    info = original.stat()
                    os.utime(original, ns=(info.st_atime_ns, info.st_mtime_ns + 1_000_000_000))
                else:
                    original.chmod(original.stat().st_mode & ~0o222)
            if fault == "timeout":
                raise subprocess.TimeoutExpired(
                    args, 60, output=b"partial", stderr=b"partial-errors"
                )
            if root.name == value["cases"][-1]["id"] and fault == "race":
                output.write_bytes(b"RACED")
            return subprocess.CompletedProcess(
                args,
                row["exit"],
                subject.stdout(value, row),
                b"wrong" if fault == "stderr" else errors,
            )

        return calls, run

    @unittest.skipUnless(os.name == "nt", "固定Windows参照のentry契約")
    def test_whole_entry_rejects_runtime_streams_originals_and_timeout(self):
        """全件成功だけを保存し、途中の失敗とtimeoutのpartialを保持する。"""
        value = subject.template()
        for fault in (None, "runtime", "stderr", "bytes", "mtime", "mode", "timeout"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                output, work = root / "out.json", root / "work"
                calls, run = self.fake_process(value, output, fault)
                with (
                    patch.object(capture.subprocess, "run", side_effect=run),
                    patch.dict(
                        os.environ,
                        {
                            key: "ambient"
                            for key in (
                                "RUBYOPT",
                                "RUBYLIB",
                                "RUBOCOP_OPTS",
                                "RUBOCOP_TARGET_RUBY_VERSION",
                                "NO_COLOR",
                            )
                        },
                    ),
                ):
                    if fault is None:
                        self.assertEqual(subject.collect(Path("ruby"), work, output), value)
                        self.assertEqual(json.loads(output.read_bytes()), value)
                        self.assertEqual(len(calls), 26)
                    else:
                        with self.assertRaises(ValueError):
                            subject.collect(Path("ruby"), work, output)
                        self.assertFalse(output.exists())
                if fault == "timeout":
                    raw = json.loads(
                        (work / value["cases"][0]["id"] / "private-process.json").read_bytes()
                    )
                    self.assertTrue(raw["timed_out"])
                    self.assertEqual(bytes.fromhex(raw["stdout_hex"]), b"partial")
                    self.assertEqual(bytes.fromhex(raw["stderr_hex"]), b"partial-errors")

    @unittest.skipUnless(os.name == "nt", "固定Windows参照のentry契約")
    def test_exclusive_output_keeps_a_racing_file(self):
        """検証後に現れた出力を上書きしない。"""
        value = subject.template()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "out.json"
            calls, run = self.fake_process(value, output, "race")
            with patch.object(capture.subprocess, "run", side_effect=run):
                with self.assertRaises(FileExistsError):
                    subject.collect(Path("ruby"), root / "work", output)
            self.assertEqual(output.read_bytes(), b"RACED")
            self.assertEqual(len(calls), 26)


if __name__ == "__main__":
    unittest.main()
