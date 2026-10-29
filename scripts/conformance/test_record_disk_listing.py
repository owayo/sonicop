"""19条件の採取を、候補を起動せず独立した失敗対照で保護する。"""

import copy
import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_disk_listing as recorder

_REAL_RUN = subprocess.run


def _fixture():
    return recorder._template()


def _answer(case, **kwargs):
    stdout, stderr = (bytes.fromhex(case[name]) for name in ("stdout_hex", "stderr_hex"))
    if "stdout" in kwargs:
        kwargs["stdout"].write(stdout)
        kwargs["stderr"].write(stderr)
    return SimpleNamespace(returncode=case["exit"], stdout=stdout, stderr=stderr)


def _dangling_entry(path, target):
    root = path.parent.resolve()
    if not path.resolve(strict=False).is_relative_to(root) or not target.resolve(
        strict=False
    ).is_relative_to(root.parent):
        raise ValueError("専用tempdir外へlinkを作成できません")
    try:
        path.symlink_to(target, target_is_directory=True)
    except OSError:
        if os.name != "nt" or any(c in str(path) + str(target) for c in '%"\r\n'):
            raise
        done = _REAL_RUN(
            f'cmd.exe /d /v:off /c mklink /J "{path}" "{target}"',
            capture_output=True,
            check=False,
        )
        if done.returncode:
            raise ValueError("real dangling junctionを作成できません")
    if not os.path.lexists(path) or path.exists():
        raise ValueError("dangling entryの対照が成立していません")


def _remove_entry(path):
    if path.is_symlink():
        path.unlink()
    else:
        path.rmdir()


