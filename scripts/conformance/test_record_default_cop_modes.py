"""既定mode採取の型/保持/完了/freshを独立な負対照で検査する。"""

import copy
import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import record_default_cop_modes as recorder


def _dangling_entry(path, target):
    # Windowsの1314でも同じlexists/exists境界を実junctionで検査し、未実行を成功にしない。
    root = path.parent.resolve()
    if not path.resolve(strict=False).is_relative_to(root) or not target.resolve(
        strict=False
    ).is_relative_to(root.parent):
        raise ValueError("test entry is outside its owned temporary directory")
    try:
        path.symlink_to(target)
        kind = "symlink"
    except OSError:
        if os.name != "nt":
            raise
        if any(char in str(path) + str(target) for char in ("%", '"', "\r", "\n")):
            raise ValueError("unsafe command path") from None
        output = subprocess.run(
            f'cmd.exe /d /v:off /c mklink /J "{path}" "{target}"',
            capture_output=True,
            check=False,
        )
        if output.returncode != 0:
            raise RuntimeError(output.stderr.decode(errors="replace")) from None
        kind = "non_file"
    if not os.path.lexists(path) or path.exists():
        raise ValueError("expected a real dangling entry")
    return kind


class ReferenceContract(unittest.TestCase):
    """正しいchild応答を保ち、各guardだけを失敗させる。"""

    def test_fixed_ids_forms_runtime_types_and_queries(self):
        """45/15/3とAPI15の欠落/重複/別入力/型を拒む。"""
        for group in recorder.GROUPS:
            data = recorder._fixture(group)
            recorder.validate_fixture(data, group)
            for mutate in (
                lambda value: value.update(schema=True),
                lambda value: value.update(runtime={}),
                lambda value: value.update(cases=[]),
                lambda value: value["cases"].append(copy.deepcopy(value["cases"][0])),
                lambda value: value["cases"][0]["files"].update({"a.rb": "WRONG"}),
                lambda value: value["cases"][0].update(exit=True),
                lambda value: value["cases"][0].update(coverage="WRONG"),
                lambda value: value["cases"][0].update(stderr_hex=b"WRONG".hex()),
            ):
                changed = copy.deepcopy(data)
                mutate(changed)
                with self.assertRaises(ValueError):
                    recorder.validate_fixture(changed, group)
        data = recorder._fixture("queries")
        for key, value in (("value", 0), ("truthy", 0), ("ruby_class", "String")):
            changed = copy.deepcopy(data)
            changed["queries"][0][key] = value
            with self.assertRaises(ValueError):
                recorder.validate_fixture(changed, "queries")

    def test_completed_report_requires_identity_integer_counts_and_full_inspection(self):
        """bool==intと未完了reportを正常完了へ読み替えない。"""
        report = recorder._fixture("modes")["cases"][0]["report"]
        for key, value in (
            ("offense_count", True),
            ("target_file_count", True),
            ("inspected_file_count", True),
            ("inspected_file_count", 0),
        ):
            changed = copy.deepcopy(report)
            changed["summary"][key] = value
            with self.assertRaises(ValueError):
                recorder.completed_report(json.dumps(changed).encode())
        for mutate in (
            lambda value: value.update(files=[]),
            lambda value: value.update(files=[None]),
            lambda value: value.update(summary=False),
            lambda value: value["metadata"].update(ruby_engine="sonicop"),
            lambda value: value["metadata"].update(rubocop_version="wrong"),
        ):
            changed = copy.deepcopy(report)
            mutate(changed)
            with self.assertRaises(ValueError):
                recorder.completed_report(json.dumps(changed).encode())

    def test_environment_copy_preserves_gem_os_and_parent(self):
        """注入ENV4とNO_COLORだけを除き、TERMは測定条件へ固定する。"""
        environment = {
            "RUBYOPT": "bad",
            "RUBYLIB": "bad",
            "RUBOCOP_OPTS": "bad",
            "RUBOCOP_TARGET_RUBY_VERSION": "bad",
            "NO_COLOR": "bad",
            "GEM_HOME": "gem",
            "GEM_PATH": "gems",
            "PATH": "ruby",
            "CLICOLOR_FORCE": "0",
            "KEEP": "keep",
        }
        with patch.dict(os.environ, environment, clear=True):
            before = dict(os.environ)
            self.assertEqual(
                recorder.child_environment(),
                {
                    "GEM_HOME": "gem",
                    "GEM_PATH": "gems",
                    "PATH": "ruby",
                    "CLICOLOR_FORCE": "0",
                    "KEEP": "keep",
                    "TERM": "dumb",
                },
            )
            self.assertEqual(dict(os.environ), before)

    def test_four_fresh_paths_reject_entries_aliases_and_dangling_before_children(self):
        """既存entryと別表記重複を分離し、子未起動とKEEP保持を確かめる。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = [root / "work", root / "modes", root / "queries", root / "lists"]
            for index in range(4):
                paths[index].write_bytes(b"KEEP")
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", *paths)
                child.assert_not_called()
                self.assertEqual(paths[index].read_bytes(), b"KEEP")
                paths[index].unlink()
            for left in range(4):
                for right in range(left + 1, 4):
                    aliases = paths.copy()
                    aliases[right] = root / "part" / ".." / paths[left].name
                    self.assertTrue(all(not os.path.lexists(path) for path in aliases))
                    with (
                        patch.object(recorder.subprocess, "run") as child,
                        self.assertRaises(ValueError),
                    ):
                        recorder.collect("ruby", *aliases)
                    child.assert_not_called()
            if os.name == "nt":
                with self.assertRaises(ValueError):
                    recorder._fresh([root / "Case", root / "case", paths[2], paths[3]])
            dangling = root / "dangling"
            _dangling_entry(dangling, root / "missing")
            with patch.object(recorder.subprocess, "run") as child, self.assertRaises(ValueError):
                recorder.collect("ruby", dangling, *paths[1:])
            child.assert_not_called()
            self.assertTrue(os.path.lexists(dangling))
            self.assertFalse(dangling.exists())

    def test_positive_cli_rows_keep_full_raw_and_private_attributes(self):
        """各群の正しいchildを受理し、実exit/raw/statの束縛を残す。"""
        for group in recorder.GROUPS:
            row = recorder._fixture(group)["cases"][0]
            output = subprocess.CompletedProcess(
                [], row["exit"], bytes.fromhex(row["stdout_hex"]), bytes.fromhex(row["stderr_hex"])
            )
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / "case"
                with patch.object(recorder, "_run", return_value=output):
                    self.assertEqual(
                        recorder.record_case("ruby", Path("wrapper"), root, row, group), row
                    )
                raw = json.loads((root / "private-cli.json").read_bytes())
                self.assertEqual(raw["exit"], row["exit"])
                self.assertEqual(raw["stdout_hex"], row["stdout_hex"])
                self.assertEqual(raw["inputs_before"], raw["inputs_after"])
                self.assertIs(type(raw["inputs_before"]["a.rb"]["mtime_ns"]), int)
                self.assertIs(type(raw["inputs_before"]["a.rb"]["mode"]), int)

    def test_cli_mutations_are_independent_of_correct_reference_output(self):
        """正しい応答のままbytes/mtime/mode/outを一つずつ変更する。"""
        row = recorder._fixture("modes")["cases"][0]
        for mutation in ("bytes", "mtime", "mode", "out", "status", "stderr"):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / "case"
                original_mode = None

                def child(*_arguments):
                    nonlocal original_mode
                    path = root / "a.rb"
                    info = path.stat()
                    original_mode = info.st_mode
                    if mutation == "bytes":
                        raw = path.read_bytes()
                        path.write_bytes(raw.replace(b"is_ready", b"is_reado"))
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif mutation == "mtime":
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns + 1_000_000_000))
                    elif mutation == "mode":
                        path.chmod(info.st_mode & ~stat.S_IWRITE)
                    elif mutation == "out":
                        (root / "report.json").write_bytes(b"WRONG")
                    return subprocess.CompletedProcess(
                        [],
                        0 if mutation == "status" else row["exit"],
                        bytes.fromhex(row["stdout_hex"]),
                        b"WRONG" if mutation == "stderr" else b"",
                    )

                try:
                    with (
                        patch.object(recorder, "_run", side_effect=child),
                        self.assertRaises(ValueError),
                    ):
                        recorder.record_case("ruby", Path("wrapper"), root, row, "modes")
                finally:
                    if original_mode is not None:
                        (root / "a.rb").chmod(original_mode)

    def test_api_mutations_are_independent_of_correct_fifteen_observations(self):
        """実15値を返しつつ、APIだけの原本/out変更を独立に拒む。"""
        template = recorder._fixture("queries")
        for mutation in (None, "bytes", "mtime", "mode", "out", "status", "stderr"):
            with tempfile.TemporaryDirectory() as directory:
                work = Path(directory)
                for row in template["cases"]:
                    root = work / row["id"]
                    root.mkdir()
                    for name, value in row["files"].items():
                        (root / name).write_bytes(value.encode())
                target = work / template["cases"][0]["id"] / "a.rb"
                original_mode = target.stat().st_mode

                def child(*_arguments):
                    info = target.stat()
                    if mutation == "bytes":
                        target.write_bytes(target.read_bytes().replace(b"unless", b"unlesS"))
                        os.utime(target, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif mutation == "mtime":
                        os.utime(target, ns=(info.st_atime_ns, info.st_mtime_ns + 1_000_000_000))
                    elif mutation == "mode":
                        target.chmod(info.st_mode & ~stat.S_IWRITE)
                    elif mutation == "out":
                        target.with_name("report.json").write_bytes(b"WRONG")
                    return subprocess.CompletedProcess(
                        [],
                        2 if mutation == "status" else 0,
                        json.dumps(template["queries"]).encode(),
                        b"WRONG" if mutation == "stderr" else b"",
                    )

                try:
                    with patch.object(recorder, "_run", side_effect=child):
                        if mutation is None:
                            self.assertEqual(
                                recorder.record_queries(
                                    "ruby", work, template["cases"], template["queries"]
                                ),
                                template["queries"],
                            )
                            evidence = json.loads((work / "private-api.json").read_bytes())
                            self.assertEqual(evidence["inputs_before"], evidence["inputs_after"])
                        else:
                            with self.assertRaises(ValueError):
                                recorder.record_queries(
                                    "ruby", work, template["cases"], template["queries"]
                                )
                finally:
                    target.chmod(original_mode)

    def test_api_dangling_output_is_rejected_without_reading_its_target(self):
        """APIが作ったdangling outを、正しい15値のまま独立に拒否する。"""
        template = recorder._fixture("queries")
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            probe = work / "permission-probe"
            probe_kind = _dangling_entry(probe, work / "missing")
            if probe_kind == "symlink":
                probe.unlink()
            else:
                probe.rmdir()
            for row in template["cases"]:
                root = work / row["id"]
                root.mkdir()
                for name, value in row["files"].items():
                    (root / name).write_bytes(value.encode())
            output_path = work / template["cases"][0]["id"] / "report.json"

            def child(*_arguments):
                _dangling_entry(output_path, work / "missing")
                return subprocess.CompletedProcess(
                    [], 0, json.dumps(template["queries"]).encode(), b""
                )

            with patch.object(recorder, "_run", side_effect=child), self.assertRaises(ValueError):
                recorder.record_queries("ruby", work, template["cases"], template["queries"])
            evidence = json.loads((work / "private-api.json").read_bytes())
            self.assertEqual(evidence["inputs_before"], evidence["inputs_after"])
            self.assertEqual(evidence["out_after"][template["cases"][0]["id"]]["kind"], probe_kind)
            self.assertTrue(os.path.lexists(output_path))
            self.assertFalse(output_path.exists())

    def test_unknown_or_changed_form_never_starts_a_child(self):
        """同IDでも入力差や欠落APIを子起動前に拒む。"""
        row = recorder._fixture("modes")["cases"][0]
        for mutation in ("id", "args"):
            changed = copy.deepcopy(row)
            changed[mutation] = "unknown" if mutation == "id" else []
            with patch.object(recorder, "_run") as child, self.assertRaises(ValueError):
                recorder.record_case("ruby", Path("wrapper"), Path("unused"), changed, "modes")
            child.assert_not_called()
        queries = recorder._fixture("queries")
        with patch.object(recorder, "_run") as child, self.assertRaises(ValueError):
            recorder.record_queries(
                "ruby", Path("unused"), queries["cases"], queries["queries"][:-1]
            )
        child.assert_not_called()

    def test_exclusive_output_never_replaces_an_entry(self):
        """最終publicationのraceでも既存本文を失わない。"""
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "result.json"
            path.write_bytes(b"KEEP")
            with self.assertRaises(FileExistsError):
                recorder._write_json(path, {"wrong": True})
            self.assertEqual(path.read_bytes(), b"KEEP")


if __name__ == "__main__":
    unittest.main()
