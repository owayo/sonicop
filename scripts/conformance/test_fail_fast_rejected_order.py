"""実参照の一覧が空・欠落・重複なら採取を失敗にする。"""

import pathlib
import tempfile
import types
import unittest
from unittest import mock

import record_fail_fast_rejected_order as recorder


class RejectedListingGuards(unittest.TestCase):
    def test_incomplete_duplicate_or_failed_listing_is_rejected(self):
        for code, stdout, stderr in (
            (0, b"", b""),
            (0, b"a.rb\n", b""),
            (0, b"a.rb\na.rb\n", b""),
            (2, b"a.rb\nb.rb\n", b""),
            (0, b"a.rb\nb.rb\n", b"warning"),
        ):
            with (
                self.subTest(code=code, stdout=stdout, stderr=stderr),
                tempfile.TemporaryDirectory() as directory,
            ):
                result = types.SimpleNamespace(returncode=code, stdout=stdout, stderr=stderr)
                with (
                    mock.patch.object(recorder.reference, "_run", return_value=result),
                    self.assertRaises(ValueError),
                ):
                    recorder._record(
                        "fixed-ruby",
                        pathlib.Path("reference.rb"),
                        pathlib.Path(directory) / "input",
                        False,
                    )

    def test_source_mutation_is_rejected_after_a_complete_listing(self):
        def run(_command, cwd, _stdin):
            (cwd / "a.rb").write_bytes(b"changed\n")
            return types.SimpleNamespace(returncode=0, stdout=b"b.rb\r\na.rb\r\n", stderr=b"")

        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(recorder.reference, "_run", side_effect=run),
            self.assertRaisesRegex(ValueError, "changed physical input"),
        ):
            recorder._record(
                "fixed-ruby", pathlib.Path("reference.rb"), pathlib.Path(directory) / "input", True
            )


if __name__ == "__main__":
    unittest.main()
