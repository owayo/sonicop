"""directory debug参照が欠落・重複・誤ったstreamを受け入れないことを確認する。"""

import copy
from pathlib import Path
import tempfile
import types
import unittest
from unittest import mock

import record_debug_directories as recorder


class DebugDirectoryGuards(unittest.TestCase):
    def test_incomplete_or_duplicate_listing_is_rejected(self):
        row = {
            "args": [],
            "stdout": {
                "reference_headers": ["config", "default"],
                "timer_count": 1,
                "text": "a.rb\r\nb.rb\r\nFinished in <elapsed> seconds\r\n",
                "reports": [],
            },
            "stderr": {"reference_headers": [], "timer_count": 0, "text": "", "reports": []},
            "output": None,
            "listing": True,
        }
        recorder.validate(row, ["a.rb", "b.rb"], [], [])
        for text in ("", "a.rb\r\n", "a.rb\r\na.rb\r\n"):
            candidate = copy.deepcopy(row)
            candidate["stdout"]["text"] = text + "Finished in <elapsed> seconds\r\n"
            with self.subTest(text=text), self.assertRaises(ValueError):
                recorder.validate(candidate, ["a.rb", "b.rb"], [], [])

    def test_missing_header_or_wrong_timer_stream_is_rejected(self):
        row = {
            "args": [],
            "stdout": {
                "reference_headers": ["config", "default"],
                "timer_count": 1,
                "text": "Finished in <elapsed> seconds\r\n",
                "reports": [],
            },
            "stderr": {"reference_headers": [], "timer_count": 0, "text": "", "reports": []},
            "output": None,
            "listing": True,
        }
        for stream, key, value in (
            ("stdout", "reference_headers", []),
            ("stderr", "reference_headers", ["config"]),
            ("stdout", "timer_count", 0),
            ("stderr", "timer_count", 1),
        ):
            candidate = copy.deepcopy(row)
            candidate[stream][key] = value
            with self.subTest(stream=stream, key=key), self.assertRaises(ValueError):
                recorder.validate(candidate, [], [], [])

    def test_physical_input_mutation_is_rejected(self):
        def run(_args, *, cwd, **_kwargs):
            (cwd / "a.rb").write_bytes(b"changed\n")
            return types.SimpleNamespace(returncode=0, stdout=b"", stderr=b"")

        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(recorder.subprocess, "run", side_effect=run),
            mock.patch.object(recorder, "validate"),
            self.assertRaisesRegex(ValueError, "原本byte"),
        ):
            recorder._record(
                "fixed-ruby", "reference.rb", Path(directory) / "recording", recorder.forms()[3]
            )


if __name__ == "__main__":
    unittest.main()
