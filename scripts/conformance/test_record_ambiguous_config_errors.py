"""曖昧名の本家採取で、未完了・誤phase・原本変更を偽成功にしない。"""

import copy
import json
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_ambiguous_config_errors as recorder


def fixture():
    """候補を呼ばず、実RuboCop23条件の保存値を対照にする。

    Returns:
        dict: 元rawと固定入力を保つ参照fixture。

    """
    return json.loads(
        (
            Path(__file__).resolve().parents[2] / "tests/fixtures/ambiguous_config_errors.json"
        ).read_bytes()
    )


class ReferenceContract(unittest.TestCase):
    """20strictと3後段JSON残差を分離し、各失敗ガードを単独で検査する。"""

    def test_environment_copy_removes_only_controlled_variables(self):
        """共用ENV4とambient色だけを除き、親/GEM/PATH/OSを保つ。"""
        parent = {
            "rubyopt": "bad",
            "RuByLiB": "bad",
            "RUBOCOP_OPTS": "bad",
            "RUBOCOP_TARGET_RUBY_VERSION": "bad",
            "NO_COLOR": "ambient",
            "GEM_HOME": "gem",
            "GEM_PATH": "paths",
            "PATH": "bin",
            "SystemRoot": "os",
            "KEEP_SENTINEL": "keep",
        }
        with patch.dict(os.environ, parent, clear=True):
            before = dict(os.environ)
            actual = recorder.child_environment({"CLICOLOR_FORCE": "1"})
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
            self.assertEqual(actual, {**expected, "CLICOLOR_FORCE": "1"})
            self.assertEqual(dict(os.environ), before)

    def test_fixture_requires_fixed_inputs_ids_types_and_coverage(self):
        """23ID/schema/int/boolを固定し、後段3の比較範囲を別行へ移さない。"""
        data = fixture()
        recorder.validate_fixture(data)
        for key, value in [("schema", True), ("cases", []), ("sources", {}), ("runtime", {})]:
            changed = copy.deepcopy(data)
            changed[key] = value
            with self.assertRaises(ValueError):
                recorder.validate_fixture(changed)
        for key, value in [
            ("exit", True),
            ("stdout_json", 0),
            ("id", "unknown"),
            ("coverage", "deferred_152"),
            ("args", []),
            ("env", {}),
        ]:
            changed = copy.deepcopy(data)
            changed["cases"][0][key] = value
            with self.assertRaises(ValueError):
                recorder.validate_fixture(changed)
        for cases in (
            data["cases"][:-1],
            list(reversed(data["cases"])),
            data["cases"][:-1] + [data["cases"][0]],
        ):
            with self.assertRaises(ValueError):
                recorder.validate_fixture({**data, "cases": cases})

    def test_native_color_ending_and_namespace_order_cannot_be_erased(self):
        """最終CRLFをLFや逆phase色へ替えた元出力を拒む。"""
        data = fixture()
        for index in (0, 5, 10):
            changed = copy.deepcopy(data)
            changed["cases"][index]["stderr_hex"] = (
                bytes.fromhex(changed["cases"][index]["stderr_hex"]).replace(b"\r\n", b"\n").hex()
            )
            with self.assertRaises(ValueError):
                recorder.validate_fixture(changed)
        for name in ("force-color", "nested-color"):
            changed = copy.deepcopy(data)
            row = next(row for row in changed["cases"] if row["id"] == name)
            row["stderr_hex"] = recorder.ANSI.sub(b"", bytes.fromhex(row["stderr_hex"])).hex()
            with self.assertRaises(ValueError):
                recorder.validate_fixture(changed)
        changed = copy.deepcopy(data)
        row = changed["cases"][0]
        row["stderr_hex"] = ("Error: " + row["message"] + "\r\n").encode().hex()
        with self.assertRaises(ValueError):
            recorder.validate_fixture(changed)

    def test_message_and_origin_cannot_change_together_with_the_raw(self):
        """message/rawの自己整合性だけで候補順やorigin変更を採用しない。"""
        for old, new in [
            (
                "Lint/MultipleComparison or Style/MultipleComparison",
                "Style/MultipleComparison or Lint/MultipleComparison",
            ),
            ("<root>/config.yml", "<root>/wrong.yml"),
        ]:
            changed = fixture()
            row = changed["cases"][0]
            row["message"] = row["message"].replace(old, new)
            row["stderr_hex"] = (
                bytes.fromhex(row["stderr_hex"]).replace(old.encode(), new.encode()).hex()
            )
            with self.assertRaises(ValueError):
                recorder.validate_fixture(changed)

    def test_json_completion_identity_all_offenses_and_count_types_are_strict(self):
        """正常と後段0件を混ぜず、bool件数や診断の隠蔽を拒む。"""
        data = fixture()
        for name, inspected in (("canonical", 1), ("nested-color", 0)):
            row = next(row for row in data["cases"] if row["id"] == name)
            report = json.loads(bytes.fromhex(row["stdout_hex"]))
            recorder._report(json.dumps(report).encode(), inspected)
            for key, value in [
                ("offense_count", False),
                ("target_file_count", True),
                ("inspected_file_count", bool(inspected)),
                ("inspected_file_count", 9),
            ]:
                changed = copy.deepcopy(report)
                changed["summary"][key] = value
                with self.assertRaises(ValueError):
                    recorder._report(json.dumps(changed).encode(), inspected)
            for files in (
                report["files"] * 2 if inspected else [{"path": "a.rb", "offenses": []}],
                [{"path": "a.rb", "offenses": [{"message": "hidden"}]}],
            ):
                changed = copy.deepcopy(report)
                changed["files"] = files
                with self.assertRaises(ValueError):
                    recorder._report(json.dumps(changed).encode(), inspected)
            changed = copy.deepcopy(report)
            changed["metadata"]["ruby_engine"] = "sonicop"
            with self.assertRaises(ValueError):
                recorder._report(json.dumps(changed).encode(), inspected)

    def test_record_keeps_real_fields_and_originals(self):
        """本家の正しい出力だけを使い、origin投影以外のfieldを変えない。"""
        data = fixture()
        for name in ("multiple", "canonical", "nested-color", "nested-cwd", "out-sentinel"):
            row = next(row for row in data["cases"] if row["id"] == name)
            form = next(form for form in recorder.case_inputs() if form["id"] == name)

            def child(*args, row=row, form=form, **kwargs):
                root = kwargs["cwd"] if form["cwd"] == "." else kwargs["cwd"].parent
                root = root.resolve() if sys.platform == "darwin" else root
                stderr = bytes.fromhex(row["stderr_hex"]).replace(
                    b"<root>", root.as_posix().encode()
                )
                return SimpleNamespace(
                    returncode=row["exit"], stdout=bytes.fromhex(row["stdout_hex"]), stderr=stderr
                )

            with patch.object(recorder.subprocess, "run", side_effect=child):
                self.assertEqual(recorder.record("ruby", form), row)

    def test_origin_projection_only_changes_the_two_measured_positions(self):
        """nested-cwdの先頭warningとerror以外にある同一文字列を消さない。"""
        row = next(row for row in fixture()["cases"] if row["id"] == "nested-cwd")
        form = next(form for form in recorder.case_inputs() if form["id"] == "nested-cwd")
        root = Path("C:/controlled/project") if os.name == "nt" else Path("/controlled/project")
        expected = bytes.fromhex(row["stderr_hex"]).decode()
        raw = expected.replace("<root>", root.as_posix()).encode()
        suffix = f"literal {root.as_posix()}/unchanged\n"
        self.assertEqual(
            recorder._public_stderr(raw + suffix.encode(), root, form), expected + suffix
        )
        wrong = raw.replace(
            (root.as_posix() + "/config.yml: Warning:").encode(), b"wrong.yml: Warning:"
        )
        with self.assertRaises(ValueError):
            recorder._public_stderr(wrong, root, form)

    def test_bytes_mtime_and_unexpected_out_fail_independently(self):
        """他channelを正しいまま保ち、本文単独/mtime単独/out単独の変更を拒む。"""
        row = next(row for row in fixture()["cases"] if row["id"] == "multiple")
        form = recorder.case_inputs()[0]
        for kind in ("bytes", "mtime", "out"):

            def child(*args, kind=kind, **kwargs):
                root = kwargs["cwd"]
                path = root / "a.rb"
                status = path.stat()
                if kind == "bytes":
                    path.write_bytes(b"y=2\n")
                    os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns))
                    self.assertEqual(path.stat().st_mtime_ns, status.st_mtime_ns)
                elif kind == "mtime":
                    os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns + 2_000_000_000))
                else:
                    (root / "report.json").write_bytes(b"unexpected")
                return SimpleNamespace(
                    returncode=row["exit"],
                    stdout=bytes.fromhex(row["stdout_hex"]),
                    stderr=bytes.fromhex(row["stderr_hex"]).replace(
                        b"<root>", root.as_posix().encode()
                    ),
                )

            with patch.object(recorder.subprocess, "run", side_effect=child):
                with self.assertRaises(ValueError):
                    recorder.record("ruby", form)

    def test_existing_out_is_included_in_the_bytes_and_mtime_inventory(self):
        """初期拒否前の既存outも、mtimeを戻した本文変更から守る。"""
        row = next(row for row in fixture()["cases"] if row["id"] == "out-sentinel")
        form = next(form for form in recorder.case_inputs() if form["id"] == "out-sentinel")

        def child(*args, **kwargs):
            root = kwargs["cwd"]
            path = root / "report.json"
            status = path.stat()
            path.write_bytes(b"changed!\n")
            os.utime(path, ns=(status.st_atime_ns, status.st_mtime_ns))
            self.assertEqual(path.stat().st_mtime_ns, status.st_mtime_ns)
            return SimpleNamespace(
                returncode=row["exit"],
                stdout=bytes.fromhex(row["stdout_hex"]),
                stderr=bytes.fromhex(row["stderr_hex"]).replace(
                    b"<root>", root.as_posix().encode()
                ),
            )

        with patch.object(recorder.subprocess, "run", side_effect=child):
            with self.assertRaises(ValueError):
                recorder.record("ruby", form)

    def test_fresh_alias_dangling_and_missing_input_guards_precede_child_start(self):
        """既存entryとaliasを独立に検査し、空行列でも本家を起動しない。"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = (root / "work", root / "out.json")
            original_lstat = os.lstat

            def invoke(destinations):
                work, output = destinations
                with patch.object(
                    sys,
                    "argv",
                    ["record", "--ruby", "ruby", "--work-dir", str(work), "--output", str(output)],
                ):
                    recorder.main()

            for target in paths:

                def _lstat(path, *args, target=target, **kwargs):
                    if Path(path) == target:
                        return SimpleNamespace(st_mode=stat.S_IFLNK)
                    return original_lstat(path, *args, **kwargs)

                with patch("os.lstat", side_effect=_lstat):
                    with self.assertRaises(ValueError):
                        recorder._fresh_paths(*paths)
                target.write_bytes(b"KEEP")
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaises(ValueError):
                        invoke(paths)
                    child.assert_not_called()
                self.assertEqual(target.read_bytes(), b"KEEP")
                target.unlink()
            aliases = [(root / "absent" / ".." / "same", root / "same")]
            if os.name == "nt":
                aliases.append((root / "Same", root / "same"))
            for destinations in aliases:
                self.assertTrue(all(not os.path.lexists(path) for path in destinations))
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaises(ValueError):
                        invoke(destinations)
                    child.assert_not_called()
            with (
                patch.object(recorder, "case_inputs", return_value=[]),
                patch.object(recorder.subprocess, "run") as child,
            ):
                with self.assertRaises(ValueError):
                    invoke(paths)
                child.assert_not_called()


if __name__ == "__main__":
    unittest.main()
