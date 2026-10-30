"""対象版参照の型・phase・全rawと、全採取後だけの保存を検査する。"""

import copy
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_target_ruby_validation as recorder


class TargetRubyValidationContract(unittest.TestCase):
    """既存の共通I/O契約に加え、このcollectorの全件照合を検査する。"""

    def test_inputs_types_frames_reports_api_and_scope_are_fixed(self):
        """全rawや比較範囲の削減をcanonical照合で拒否する。"""
        value = recorder.template()
        self.assertEqual(len(value["cases"]), 51)
        self.assertEqual(len(value["api"]), 9)
        for field in (
            "exit",
            "stdout_hex",
            "stderr_template",
            "stderr_header_template",
            "comparison",
            "files",
        ):
            data = copy.deepcopy(value)
            data["cases"][0][field] = None
            with self.subTest(field=field), self.assertRaises(ValueError):
                recorder.validate(data)
        for field in ("api", "source_sha256", "scope"):
            data = copy.deepcopy(value)
            data[field] = {}
            with self.subTest(field=field), self.assertRaises(ValueError):
                recorder.validate(data)

    def test_fresh_entry_checks_precede_platform_or_children(self):
        """既存entryをOS制限や実Ruby起動より先に拒否する。"""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            output = root / "out.json"
            output.write_bytes(b"kept")
            before = output.stat()
            with patch.object(recorder, "_capture") as child:
                with self.assertRaises(ValueError):
                    recorder.collect(Path("unused"), root / "work", output)
                child.assert_not_called()
            self.assertEqual(output.read_bytes(), b"kept")
            self.assertEqual(output.stat().st_mtime_ns, before.st_mtime_ns)

    def test_all_processes_raw_and_metadata_must_match_before_save(self):
        """最後のCLI、API値、runtime、全frameの不一致でも出力を残さない。"""
        data = recorder.template()
        for fault in (None, "last-cli", "api", "runtime", "frames"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                work, output = root / "work", root / "out.json"
                calls = []

                def capture(
                    ruby, args, cwd, files, environment, stdin=b"", *, calls=calls, fault=fault
                ):
                    calls.append((args, cwd, files, environment, stdin))
                    if len(calls) == 1:
                        rows = [
                            {k: v for k, v in row.items() if k != "scalar_yaml"}
                            for row in data["api"]
                        ]
                        observed = {
                            "runtime": copy.deepcopy(data["runtime"]),
                            "source_sha256": data["source_sha256"],
                            "roots": {"RUBOCOP_ROOT": "rubocop-root", "AST_ROOT": "ast-root"},
                            "rows": rows,
                        }
                        if fault == "api":
                            rows[0]["getter_inspect"] = "2.7"
                        if fault == "runtime":
                            observed["runtime"]["ruby"] = "0.0.0"
                        return (
                            SimpleNamespace(
                                returncode=0, stdout=json.dumps(observed).encode(), stderr=b""
                            ),
                            {},
                            {},
                        )
                    row = data["cases"][len(calls) - 2]
                    bindings = {
                        "CWD": str(cwd),
                        "CWD_FWD": str(cwd).replace("\\", "/"),
                        "RUBOCOP_ROOT": "rubocop-root",
                        "AST_ROOT": "ast-root",
                    }
                    stderr = recorder.bind(row["stderr_template"], bindings).encode()
                    if fault == "frames" and row["comparison"] == "parser_header":
                        stderr = recorder.bind(row["stderr_header_template"], bindings).encode()
                    stdout = bytes.fromhex(row["stdout_hex"])
                    if fault == "last-cli" and len(calls) == 52:
                        stdout += b"partial"
                    report = (
                        {"report.json": {"bytes_hex": "4b454550"}} if row["report_present"] else {}
                    )
                    return (
                        SimpleNamespace(returncode=row["exit"], stdout=stdout, stderr=stderr),
                        report,
                        report,
                    )

                with (
                    patch.object(recorder, "os", SimpleNamespace(name="nt")),
                    patch.object(recorder, "_windows_name", side_effect=str),
                    patch.object(recorder, "_capture", side_effect=capture),
                ):
                    if fault is None:
                        self.assertEqual(recorder.collect(Path("ruby-only"), work, output), data)
                        self.assertEqual(len(calls), 52)
                        self.assertEqual(json.loads(output.read_bytes()), data)
                        self.assertEqual(calls[-1][0][:2], ["-e", recorder.CLI])
                    else:
                        with self.assertRaises(ValueError):
                            recorder.collect(Path("ruby-only"), work, output)
                        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
