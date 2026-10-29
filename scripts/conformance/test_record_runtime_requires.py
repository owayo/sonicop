"""実runtime採取が候補や未完了を成功へ混ぜないための単独負対照。"""

import copy
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import record_runtime_requires as recorder


def fixture(boundaries=False):
    """候補を呼ばず公開済みの実参照だけを読む。

    Args:
        boundaries: 別scopeの境界参照なら真。

    Returns:
        dict: 固定した本家参照。

    """
    return json.loads((recorder.BOUNDARIES if boundaries else recorder.MAIN).read_bytes())


class ReferenceContract(unittest.TestCase):
    """成功応答を保ち、一つの保持guardだけが失敗を検出する。"""

    def test_fixture_fixed_ids_input_types_and_scopes(self):
        """空/重複/欠落/入力変更/型違い/範囲の付替えを拒む。"""
        for boundary in (False, True):
            data = fixture(boundary)
            recorder.validate_fixture(data, boundaries=boundary)
            for mutate in (
                lambda value: value.update(schema=True),
                lambda value: value.update(native_invocations=False),
                lambda value: value.update({"runtime" if boundary else "primary": {}}),
                lambda value: value.update(cases=[]),
                lambda value: value["cases"].append(copy.deepcopy(value["cases"][0])),
                lambda value: value["cases"][0]["files"].update({"a.rb": "changed"}),
                lambda value: (value["cases"][0] if boundary else value["cases"][0]["cli"]).update(
                    exit=True
                ),
                lambda value: (value["cases"][0] if boundary else value["cases"][0]["cli"]).update(
                    stderr_hex="xz"
                ),
            ):
                changed = copy.deepcopy(data)
                mutate(changed)
                with self.assertRaises(ValueError):
                    recorder.validate_fixture(changed, boundaries=boundary)
        changed = fixture(True)
        changed["cases"][0]["comparison"] = "version_identity"
        with self.assertRaises(ValueError):
            recorder.validate_fixture(changed, boundaries=True)
        changed = fixture()
        changed["cases"][0]["api"]["loaded_features"] = [True]
        with self.assertRaises(ValueError):
            recorder.validate_fixture(changed)

    def test_environment_is_a_controlled_child_copy(self):
        """ENV4/ambient色だけを除き、GEM/PATH/OSと親を保つ。"""
        parent = {
            "RUBYOPT": "bad",
            "RUBYLIB": "bad",
            "RUBOCOP_OPTS": "bad",
            "RUBOCOP_TARGET_RUBY_VERSION": "bad",
            "NO_COLOR": "bad",
            "GEM_HOME": "gem",
            "GEM_PATH": "gems",
            "PATH": "ruby",
            "KEEP": "keep",
        }
        with patch.dict(os.environ, parent, clear=True):
            before = dict(os.environ)
            self.assertEqual(
                recorder.child_environment({"CLICOLOR_FORCE": "1"}),
                {
                    "GEM_HOME": "gem",
                    "GEM_PATH": "gems",
                    "PATH": "ruby",
                    "KEEP": "keep",
                    "CLICOLOR_FORCE": "1",
                },
            )
            self.assertEqual(dict(os.environ), before)

    def test_fresh_alias_and_existing_entries_precede_children(self):
        """既存entryと別表記重複を独立に検査し、子を起動しない。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = [root / "work", root / "main.json", root / "boundary.json"]
            for index in range(3):
                paths[index].write_bytes(b"KEEP")
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", *paths)
                child.assert_not_called()
                self.assertEqual(paths[index].read_bytes(), b"KEEP")
                paths[index].unlink()
            for right in (root / "part/../work", root / "work"):
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", paths[0], right, paths[2])
                child.assert_not_called()
            if os.name == "nt":
                with self.assertRaises(ValueError):
                    recorder._fresh([root / "Case", root / "case", paths[2]])
            link = root / "dangling"
            try:
                link.symlink_to(root / "missing")
            except OSError:
                return
            with patch.object(recorder.subprocess, "run") as child, self.assertRaises(ValueError):
                recorder.collect("ruby", link, paths[1], paths[2])
            child.assert_not_called()
            self.assertTrue(link.is_symlink())

    def test_positive_cli_api_and_boundary_complete(self):
        """正しい全rawとAPIを返すchildで保存guard自身を検証する。"""
        row = fixture()["cases"][0]
        boundary = fixture(True)["cases"][0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cli = subprocess.CompletedProcess(
                [],
                row["cli"]["exit"],
                bytes.fromhex(row["cli"]["stdout_hex"]),
                bytes.fromhex(row["cli"]["stderr_hex"]),
            )
            api = subprocess.CompletedProcess([], 0, json.dumps(row["api"]).encode(), b"")
            with patch.object(recorder, "_run", return_value=cli):
                self.assertEqual(recorder.record("ruby", root / "main", row), row)
            with patch.object(recorder, "_run", return_value=api):
                self.assertEqual(recorder.record_api("ruby", root / "main", row), row["api"])
            output = subprocess.CompletedProcess(
                [],
                boundary["exit"],
                bytes.fromhex(boundary["stdout_hex"]),
                bytes.fromhex(boundary["stderr_hex"]),
            )
            with patch.object(recorder, "_run", return_value=output):
                self.assertEqual(
                    recorder.record_boundaries("ruby", root / "boundary", boundary), boundary
                )

    def test_cli_source_bytes_mtime_and_output_fail_independently(self):
        """正しいrawのまま原本の本文/mtime/出力を一つずつ変える。"""
        row = fixture()["cases"][0]
        for mutation in ("bytes", "mtime", "out", "status"):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / "case"

                def child(*_arguments):
                    path = root / "a.rb"
                    info = path.stat()
                    if mutation == "bytes":
                        path.write_bytes(b"y=1\n")
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif mutation == "mtime":
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns + 1_000_000_000))
                    elif mutation == "out":
                        (root / "report.json").write_bytes(b"WRONG")
                    return subprocess.CompletedProcess(
                        [],
                        0 if mutation == "status" else row["cli"]["exit"],
                        bytes.fromhex(row["cli"]["stdout_hex"]),
                        bytes.fromhex(row["cli"]["stderr_hex"]),
                    )

                with (
                    patch.object(recorder, "_run", side_effect=child),
                    self.assertRaises(ValueError),
                ):
                    recorder.record("ruby", root, row)

    def test_api_source_bytes_mtime_and_output_fail_independently(self):
        """APIの正しいJSONを保ち、API時だけの原本変更を拒む。"""
        row = fixture()["cases"][0]
        for mutation in ("bytes", "mtime", "out", "stderr", "status"):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / "case"
                recorder._write_inputs(root, row)

                def child(*_arguments):
                    path = root / "a.rb"
                    info = path.stat()
                    if mutation == "bytes":
                        path.write_bytes(b"y=1\n")
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif mutation == "mtime":
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns + 1_000_000_000))
                    elif mutation == "out":
                        (root / "report.json").write_bytes(b"WRONG")
                    return subprocess.CompletedProcess(
                        [],
                        1 if mutation == "status" else 0,
                        json.dumps(row["api"]).encode(),
                        b"WRONG" if mutation == "stderr" else b"",
                    )

                with (
                    patch.object(recorder, "_run", side_effect=child),
                    self.assertRaises(ValueError),
                ):
                    recorder.record_api("ruby", root, row)

    def test_altered_api_form_and_unknown_case_never_start_child(self):
        """同じIDでも固定入力が変わったAPIを子起動前に拒む。"""
        row = fixture()["cases"][0]
        for mutation in ("id", "files"):
            changed = copy.deepcopy(row)
            if mutation == "id":
                changed["id"] = "unknown"
            else:
                changed["files"]["a.rb"] = "WRONG"
            with (
                patch.object(recorder, "_run") as child,
                self.assertRaises((ValueError, StopIteration)),
            ):
                recorder.record_api("ruby", Path("unused"), changed)
            child.assert_not_called()

    def test_private_evidence_and_projection_preserve_the_measured_boundary(self):
        """private記録は実exit/全raw/原本属性を持ち、似たprefixを投影しない。"""
        row = fixture()["cases"][0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "case"
            output = subprocess.CompletedProcess(
                [],
                row["cli"]["exit"],
                bytes.fromhex(row["cli"]["stdout_hex"]),
                bytes.fromhex(row["cli"]["stderr_hex"]),
            )
            with patch.object(recorder, "_run", return_value=output):
                recorder.record("ruby", root, row)
            evidence = json.loads((root / "private-cli-raw.json").read_bytes())
            self.assertEqual(evidence["exit"], output.returncode)
            self.assertEqual(evidence["stderr_hex"], row["cli"]["stderr_hex"])
            self.assertEqual(evidence["inputs_before"], evidence["inputs_after"])
            self.assertEqual(evidence["inputs_before"]["a.rb"]["bytes_hex"], b"x=1\n".hex())
            self.assertIs(type(evidence["inputs_before"]["a.rb"]["mtime_ns"]), int)
            self.assertIs(type(evidence["inputs_before"]["a.rb"]["mode"]), int)
            self.assertIsNone(evidence["out_after"])
            with self.assertRaises(FileExistsError):
                recorder._private(root, "private-cli-raw.json", output, {}, {})
            physical = root.resolve() if recorder.sys.platform == "darwin" else root.absolute()
            prefix = physical.as_posix().encode()
            self.assertEqual(recorder._project(prefix + b"/ext.rb", root), b"<root>/ext.rb")
            self.assertEqual(
                recorder._project(b"used in " + prefix + b"/config.yml", root),
                b"used in <root>/config.yml",
            )
            for text in (prefix + b"-sibling/ext.rb", b"prefix" + prefix + b"/ext.rb"):
                self.assertEqual(recorder._project(text, root), text)

    def test_typed_comparison_rejects_bool_counts_and_missing_fields(self):
        """Pythonのbool==intで元API/JSONを誤受理しない。"""
        for actual, expected in (
            (True, 1),
            (False, 0),
            ({"count": True}, {"count": 1}),
            ({}, {"exit": 0}),
            ([1], []),
        ):
            with self.assertRaises(ValueError):
                recorder._same(actual, expected)


if __name__ == "__main__":
    unittest.main()
