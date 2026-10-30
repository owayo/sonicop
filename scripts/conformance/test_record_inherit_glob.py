"""glob参照の欠落と最後の実process不一致で保存を拒否する契約。"""

import contextlib
import copy
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_inherit_glob as recorder


class InheritGlobContract(unittest.TestCase):
    """共通I/O契約に加えて、この参照の全件照合を検査する。"""

    def test_inputs_raw_requests_api_and_scope_are_fixed(self):
        """順序、重複、raw、要求、出典、scopeの削減を拒否する。"""
        value = recorder.template()
        self.assertEqual(sum(len(g["rows"]) for g in value["api"]["groups"]), 49)
        self.assertEqual(len(value["cli"]["cases"]), 14)
        for group, field in (
            ("api", "groups"),
            ("api", "source_sha256"),
            ("cli", "cases"),
            ("cli", "scope"),
        ):
            changed = copy.deepcopy(value)
            changed[group][field] = None
            with self.subTest(field=field), self.assertRaises(ValueError):
                recorder.validate(changed)
        for field in ("stdout_hex", "stderr_template", "requests", "files", "exit"):
            changed = copy.deepcopy(value)
            changed["cli"]["cases"][-1][field] = None
            with self.subTest(field=field), self.assertRaises(ValueError):
                recorder.validate(changed)

    def test_existing_entry_is_rejected_before_children(self):
        """既存の出力の本文とmtimeを維持し、Rubyを起動しない。"""
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

    def test_last_cli_and_api_and_requests_must_match_before_save(self):
        """最後のCLI、API展開、runtime、要求の不一致で一部成功を保存しない。"""
        data = recorder.template()
        for fault in (None, "last-cli", "api", "runtime", "requests"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                output = root / "out.json"
                calls = []

                @contextlib.contextmanager
                def server(row, *, fault=fault):
                    yield "http://127.0.0.1:12345", ([] if fault == "requests" else row["requests"])

                def capture(
                    ruby, args, cwd, files, environment, stdin=b"", *, fault=fault, calls=calls
                ):
                    calls.append(cwd)
                    index = len(calls) - 1
                    if index < 3:
                        observed = {
                            "runtime": copy.deepcopy(data["api"]["runtime"]),
                            "source_sha256": data["api"]["source_sha256"],
                            "rows": copy.deepcopy(data["api"]["groups"][index]["rows"]),
                        }
                        if fault == "api":
                            observed["rows"][-1]["expanded"] = []
                        if fault == "runtime":
                            observed["runtime"]["ruby"] = "0.0.0"
                        done = SimpleNamespace(
                            returncode=0, stdout=json.dumps(observed).encode(), stderr=b""
                        )
                    else:
                        row = data["cli"]["cases"][index - 3]
                        bound = recorder.bindings(cwd, "http://127.0.0.1:12345", row)
                        done = SimpleNamespace(
                            returncode=row["exit"],
                            stdout=bytes.fromhex(row["stdout_hex"]),
                            stderr=recorder.bind(row["stderr_template"], bound).encode(),
                        )
                        if fault == "last-cli" and index == 16:
                            done.stdout += b"unexpected"
                    return done, {}, {}

                with (
                    patch.object(recorder, "os", SimpleNamespace(name="nt")),
                    patch.object(recorder, "server", server),
                    patch.object(recorder, "_capture", capture),
                ):
                    if fault is None:
                        self.assertEqual(
                            recorder.collect(Path("unused"), root / "work", output), data
                        )
                        self.assertEqual(len(calls), 17)
                        self.assertTrue(output.is_file())
                    else:
                        with self.assertRaises(ValueError):
                            recorder.collect(Path("unused"), root / "work", output)
                        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
