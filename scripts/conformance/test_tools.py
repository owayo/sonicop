"""比較ツールが何も測らず成功する経路を陰性対照で止める。"""

import contextlib
import hashlib
import io
import json
import os
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

    def rollback_fixture(self, root, message=None, relative="sample.rb"):
        manifest = self.fixture(root)
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace("path: sample.rb", f"path: {relative}"),
            encoding="utf-8",
        )
        work = root / "work"
        work.mkdir()
        source = work / relative
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_bytes(b"before\n")
        stderr = root / "stderr.txt"
        # 本体のstderrと同じUTF-8バイトを使い、Windowsの既定文字コードへ依存させない。
        stderr.write_bytes(
            (
                message
                or f"Autocorrection was not written to {source} because it introduced a syntax error.\n"
            ).encode("utf-8")
        )
        return stderr, work, manifest, "example", "unsafe"

    def test_registered_rollback_path_is_recognized(self):
        with tempfile.TemporaryDirectory() as scratch:
            result = rollbacks.check(*self.rollback_fixture(pathlib.Path(scratch)))
            self.assertEqual(result, [{"path": "sample.rb", "divergence_id": "example"}])

    def test_registered_nested_rollback_path_uses_manifest_separators(self):
        with tempfile.TemporaryDirectory() as scratch:
            result = rollbacks.check(
                *self.rollback_fixture(pathlib.Path(scratch), relative="foo/bar.rb")
            )
            self.assertEqual(result, [{"path": "foo/bar.rb", "divergence_id": "example"}])

    def test_registered_unicode_rollback_stderr_is_utf8(self):
        with tempfile.TemporaryDirectory() as scratch:
            relative = "入れ子/例😀.rb"
            result = rollbacks.check(
                *self.rollback_fixture(pathlib.Path(scratch), relative=relative)
            )
            self.assertEqual(result, [{"path": relative, "divergence_id": "example"}])

    def test_rollback_namespace_conversion_preserves_path_flavors(self):
        for path, expected in [
            (r"C:\copy\foo\bar.rb", r"C:\copy\foo\bar.rb"),
            (r"\\?\C:\copy\foo\bar.rb", r"C:\copy\foo\bar.rb"),
            (r"\\?\UNC\server\share\copy\例.rb", r"\\server\share\copy\例.rb"),
            (r"\\?\unc\server\share\copy\例.rb", r"\\server\share\copy\例.rb"),
            (r"\\.\pipe\example", r"\\.\pipe\example"),
            (r"\\?\Volume{example}\case.rb", r"\\?\Volume{example}\case.rb"),
        ]:
            with self.subTest(path=path):
                result = rollbacks._without_extended_prefix(pathlib.PureWindowsPath(path))
                self.assertEqual(result.as_posix(), pathlib.PureWindowsPath(expected).as_posix())
        for path in [r"/copy/literal\name.rb", r"//?/C:/copy/literal\name.rb"]:
            source = pathlib.PurePosixPath(path)
            self.assertEqual(rollbacks._without_extended_prefix(source).as_posix(), path)

    @unittest.skipUnless(sys.platform == "win32", "Windowsの実パスで名前空間を照合する")
    def test_windows_rollback_paths_accept_extended_unicode_aliases(self):
        for spelling in ["native", "slash", "extended"]:
            for extended_work in [False, True]:
                with (
                    self.subTest(spelling=spelling, extended_work=extended_work),
                    tempfile.TemporaryDirectory() as scratch,
                ):
                    relative = "入れ子/例😀.rb"
                    stderr, work, manifest, corpus, mode = self.rollback_fixture(
                        pathlib.Path(scratch), relative=relative
                    )
                    source = str(work / relative)
                    if spelling == "slash":
                        source = source.replace("\\", "/")
                    elif spelling == "extended":
                        source = "\\\\?\\" + source
                    stderr.write_bytes(
                        f"Autocorrection was not written to {source} "
                        "because it introduced a syntax error.\n".encode()
                    )
                    if extended_work:
                        work = pathlib.Path("\\\\?\\" + str(work))
                    self.assertEqual(
                        rollbacks.check(stderr, work, manifest, corpus, mode),
                        [{"path": relative, "divergence_id": "example"}],
                    )

    @unittest.skipIf(sys.platform == "win32", "POSIXのファイル名中のbackslashを検査する")
    def test_rollback_paths_keep_posix_literal_backslashes(self):
        with tempfile.TemporaryDirectory() as scratch:
            relative = r"literal\name.rb"
            stderr, work, manifest, corpus, mode = self.rollback_fixture(
                pathlib.Path(scratch), relative=relative
            )
            self.assertEqual(
                rollbacks.check(stderr, work, manifest, corpus, mode),
                [{"path": relative, "divergence_id": "example"}],
            )
            # 実際には別のパスなので、区切りに読み替えた名前は受理しない。
            stderr.write_bytes(
                f"Autocorrection was not written to {work}/literal/name.rb "
                "because it introduced a syntax error.\n".encode()
            )
            with self.assertRaises(ValueError):
                rollbacks.check(stderr, work, manifest, corpus, mode)

    def test_rollback_outside_the_work_copy_is_rejected(self):
        for outside_name in ["other", "work-sibling"]:
            with (
                self.subTest(outside=outside_name),
                tempfile.TemporaryDirectory() as scratch,
            ):
                root = pathlib.Path(scratch)
                inputs = self.rollback_fixture(root, relative="foo/bar.rb")
                outside = root / outside_name / "foo/bar.rb"
                outside.parent.mkdir(parents=True)
                outside.write_bytes(b"before\n")
                inputs[0].write_bytes(
                    f"Autocorrection was not written to {outside} "
                    "because it introduced a syntax error.\n".encode()
                )
                with self.assertRaises(ValueError):
                    rollbacks.check(*inputs)

    def test_registered_rollback_with_another_reason_is_rejected(self):
        for reason in ["an encoding error.", "a syntax error", "a syntax error. extra"]:
            with self.subTest(reason=reason), tempfile.TemporaryDirectory() as scratch:
                inputs = self.rollback_fixture(pathlib.Path(scratch), relative="foo/bar.rb")
                message = inputs[0].read_text(encoding="utf-8")
                inputs[0].write_bytes(message.replace("a syntax error.", reason).encode("utf-8"))
                with self.assertRaises(ValueError):
                    rollbacks.check(*inputs)

    def test_registered_rollback_mixed_with_another_error_is_rejected(self):
        for other in ["cannot read input\n", "unknown-path"]:
            with self.subTest(other=other), tempfile.TemporaryDirectory() as scratch:
                inputs = self.rollback_fixture(pathlib.Path(scratch), relative="foo/bar.rb")
                message = inputs[0].read_text(encoding="utf-8")
                addition = (
                    message.replace("bar.rb", "unknown.rb") if other == "unknown-path" else other
                )
                inputs[0].write_bytes((message + addition).encode("utf-8"))
                with self.assertRaises(ValueError):
                    rollbacks.check(*inputs)

    def test_invalid_utf8_rollback_stderr_is_rejected(self):
        with tempfile.TemporaryDirectory() as scratch:
            inputs = self.rollback_fixture(pathlib.Path(scratch))
            inputs[0].write_bytes(inputs[0].read_bytes() + b"\xff")
            with self.assertRaises(UnicodeDecodeError):
                rollbacks.check(*inputs)

    def test_rollback_cli_outputs_json_across_locales_and_rejects_mixed_errors(self):
        with tempfile.TemporaryDirectory() as scratch:
            relative = "入れ子/例😀.rb"
            stderr, work, manifest, corpus, mode = self.rollback_fixture(
                pathlib.Path(scratch), relative=relative
            )
            message = stderr.read_bytes()
            command = [
                sys.executable,
                "-X",
                "utf8=0",
                str(pathlib.Path(rollbacks.__file__)),
                str(stderr),
                str(work),
                str(manifest),
                corpus,
                mode,
            ]
            # Native UTF-8の入力と判定JSONを、従来のWindows文字コードでも受け渡す。
            env = dict(os.environ, PYTHONUTF8="0", PYTHONIOENCODING="cp932")
            accepted = subprocess.run(command, capture_output=True, check=False, env=env)
            self.assertEqual(accepted.returncode, 0, accepted.stderr)
            self.assertEqual(
                json.loads(accepted.stdout), [{"path": relative, "divergence_id": "example"}]
            )
            for additional in [b"cannot read input\n", b"\xff"]:
                stderr.write_bytes(message + additional)
                rejected = subprocess.run(command, capture_output=True, check=False, env=env)
                self.assertEqual(rejected.returncode, 2, rejected.stderr)
                self.assertEqual(rejected.stdout, b"")

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
