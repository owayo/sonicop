"""callback参照の削減と、部分成功だけの保存を拒否する契約。"""

import copy
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_cache_root_option_order as recorder


class CacheRootOrderContract(unittest.TestCase):
    """共通I/O契約に加え、補正一件と全43 processの照合を検査する。"""

    def test_fixed_inputs_raw_source_scope_and_correction_are_required(self):
        """順序、runtime、全raw、補正後本文と比較範囲の変更を拒否する。"""
        value = recorder.template()
        self.assertEqual(len(value["cases"]), 42)
        for field in ("runtime", "source_sha256", "api_stdout_hex", "scope", "reference_help_hex"):
            changed = copy.deepcopy(value)
            changed[field] = None
            with self.subTest(field=field), self.assertRaises(ValueError):
                recorder.validate(changed)
        for field in (
            "args",
            "files",
            "stdout_hex",
            "stderr_hex",
            "marker_hex",
            "cache_roots_present",
            "corrected_source_hex",
        ):
            changed = copy.deepcopy(value)
            changed["cases"][0][field] = "changed"
            with self.subTest(field=field), self.assertRaises(ValueError):
                recorder.validate(changed)

    def test_existing_entry_is_rejected_before_any_child(self):
        """既存の出力を保持し、実Rubyを起動しない。"""
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

    def test_last_cli_api_marker_or_corrected_bytes_fail_before_save(self):
        """最後の出力や補正後原本の不一致を、途中まで成功した記録として保存しない。"""
        data = recorder.template()
        for fault in (None, "last-cli", "api", "marker", "corrected-source"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                output = root / "out.json"
                calls = []

                def capture(
                    ruby, args, cwd, files, environment, stdin=b"", *, fault=fault, calls=calls
                ):
                    calls.append(cwd)
                    if len(calls) == 1:
                        stdout = bytes.fromhex(data["api_stdout_hex"])
                        if fault == "api":
                            stdout += b"unexpected"
                        return SimpleNamespace(returncode=0, stdout=stdout, stderr=b""), {}, {}
                    row = next(row for row in data["cases"] if row["id"] == cwd.name)
                    protected = set(row["files"])
                    if row["corrected_source_hex"] is not None:
                        protected.remove("a.rb")
                        (cwd / "a.rb").write_bytes(
                            b"wrong"
                            if fault == "corrected-source"
                            else bytes.fromhex(row["corrected_source_hex"])
                        )
                    self.assertEqual(set(files), protected)
                    if row["marker_hex"] is not None and fault != "marker":
                        (cwd / "loaded_marker").write_bytes(bytes.fromhex(row["marker_hex"]))
                    for name, present in row["cache_roots_present"].items():
                        if present:
                            (cwd / name).mkdir()
                    stderr = bytes.fromhex(row["stderr_hex"])
                    if fault == "last-cli" and len(calls) == 43:
                        stderr += b"unexpected"
                    return (
                        SimpleNamespace(
                            returncode=row["exit"],
                            stdout=bytes.fromhex(row["stdout_hex"]),
                            stderr=stderr,
                        ),
                        {},
                        {},
                    )

                with (
                    patch.object(recorder, "os", SimpleNamespace(name="nt")),
                    patch.object(recorder, "_capture", capture),
                ):
                    if fault is None:
                        self.assertEqual(
                            recorder.collect(Path("unused"), root / "work", output), data
                        )
                        self.assertEqual(len(calls), 43)
                        self.assertEqual(json.loads(output.read_bytes()), data)
                    else:
                        with self.assertRaises(ValueError):
                            recorder.collect(Path("unused"), root / "work", output)
                        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
