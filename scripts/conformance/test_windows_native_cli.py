"""版の違う Oracle や欠けた job を、新しい Windows 期待へ採用しない。"""

import hashlib
import json
import os
import pathlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import record_windows_native_cli as recorder


class NativeCliRecorderTest(unittest.TestCase):
    def test_version_child_uses_the_shared_isolated_environment(self):
        poison = {
            key: "poison"
            for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
        }
        metadata = {"ruby": "4.0.7", "platform": "x64-mingw-ucrt", "gems": recorder.SPEC_GEMS}
        result = subprocess.CompletedProcess([], 0, json.dumps(metadata).encode(), b"")
        with patch.dict(os.environ, poison):
            before = os.environ.copy()
            with (
                tempfile.TemporaryDirectory() as directory,
                patch.object(recorder.subprocess, "run", return_value=result) as run,
            ):
                recorder.pinned_oracle("ruby", pathlib.Path(directory))
                self.assertEqual(run.call_args.kwargs["env"], recorder.oracle_child_env())
            self.assertEqual(os.environ.copy(), before)

    def sample(self, folder):
        source = b"x=1\n"
        safe = b"x = 1\r\n"
        record = {
            "case_index": 0,
            "name": "operator",
            "source_sha256": hashlib.sha256(source).hexdigest(),
            "safe_after_hex": safe.hex(),
            "safe_after_sha256": hashlib.sha256(safe).hexdigest(),
            "expected_first_offenses": [{"corrected": True}],
            "expected_repeated_offenses": [],
        }
        requested = {
            "name": "job",
            "fixture": "input.json",
            "group": None,
            "cases": [{"index": 0, "name": "operator", "source": source}],
        }
        (folder / "input.json").write_text("{}", encoding="utf-8")
        return requested, {
            "name": "job",
            "fixture": "input.json",
            "group": None,
            "records": [record],
        }

    def test_pinned_oracle_rejects_wrong_versions_and_platform(self):
        base = {"ruby": "4.0.7", "platform": "x64-mingw-ucrt", "gems": recorder.SPEC_GEMS}
        for key, value in [
            ("ruby", "4.0.8"),
            ("platform", "x86_64-linux"),
            ("platform", None),
            ("gems", {"rubocop": "1.88.0"}),
        ]:
            metadata = {**base, key: value}
            result = subprocess.CompletedProcess([], 0, json.dumps(metadata).encode(), b"")
            with tempfile.TemporaryDirectory() as directory, self.subTest(key=key, value=value):
                with (
                    patch.object(recorder.subprocess, "run", return_value=result),
                    self.assertRaises(ValueError),
                ):
                    recorder.pinned_oracle("ruby", pathlib.Path(directory))
        # Windows の生成時にも公開 Ruby/JSON container を CRLF へ変えない。
        valid = subprocess.CompletedProcess([], 0, json.dumps(base).encode(), b"")
        with tempfile.TemporaryDirectory() as directory:
            folder = pathlib.Path(directory)
            with patch.object(recorder.subprocess, "run", return_value=valid):
                recorder.pinned_oracle("ruby", folder)
            for name in ("reference.rb", "loaded-versions.json"):
                self.assertNotIn(b"\r", (folder / name).read_bytes())

    def test_assemble_rejects_missing_duplicate_and_modified_records(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = pathlib.Path(directory)
            requested, job = self.sample(folder)
            variants = []
            for key, value in [
                ("name", "different"),
                ("source_sha256", "0" * 64),
                ("safe_after_sha256", "0" * 64),
                ("case_index", 1),
                ("case_index", False),
            ]:
                changed = json.loads(json.dumps(job))
                changed["records"][0][key] = value
                variants.append(changed)
            variants.extend([{**job, "records": []}, {**job, "records": job["records"] * 2}])
            for index, broken in enumerate(variants):
                (folder / "job.json").write_text(json.dumps(broken), encoding="utf-8")
                with self.subTest(variant=index), self.assertRaises(ValueError):
                    recorder.assemble(folder, [requested], folder, {})
            with self.assertRaises(ValueError):
                recorder.assemble(folder, [], folder, {})

    def test_assemble_preserves_oracle_values_and_ignores_stale_unrequested_jobs(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = pathlib.Path(directory)
            requested, job = self.sample(folder)
            (folder / "job.json").write_text(json.dumps(job), encoding="utf-8")
            (folder / "stale.json").write_text("not a job", encoding="utf-8")
            artifact = recorder.assemble(folder, [requested], folder, {"ruby": "4.0.7"})
            case = artifact["cases"]["input.json"]["record:0"]
            self.assertEqual(case["safe_after_hex"], job["records"][0]["safe_after_hex"])
            self.assertEqual(case["expected_first_exit"], 0)
            self.assertEqual(case["expected_repeated_exit"], 0)
            self.assertEqual(
                artifact["reference"]["fixture_sha256"]["input.json"],
                hashlib.sha256(b"{}").hexdigest(),
            )


if __name__ == "__main__":
    unittest.main()