class DiskListingContract(unittest.TestCase):
    """実rawの完全一致と原本・出力先の独立した保持を検査する。"""

    def test_fixed_scope_schema_source_and_raw(self):
        """空・重複・順序・型・入力・出典・raw差替えを採取前に拒否する。"""
        value = _fixture()
        recorder.validate_fixture(value)
        self.assertEqual(tuple(row["id"] for row in value["cases"]), recorder.IDS)
        for name in (
            "empty",
            "duplicate",
            "order",
            "schema",
            "scope",
            "exit",
            "input",
            "source",
            "raw",
        ):
            broken = copy.deepcopy(value)
            if name == "empty":
                broken["cases"] = []
            elif name == "duplicate":
                broken["cases"][-1] = broken["cases"][0]
            elif name == "order":
                broken["cases"].reverse()
            elif name == "schema":
                broken["schema"] = True
            elif name == "scope":
                broken["cases"][0]["coverage"] = "limited"
            elif name == "exit":
                broken["cases"][0]["exit"] = False
            elif name == "input":
                broken["cases"][0]["args"].append("other.rb")
            elif name == "source":
                broken["source_sha256"].pop("runner.rb")
            else:
                broken["cases"][0]["stdout_hex"] = ""
            with self.subTest(name=name), self.assertRaises(ValueError):
                recorder.validate_fixture(broken)

    def test_child_environment_keeps_parent_and_gem_paths(self):
        """ENV4とNO_COLORだけを除き、色/TERM固定以外の親環境を変更しない。"""
        with patch.dict(
            os.environ,
            {
                "RUBYOPT": "bad",
                "RUBYLIB": "bad",
                "RUBOCOP_OPTS": "bad",
                "RUBOCOP_TARGET_RUBY_VERSION": "bad",
                "NO_COLOR": "yes",
                "KEEP_SENTINEL": "keep",
                "GEM_HOME": "retained",
            },
        ):
            before = dict(os.environ)
            child = recorder.child_environment()
            self.assertEqual(dict(os.environ), before)
            expected = {
                key: value
                for key, value in before.items()
                if (key.upper() if os.name == "nt" else key)
                not in {
                    "RUBYOPT",
                    "RUBYLIB",
                    "RUBOCOP_OPTS",
                    "RUBOCOP_TARGET_RUBY_VERSION",
                    "NO_COLOR",
                }
            }
            expected.update(CLICOLOR_FORCE="0", TERM="dumb")
            self.assertEqual(child, expected)

    def test_all_nineteen_positive_raw_and_private_attributes(self):
        """実参照の19応答と実File redirectを使い、全属性の証跡を保存する。"""
        with tempfile.TemporaryDirectory() as temporary:
            for row in _fixture()["cases"]:
                root = Path(temporary) / row["id"]
                with patch.object(
                    recorder.subprocess,
                    "run",
                    side_effect=lambda *args, row=row, **kwargs: _answer(row, **kwargs),
                ):
                    self.assertEqual(recorder.record("ruby", row, root), row)
                private = json.loads((root / "private-cli.json").read_bytes())
                self.assertEqual(private["inputs_before"], private["inputs_after"])
                self.assertEqual(private["out_before"], private["out_after"])
                self.assertEqual(private["stdout_hex"], row["stdout_hex"])
                self.assertEqual(private["exit"], row["exit"])

    def test_unknown_and_changed_case_precede_child(self):
        """未知IDや同IDの変更入力を子起動前に拒否する。"""
        with tempfile.TemporaryDirectory() as temporary:
            for kind in ("id", "args"):
                row = copy.deepcopy(_fixture()["cases"][0])
                if kind == "id":
                    row["id"] = "unknown"
                else:
                    row["args"].append("other.rb")
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.record("ruby", row, Path(temporary) / kind)
                child.assert_not_called()

    def test_listing_exit_order_channel_and_empty_are_not_success(self):
        """正規出力の欠落・別path・重複・逆順・宛先・終了値を独立に拒否する。"""
        row = next(r for r in _fixture()["cases"] if r["id"] == "explicit-reverse")
        good = _answer(row)
        invalid = [
            SimpleNamespace(returncode=0, stdout=b"", stderr=b""),
            SimpleNamespace(returncode=0, stdout=b"other.rb\r\n", stderr=b""),
            SimpleNamespace(returncode=0, stdout=b"good.rb\r\ngood.rb\r\n", stderr=b""),
            SimpleNamespace(returncode=0, stdout=b"good.rb\r\nsecond.rb\r\n", stderr=b""),
            SimpleNamespace(returncode=0, stdout=b"", stderr=good.stdout),
            SimpleNamespace(returncode=2, stdout=good.stdout, stderr=b""),
            SimpleNamespace(returncode=False, stdout=good.stdout, stderr=b""),
        ]
        with tempfile.TemporaryDirectory() as temporary:
            for index, child in enumerate(invalid):
                with (
                    patch.object(recorder.subprocess, "run", return_value=child),
                    self.assertRaises(ValueError),
                ):
                    recorder.record("ruby", row, Path(temporary) / str(index))

    def test_complete_json_and_stdin_suffix_are_required(self):
        """zero/bool件数・重複file・別path・診断field・suffix差だけでも拒否する。"""
        row = next(r for r in _fixture()["cases"] if r["id"] == "stdin-json-correction")
        report = json.loads(bytes.fromhex(row["stdout_hex"]).decode().split("\r\n")[0])
        for key, value in (
            ("target_file_count", 0),
            ("inspected_file_count", True),
            ("offense_count", False),
        ):
            changed = copy.deepcopy(report)
            changed["summary"][key] = value
            with self.assertRaises(ValueError):
                recorder.split_report(json.dumps(changed).encode())
        with tempfile.TemporaryDirectory() as temporary:
            for index, kind in enumerate(("duplicate", "path", "field", "suffix")):
                changed = copy.deepcopy(report)
                if kind == "duplicate":
                    changed["files"] *= 2
                elif kind == "path":
                    changed["files"][0]["path"] = "other.rb"
                elif kind == "field":
                    changed["files"][0]["offenses"][0]["location"]["length"] += 1
                suffix = b"wrong" if kind == "suffix" else bytes.fromhex(row["suffix_hex"])
                child = SimpleNamespace(
                    returncode=row["exit"], stdout=json.dumps(changed).encode() + suffix, stderr=b""
                )
                with (
                    patch.object(recorder.subprocess, "run", return_value=child),
                    self.assertRaises(ValueError),
                ):
                    recorder.record("ruby", row, Path(temporary) / str(index))

    def test_input_bytes_mtime_mode_and_new_output_each_fail_independently(self):
        """正しいCLI応答のまま一つの保持条件だけ破り、別guardで隠さない。"""
        row = _fixture()["cases"][0]
        with tempfile.TemporaryDirectory() as temporary:
            for kind in ("bytes", "mtime", "mode", "out"):

                def mutation(*args, kind=kind, **kwargs):
                    root = kwargs["cwd"]
                    path = root / "good.rb"
                    before = path.stat()
                    if kind == "bytes":
                        path.write_bytes(b"y=1\n")
                        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
                    elif kind == "mtime":
                        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns + 2_000_000_000))
                    elif kind == "mode":
                        path.chmod(stat.S_IMODE(before.st_mode) & ~0o222)
                    else:
                        (root / "report.json").write_bytes(b"unexpected")
                    return _answer(row)

                root = Path(temporary) / kind
                try:
                    with (
                        patch.object(recorder.subprocess, "run", side_effect=mutation),
                        self.assertRaises(ValueError),
                    ):
                        recorder.record("ruby", row, root)
                finally:
                    path = root / "good.rb"
                    path.chmod(0o666)

    def test_existing_output_same_bytes_rewrite_is_not_preserved(self):
        """既存outの同内容再書込みもmtimeだけの差で拒否する。"""
        row = next(r for r in _fixture()["cases"] if r["id"] == "existing-out")

        def mutation(*args, **kwargs):
            path = kwargs["cwd"] / "report.json"
            before = path.stat()
            path.write_bytes(path.read_bytes())
            os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns + 2_000_000_000))
            return _answer(row)

        with tempfile.TemporaryDirectory() as temporary:
            with (
                patch.object(recorder.subprocess, "run", side_effect=mutation),
                self.assertRaises(ValueError),
            ):
                recorder.record("ruby", row, Path(temporary) / "case")

    def test_fresh_alias_existing_and_real_dangling_entries_precede_child(self):
        """既存entryと未作成aliasを別々に検査し、子未起動を保証する。"""
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            for index in range(2):
                paths = [base / "work", base / "output"]
                paths[index].write_bytes(b"KEEP")
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", *paths)
                child.assert_not_called()
                self.assertEqual(paths[index].read_bytes(), b"KEEP")
                paths[index].unlink()
            aliases = [
                (base / "same", base / "same"),
                (base / "parent/../same", base / "same"),
                (base / "parent/work", base / "parent"),
            ]
            if os.name == "nt":
                aliases.append((base / "CASE", base / "case"))
            for work, output in aliases:
                self.assertFalse(os.path.lexists(work))
                self.assertFalse(os.path.lexists(output))
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", work, output)
                child.assert_not_called()
            entry = base / "output"
            _dangling_entry(entry, base / "missing")
            try:
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaises(ValueError),
                ):
                    recorder.collect("ruby", base / "work", entry)
                child.assert_not_called()
            finally:
                _remove_entry(entry)

    def test_new_real_dangling_output_is_not_absence(self):
        """lexists=True/exists=Falseの新entryを、不生成成功へ読み替えない。"""
        row = _fixture()["cases"][0]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "case"

            def mutation(*args, **kwargs):
                _dangling_entry(root / "report.json", root / "missing")
                return _answer(row)

            try:
                with (
                    patch.object(recorder.subprocess, "run", side_effect=mutation),
                    self.assertRaises(ValueError),
                ):
                    recorder.record("ruby", row, root)
            finally:
                _remove_entry(root / "report.json")

    def test_failure_never_publishes_and_exclusive_save_keeps_existing_bytes(self):
        """runtime後の観測失敗でも公開せず、exclusive保存は既存fileを守る。"""
        runtime = SimpleNamespace(
            returncode=0,
            stderr=b"",
            stdout=json.dumps(
                {"primary": recorder.PRIMARY, "source_sha256": recorder.SOURCES}
            ).encode(),
        )
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            output = base / "output.json"
            with (
                patch.object(recorder.subprocess, "run", return_value=runtime),
                patch.object(recorder, "record", side_effect=ValueError("failed observation")),
                self.assertRaises(ValueError),
            ):
                recorder.collect("ruby", base / "work", output)
            self.assertFalse(os.path.lexists(output))
            output.write_bytes(b"KEEP")
            with self.assertRaises(FileExistsError):
                recorder._write_json(output, {"unexpected": True})
            self.assertEqual(output.read_bytes(), b"KEEP")


if __name__ == "__main__":
    unittest.main()
