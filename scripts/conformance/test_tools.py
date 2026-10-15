"""比較ツールが何も測らず成功する経路を陰性対照で止める。"""

import contextlib
import hashlib
import io
import pathlib
import subprocess
import sys
import tempfile
import unittest

import ff_gate
import rollbacks
import syntaxcheck


class MeasurementToolsTest(unittest.TestCase):
    def fixture(self, root, known=True):
        upstream = root / "ff-up-example"
        candidate = root / "ff-port-example"
        upstream.mkdir()
        candidate.mkdir()
        (upstream / "sample.rb").write_bytes(b"before\n")
        (candidate / "sample.rb").write_bytes(b"after\n")
        (root / "ff-diff-example.txt").write_text(
            f"Files {upstream}/sample.rb and {candidate}/sample.rb differ\n"
        )
        manifest = root / "manifest.yml"
        text = "autocorrect_divergences:\n"
        if known:
            text += (
                "  - id: example\n    corpus: example\n    path: sample.rb\n"
                f"    rubocop_sha256: {hashlib.sha256(b'before' + bytes([10])).hexdigest()[:16]}\n"
                f"    sonicop_sha256: {hashlib.sha256(b'after' + bytes([10])).hexdigest()[:16]}\n"
            )
        manifest.write_text(text)
        return manifest

    def gate(self, root, manifest):
        with contextlib.redirect_stdout(io.StringIO()):
            return ff_gate.main(["example", str(root), str(manifest), "unsafe"])

    def test_known_output_hashes_match(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            self.assertEqual(self.gate(root, self.fixture(root)), 0)

    def test_unknown_difference_is_rejected(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            self.assertEqual(self.gate(root, self.fixture(root, known=False)), 1)

    def test_changed_hash_is_rejected(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            manifest = self.fixture(root)
            (root / "ff-port-example/sample.rb").write_bytes(b"unexpected\n")
            self.assertEqual(self.gate(root, manifest), 1)

    def test_healed_difference_is_rejected(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            manifest = self.fixture(root)
            (root / "ff-diff-example.txt").write_text("")
            self.assertEqual(self.gate(root, manifest), 1)

    def test_missing_syntax_input_is_rejected(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            for name in ["source", "upstream", "candidate"]:
                (root / name).mkdir()
            (root / "source/sample.rb").write_text("value = 1\n")
            with self.assertRaises(RuntimeError):
                syntaxcheck.changed(
                    *(str(root / name) for name in ["source", "candidate", "upstream"])
                )

    def test_missing_syntax_input_cli_returns_two(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            for name in ["source/example", "output/ff-up-example", "output/ff-port-example"]:
                (root / name).mkdir(parents=True)
            (root / "source/example/sample.rb").write_text("value = 1\n")
            process = subprocess.run(
                [sys.executable, str(pathlib.Path(syntaxcheck.__file__)), "example"],
                env={
                    "CORPUS_ROOT": str(root / "source"),
                    "OUT_DIR": str(root / "output"),
                    "PATH": "/usr/bin:/bin",
                },
                capture_output=True,
                check=False,
            )
            self.assertEqual(process.returncode, 2)

    def rollback_fixture(self, root, message=None):
        manifest = self.fixture(root)
        work = root / "work"
        work.mkdir()
        stderr = root / "stderr.txt"
        stderr.write_text(
            message
            or f"Autocorrection was not written to {work}/sample.rb because it introduced a syntax error.\n"
        )
        return stderr, work, manifest, "example", "unsafe"

    def test_registered_rollback_path_is_recognized(self):
        with tempfile.TemporaryDirectory() as scratch:
            result = rollbacks.check(*self.rollback_fixture(pathlib.Path(scratch)))
            self.assertEqual(result, [{"path": "sample.rb", "divergence_id": "example"}])

    def test_unregistered_rollback_path_is_rejected(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            message = f"Autocorrection was not written to {root}/work/other.rb because it introduced a syntax error.\n"
            with self.assertRaises(ValueError):
                rollbacks.check(*self.rollback_fixture(root, message))

    def test_other_exit_two_reason_is_rejected(self):
        with tempfile.TemporaryDirectory() as scratch:
            with self.assertRaises(ValueError):
                rollbacks.check(
                    *self.rollback_fixture(pathlib.Path(scratch), "cannot read input\n")
                )

    def syntax_fixture(self, upstream, candidate):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            for name, source in [
                ("source", "value = 1\n"),
                ("upstream", upstream),
                ("candidate", candidate),
            ]:
                (root / name).mkdir()
                (root / name / "sample.rb").write_text(source)
            return syntaxcheck.classify(
                *(str(root / name) for name in ["source", "upstream", "candidate"]), "sample.rb"
            )

    def test_candidate_only_invalid_output_is_detected(self):
        self.assertEqual(self.syntax_fixture("value = 2\n", "value = (\n"), "移植版だけ壊れた")

    def test_identical_invalid_output_is_detected(self):
        self.assertEqual(self.syntax_fixture("value = (\n", "value = (\n"), "両方壊れた")


if __name__ == "__main__":
    unittest.main()
