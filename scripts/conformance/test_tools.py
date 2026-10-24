"""比較ツールが何も測らず成功する経路を陰性対照で止める。"""

import contextlib
import hashlib
import io
import json
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

    def test_windows_diff_paths_preserve_known_and_unknown_differences(self):
        for prefix in ("C:/scratch/", "C:\\scratch\\", "C:\\scratch/", "\\\\server\\share\\"):
            for known in (True, False):
                with (
                    self.subTest(prefix=prefix, known=known),
                    tempfile.TemporaryDirectory() as scratch,
                ):
                    root = pathlib.Path(scratch)
                    manifest = self.fixture(root, known=known)
                    (root / "ff-diff-example.txt").write_text(
                        f"Files {prefix}ff-up-example/sample.rb and "
                        f"{prefix}ff-port-example/sample.rb differ\n"
                    )
                    self.assertEqual(self.gate(root, manifest), 0 if known else 1)

    def test_windows_diff_paths_normalize_nested_directories(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            (root / "ff-diff-example.txt").write_text(
                "Files C:\\scratch\\ff-up-example\\lib\\sample.rb and "
                "C:\\scratch\\ff-port-example\\lib\\sample.rb differ\n"
            )
            self.assertEqual(ff_gate.differing(root, "example"), ["lib/sample.rb"])

    def test_posix_diff_paths_keep_literal_backslashes(self):
        for prefix in ("/scratch/", "//scratch/"):
            with (
                self.subTest(prefix=prefix),
                tempfile.TemporaryDirectory() as scratch,
            ):
                root = pathlib.Path(scratch)
                (root / "ff-diff-example.txt").write_text(
                    f"Files {prefix}ff-up-example/literal\\name.rb and "
                    f"{prefix}ff-port-example/literal\\name.rb differ\n"
                )
                self.assertEqual(ff_gate.differing(root, "example"), ["literal\\name.rb"])

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

    def test_complete_syntax_record_has_positive_unique_paths(self):
        with tempfile.TemporaryDirectory() as scratch:
            report = pathlib.Path(scratch) / "report.json"
            report.write_text(
                json.dumps(
                    {
                        "files": [{"path": "sample.rb"}],
                        "summary": {"target_file_count": 1, "inspected_file_count": 1},
                    }
                )
            )
            self.assertEqual(syntaxcheck.complete_paths(report), {"sample.rb"})

    def test_incomplete_or_empty_syntax_record_is_rejected(self):
        for inspected, target, files in [
            (1, 2, [{"path": "a.rb"}]),
            (0, 0, []),
            (2, 2, [{"path": "a.rb"}, {"path": "a.rb"}]),
        ]:
            with (
                self.subTest(inspected=inspected, target=target),
                tempfile.TemporaryDirectory() as scratch,
            ):
                report = pathlib.Path(scratch) / "report.json"
                report.write_text(
                    json.dumps(
                        {
                            "files": files,
                            "summary": {
                                "target_file_count": target,
                                "inspected_file_count": inspected,
                            },
                        }
                    )
                )
                with self.assertRaises(RuntimeError):
                    syntaxcheck.complete_paths(report)

    def test_candidate_safety_mode_needs_no_upstream_but_rejects_broken_output(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            source = root / "source/example"
            output = root / "output"
            candidate = output / "ff-port-example"
            source.mkdir(parents=True)
            candidate.mkdir(parents=True)
            (source / "sample.rb").write_text("value = 1\n")
            report = output / "ff-sc-example.json"
            report.write_text(
                json.dumps(
                    {
                        "files": [{"path": "sample.rb"}],
                        "summary": {"target_file_count": 1, "inspected_file_count": 1},
                    }
                )
            )
            for text, expected in [("value = 2\n", 0), ("value = (\n", 1)]:
                (candidate / "sample.rb").write_text(text)
                process = subprocess.run(
                    [
                        sys.executable,
                        str(pathlib.Path(syntaxcheck.__file__)),
                        "example",
                        "--candidate-only",
                    ],
                    env={
                        "CORPUS_ROOT": str(root / "source"),
                        "OUT_DIR": str(output),
                        "PATH": syntaxcheck.ENV["PATH"],
                    },
                    capture_output=True,
                    check=False,
                )
                self.assertEqual(process.returncode, expected, process.stderr)

    def test_comparative_syntax_mode_requires_both_complete_matching_reports(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            source = root / "source/example"
            output = root / "output"
            for directory in [source, output / "ff-port-example", output / "ff-up-example"]:
                directory.mkdir(parents=True)
                (directory / "sample.rb").write_text("value = 1\n")
            complete = {
                "files": [{"path": "sample.rb"}],
                "summary": {"target_file_count": 1, "inspected_file_count": 1},
            }
            (output / "ff-sc-example.json").write_text(json.dumps(complete))
            reference = output / "ff-rc-example.json"
            # 本家の記録が無い・途中終了・別集合の場合は、複製の中身が同じでも失敗する。
            for data, expected in [
                (None, 2),
                ({"files": [], "summary": {"target_file_count": 1, "inspected_file_count": 0}}, 2),
                ({**complete, "files": [{"path": "other.rb"}]}, 2),
                (complete, 0),
            ]:
                if data is not None:
                    reference.write_text(json.dumps(data))
                process = subprocess.run(
                    [sys.executable, str(pathlib.Path(syntaxcheck.__file__)), "example"],
                    env={
                        "CORPUS_ROOT": str(root / "source"),
                        "OUT_DIR": str(output),
                        "PATH": syntaxcheck.ENV["PATH"],
                    },
                    capture_output=True,
                    check=False,
                )
                self.assertEqual(process.returncode, expected, process.stderr)

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
