"""ENV採取の全raw・getter・属性・保存先を独立に壊し、不完全な成功を拒否する。"""

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

import record_target_ruby_environment as recorder
from test_record_cache_parallel_preflight import _dangling_entry, _remove_entry


class TargetRubyEnvironmentContract(unittest.TestCase):
    """候補を使わない採取と、元raw・原本を残す拒否を検査する。"""

    def test_reference_types_api_frames_report_and_scope_are_fixed(self):
        """API値、型、内部frame、source、reportと範囲の改変を拒否する。"""
        value = recorder.template()
        self.assertEqual(len(value["cases"]), 26)
        for mutation in ("api", "type", "frames", "source", "report", "scope"):
            data = copy.deepcopy(value)
            row = data["cases"][0]
            if mutation == "api":
                row["api"]["target_inspect"] = "2.7"
            elif mutation == "type":
                row["exit"] = True
            elif mutation == "frames":
                row["stderr_template"] = row["first_line_template"]
            elif mutation == "source":
                data["source_sha256"] = {}
            elif mutation == "report":
                row["report_hex"] = ""
            else:
                row["comparison"] = "full_stderr"
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                recorder.validate(data)

    def test_entry_guards_precede_platform_and_child_start(self):
        """既存・包含・親なし・danglingを、OS拒否で代用せず検査する。"""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            output = root / "out.json"
            output.write_bytes(b"kept")
            before = output.stat()
            for work, out, message in (
                (root / "work", output, "保存先entry"),
                (root / "same", root / "same/out", "保存先が重な"),
                (root / "same/work", root / "same", "保存先が重な"),
                (root / "work", root / "absent/out", "親directory"),
            ):
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaisesRegex(ValueError, message):
                        recorder.collect(Path("unused"), work, out)
                    child.assert_not_called()
            link = root / "dangling"
            _dangling_entry(link, root / "absent")
            try:
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaisesRegex(ValueError, "保存先entry"):
                        recorder.collect(Path("unused"), root / "work", link)
                    child.assert_not_called()
            finally:
                _remove_entry(link)
            self.assertEqual(output.read_bytes(), b"kept")
            self.assertEqual(output.stat().st_mtime_ns, before.st_mtime_ns)
            self.assertEqual(output.stat().st_mode, before.st_mode)

    def test_original_attribute_changes_keep_raw_then_fail(self):
        """本文だけ・mtimeだけ・modeだけの改変を別々に拒否する。"""
        for mutation in ("bytes", "mtime", "mode"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                file = root / "a.rb"
                file.write_bytes(b"x=1\n")
                info = file.stat()

                def answer(*args, mutation=mutation, file=file, info=info, **kwargs):
                    if mutation == "bytes":
                        file.write_bytes(b"y=2\n")
                        os.utime(file, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif mutation == "mtime":
                        os.utime(file, ns=(info.st_atime_ns, info.st_mtime_ns + 10_000_000_000))
                    else:
                        file.chmod(stat.S_IRUSR)
                    return SimpleNamespace(returncode=2, stdout=b"actual", stderr=b"error")

                try:
                    with patch.object(recorder.subprocess, "run", side_effect=answer):
                        with self.assertRaises(ValueError):
                            recorder._capture(Path("unused"), [], root, {"a.rb": "x=1\n"}, {})
                    raw = json.loads((root / "private-process.json").read_bytes())
                    self.assertEqual(raw["stdout_hex"], b"actual".hex())
                    proof = json.loads((root / "private-run0.json").read_bytes())
                    self.assertNotEqual(proof["before"], proof["after"])
                finally:
                    file.chmod(info.st_mode)

    def test_timeout_and_new_dangling_entry_keep_partial_raw(self):
        """partialを保存し、未知の実link/reparseを辿る前に拒否する。"""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            error = subprocess.TimeoutExpired(["unused"], 60, output=b"partial", stderr=b"error")
            with patch.object(recorder.subprocess, "run", side_effect=error):
                with self.assertRaisesRegex(ValueError, "timeout"):
                    recorder._capture(Path("unused"), [], root, {}, {})
            raw = json.loads((root / "private-process.json").read_bytes())
            self.assertTrue(raw["timed_out"])
            self.assertEqual(raw["stdout_hex"], b"partial".hex())
            root = root / "dangling-capture"
            root.mkdir()
            link = root / "new-entry"

            def answer(*args, **kwargs):
                _dangling_entry(link, root / "absent")
                return SimpleNamespace(returncode=0, stdout=b"actual", stderr=b"")

            try:
                with patch.object(recorder.subprocess, "run", side_effect=answer):
                    with self.assertRaisesRegex(ValueError, "link/reparse"):
                        recorder._capture(Path("unused"), [], root, {}, {})
                raw = json.loads((root / "private-process.json").read_bytes())
                self.assertEqual(raw["stdout_hex"], b"actual".hex())
            finally:
                if os.path.lexists(link):
                    _remove_entry(link)

    def test_empty_environment_value_and_stdin_keep_parent_environment(self):
        """存在する空ENVを子へ渡し、Ruby注入ENVだけ除去しGem/OS環境を保つ。"""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with patch.dict(
                os.environ,
                {"RUBYOPT": "poison", "GEM_PATH": "kept", "RUBOCOP_TARGET_RUBY_VERSION": "poison"},
            ):
                before = dict(os.environ)
                with patch.object(
                    recorder.subprocess,
                    "run",
                    return_value=SimpleNamespace(returncode=2, stdout=b"", stderr=b"error"),
                ) as child:
                    recorder._capture(
                        Path("unused"), [], root, {}, {"RUBOCOP_TARGET_RUBY_VERSION": ""}, b"x=1\n"
                    )
                self.assertEqual(os.environ, before)
                environment = child.call_args.kwargs["env"]
                self.assertNotIn("RUBYOPT", environment)
                self.assertEqual(environment["GEM_PATH"], "kept")
                self.assertEqual(environment["RUBOCOP_TARGET_RUBY_VERSION"], "")
                self.assertEqual(child.call_args.kwargs["input"], b"x=1\n")

    @unittest.skipUnless(os.name == "nt", "Windowsのbasename APIを使う全entry検査")
    def test_complete_entry_checks_api_full_raw_report_and_save_race(self):
        """全二十七childと、API/raw/out/保存raceの独立した破損を検査する。"""
        data = recorder.template()
        for mutation in (None, "api", "raw", "out", "race"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp:
                work, output = Path(temp) / "work", Path(temp) / "out.json"
                roots = {
                    "RUBOCOP_ROOT": (Path(temp) / "rubocop").as_posix(),
                    "AST_ROOT": (Path(temp) / "ast").as_posix(),
                }
                calls = []
                racing = []

                def answer(
                    args,
                    *,
                    cwd,
                    input,
                    mutation=mutation,
                    work=work,
                    output=output,
                    roots=roots,
                    calls=calls,
                    racing=racing,
                    **kwargs,
                ):
                    root = Path(cwd)
                    calls.append(root.name)
                    if root.name == "api-runtime":
                        self.assertEqual(Path(args[1]).name, "reference_api.rb")
                        forms = json.loads(Path(args[2]).read_bytes())
                        self.assertEqual(len(forms), 26)
                        rows = []
                        for row, form in zip(data["cases"], forms, strict=True):
                            self.assertEqual(form["id"], row["id"])
                            self.assertEqual(
                                form["value"], row["environment"]["RUBOCOP_TARGET_RUBY_VERSION"]
                            )
                            bindings = {
                                "CWD": form["root"],
                                "CWD_FWD": form["root"].replace("\\", "/"),
                            }
                            rows.append(
                                {
                                    key: recorder.bind(value, bindings)
                                    for key, value in row["api"].items()
                                }
                            )
                        payload = {
                            "runtime": data["runtime"],
                            "source_sha256": data["source_sha256"],
                            "roots": roots,
                            "rows": rows,
                        }
                        if mutation == "api":
                            payload["rows"][0]["target_inspect"] = "wrong"
                        return SimpleNamespace(
                            returncode=0, stdout=json.dumps(payload).encode(), stderr=b""
                        )
                    row = next(row for row in data["cases"] if row["id"] == root.name)
                    self.assertEqual(args[2:], row["args"])
                    self.assertEqual(input, bytes.fromhex(row["stdin_hex"]))
                    bindings = {
                        "CWD": str(root),
                        "CWD_FWD": root.as_posix(),
                        "LAUNCHER": (work / "reference.rb").as_posix(),
                        **roots,
                    }
                    if mutation == "out" or not row["report_preserved"]:
                        (root / "report.json").write_bytes(
                            b"wrong" if mutation == "out" else bytes.fromhex(row["report_hex"])
                        )
                    if mutation == "race" and row["id"] == data["cases"][-1]["id"]:
                        output.write_bytes(b"racing sentinel")
                        racing.append(output.stat())
                    return SimpleNamespace(
                        returncode=row["exit"],
                        stdout=bytes.fromhex(row["stdout_hex"]),
                        stderr=recorder.bind(row["stderr_template"], bindings).encode()
                        + (b"extra" if mutation == "raw" else b""),
                    )

                with patch.object(recorder.subprocess, "run", side_effect=answer):
                    if mutation is None:
                        self.assertEqual(recorder.collect(Path("unused"), work, output), data)
                        self.assertEqual(len(calls), 27)
                        self.assertEqual(json.loads(output.read_bytes()), data)
                    else:
                        with self.assertRaises((ValueError, FileExistsError)):
                            recorder.collect(Path("unused"), work, output)
                        self.assertTrue(list(work.glob("*/private-process.json")))
                        if mutation == "race":
                            self.assertEqual(output.read_bytes(), b"racing sentinel")
                            self.assertEqual(output.stat().st_mtime_ns, racing[0].st_mtime_ns)
                            self.assertEqual(output.stat().st_mode, racing[0].st_mode)
                        else:
                            self.assertFalse(os.path.lexists(output))


if __name__ == "__main__":
    unittest.main()
