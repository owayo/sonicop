"""NewCops採取の全完了・型・全raw・原本属性と実entry保持を独立に検査する。"""

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

import record_new_cops_validation as recorder

_REAL_RUN = subprocess.run


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
            f'cmd.exe /d /v:off /c mklink /J "{path}" "{target}"', capture_output=True, check=False
        )
        if done.returncode:
            raise ValueError("real dangling junctionを作成できません") from None
    if not os.path.lexists(path) or path.exists():
        raise ValueError("dangling entryの対照が成立していません")


def _remove_entry(path):
    if path.is_symlink():
        path.unlink()
    else:
        path.rmdir()


def _data():
    return recorder._template()


def _collect(ruby, work, output):
    return recorder.collect(ruby, work, output)


def _row(name="AllCops-version"):
    return next(row for row in _data()["cases"] if row["id"] == name)


def _answer(argv, **kwargs):
    if argv[1] == "-e":
        return SimpleNamespace(
            returncode=0, stdout=json.dumps(_data()["runtime"]).encode(), stderr=b""
        )
    row = _row(Path(kwargs["cwd"]).name)
    return SimpleNamespace(
        returncode=row["exit"],
        stdout=bytes.fromhex(row["stdout_hex"]),
        stderr=bytes.fromhex(row["stderr_hex"]),
    )


class NewCopsRecorderContract(unittest.TestCase):
    """正しい全応答へ独立した変化を加え、別errorによる偽成功も拒否する。"""

    def test_fixed_schema_inputs_types_raw_and_all_scopes(self):
        """元64+追加14を固定し、空・順序・bool混同・部分scopeを拒否する。"""
        value = _data()
        recorder.validate_fixture(value)
        self.assertEqual(len(recorder.inputs()), 78)
        for kind in (
            "empty",
            "order",
            "schema",
            "exit",
            "preserved",
            "out",
            "scope",
            "source",
            "input",
            "raw",
        ):
            data = copy.deepcopy(value)
            if kind == "empty":
                data["cases"] = []
            elif kind == "order":
                data["cases"].reverse()
            elif kind == "schema":
                data["schema"] = True
            elif kind == "exit":
                data["cases"][0]["exit"] = False
            elif kind == "preserved":
                data["cases"][0]["inputs_bytes_mtime_preserved"] = 1
            elif kind == "out":
                data["cases"][0]["out_exists"] = 0
            elif kind == "scope":
                data["coverage"]["subset"] = True
            elif kind == "source":
                data["runtime"]["source_sha256"]["lib/rubocop/config_validator.rb"] = "0" * 64
            elif kind == "input":
                data["cases"][0]["files"]["a.rb"] = "changed\n"
            else:
                data["cases"][0]["stdout_hex"] = ""
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                recorder.validate_fixture(data)
        forms = recorder.inputs()
        forms[0]["files"]["a.rb"] = "changed"
        self.assertNotEqual(forms, recorder.inputs())

    def test_report_completion_paths_integer_types_and_identity(self):
        """受理とempty-finishedを別に検査し、欠落・重複・bool件数を拒否する。"""
        for name in ("AllCops-pending", "nested-plain"):
            recorder.validate_case(_row(name))
        for kind in ("empty", "duplicate", "path", "zero", "boolean", "offenses", "identity"):
            row = copy.deepcopy(_row("AllCops-pending"))
            report = json.loads(bytes.fromhex(row["stdout_hex"]))
            if kind == "empty":
                report["files"] = []
            elif kind == "duplicate":
                report["files"] *= 2
            elif kind == "path":
                report["files"][0]["path"] = "other.rb"
            elif kind == "zero":
                report["summary"]["inspected_file_count"] = 0
            elif kind == "boolean":
                report["summary"]["target_file_count"] = True
            elif kind == "offenses":
                report["files"][0]["offenses"] = [{"cop_name": "other"}]
            else:
                report["metadata"]["ruby_engine"] = "sonicop"
            row["stdout_hex"] = json.dumps(report).encode().hex()
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                recorder.validate_case(row)

    def test_complete_collection_keeps_all_raw_and_seeded_output(self):
        """79processの全完了とprivate raw、既存sentinelの属性保持を確認する。"""
        with (
            tempfile.TemporaryDirectory() as temp,
            patch.object(recorder.subprocess, "run", side_effect=_answer) as run,
        ):
            base = Path(temp)
            _collect("ruby", base / "work", base / "out.json")
            self.assertEqual(run.call_count, 79)
            self.assertEqual(json.loads((base / "out.json").read_bytes()), _data())
            self.assertEqual(len(list((base / "work").glob("*/private-run0.json"))), 79)
            raw = json.loads((base / "work/out-before-error/private-run0.json").read_bytes())
            self.assertEqual(raw["report_before"], raw["report_after"])
            self.assertIsNotNone(raw["report_after"])
            self.assertEqual(raw["inputs_before"], raw["inputs_after"])

    def test_source_bytes_mtime_and_mode_are_independent(self):
        """本文だけ・mtimeだけ・modeだけを変え、保持検査の独立性を確認する。"""
        for kind in ("bytes", "mtime", "mode"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                root = base / "AllCops-version"
                source = root / "a.rb"
                mutated = []

                def mutate(argv, **kwargs):
                    answer = _answer(argv, **kwargs)
                    original, info = source.read_bytes(), source.stat()
                    if kind == "bytes":
                        source.write_bytes(original + b"# changed\n")
                        os.utime(source, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif kind == "mtime":
                        os.utime(source, ns=(info.st_atime_ns, info.st_mtime_ns + 2_000_000_000))
                    else:
                        source.chmod(stat.S_IREAD)
                    self.assertEqual(source.read_bytes() == original, kind != "bytes")
                    self.assertEqual(source.stat().st_mtime_ns == info.st_mtime_ns, kind != "mtime")
                    self.assertEqual(source.stat().st_mode == info.st_mode, kind != "mode")
                    mutated.append(True)
                    return answer

                try:
                    with (
                        patch.object(recorder.subprocess, "run", side_effect=mutate),
                        self.assertRaises(ValueError),
                    ):
                        recorder.record_case("ruby", _row(), base / "reference.rb", root)
                    self.assertEqual(mutated, [True])
                finally:
                    if source.exists():
                        source.chmod(stat.S_IREAD | stat.S_IWRITE)

    def test_real_regular_and_dangling_report_entries_for_api_and_cli(self):
        """実entryの作成を観測し、API/CLIの前後それぞれで保持と拒否を確認する。"""
        for phase in ("api", "cli"):
            for timing in ("before", "after"):
                for kind in ("regular", "dangling"):
                    with (
                        self.subTest(phase=phase, timing=timing, kind=kind),
                        tempfile.TemporaryDirectory() as temp,
                    ):
                        base = Path(temp)
                        root = (
                            base / "work" / ("api-runtime" if phase == "api" else "AllCops-version")
                        )
                        entry = root / "report.json"
                        created = []
                        called = []
                        prepare = recorder._prepare

                        def create():
                            if kind == "regular":
                                entry.write_bytes(b"extra")
                            else:
                                _dangling_entry(entry, root.parent / "missing-target")
                            created.append(os.path.lexists(entry))

                        def prepare_extra(path, files):
                            prepare(path, files)
                            if path == root and timing == "before":
                                create()

                        def mutate(argv, **kwargs):
                            answer = _answer(argv, **kwargs)
                            called.append(Path(kwargs["cwd"]))
                            if Path(kwargs["cwd"]) == root and timing == "after":
                                create()
                            return answer

                        try:
                            with (
                                patch.object(recorder, "_prepare", side_effect=prepare_extra),
                                patch.object(recorder.subprocess, "run", side_effect=mutate),
                                self.assertRaises(ValueError),
                            ):
                                _collect("ruby", base / "work", base / "out.json")
                            self.assertEqual(created, [True])
                            self.assertEqual(root in called, timing == "after")
                            self.assertTrue(os.path.lexists(entry))
                            if kind == "regular":
                                self.assertEqual(entry.read_bytes(), b"extra")
                            else:
                                self.assertFalse(entry.exists())
                            self.assertFalse(os.path.lexists(base / "out.json"))
                        finally:
                            if kind == "dangling" and os.path.lexists(entry):
                                _remove_entry(entry)

    def test_existing_output_same_bytes_rewrite_is_not_accepted(self):
        """sentinelの本文が同じでもmtimeが変われば、成功へ数えない。"""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            root = base / "out-before-error"
            observed = []

            def mutate(argv, **kwargs):
                answer = _answer(argv, **kwargs)
                entry = root / "report.json"
                before, info = entry.read_bytes(), entry.stat()
                entry.write_bytes(before)
                os.utime(entry, ns=(info.st_atime_ns, info.st_mtime_ns + 2_000_000_000))
                self.assertEqual(entry.read_bytes(), before)
                self.assertNotEqual(entry.stat().st_mtime_ns, info.st_mtime_ns)
                observed.append(True)
                return answer

            with (
                patch.object(recorder.subprocess, "run", side_effect=mutate),
                self.assertRaises(ValueError),
            ):
                recorder.record_case("ruby", _row("out-before-error"), base / "reference.rb", root)
            self.assertEqual(observed, [True])

    def test_wrong_phase_color_stream_or_raw_is_rejected(self):
        """正しい件数でも別error・ANSI欠落・stream入替・report欠落を拒否する。"""
        for kind in ("error", "color", "swap", "report"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                name = "nested-color" if kind == "report" else "force"

                def mutate(argv, **kwargs):
                    answer = _answer(argv, **kwargs)
                    if kind == "error":
                        answer.stderr = b"other error\r\n"
                    elif kind == "color":
                        answer.stderr = answer.stderr.replace(b"\x1b[31m", b"").replace(
                            b"\x1b[0m", b""
                        )
                    elif kind == "swap":
                        answer.stdout, answer.stderr = answer.stderr, answer.stdout
                    else:
                        answer.stdout = b""
                    return answer

                with (
                    patch.object(recorder.subprocess, "run", side_effect=mutate),
                    self.assertRaises(ValueError),
                ):
                    recorder.record_case("ruby", _row(name), base / "reference.rb", base / name)

    def test_runtime_types_versions_and_sources_fail_before_cli(self):
        """APIの型・版・source・stderrが変われば、一件目で拒否する。"""
        for kind in ("ruby", "gem", "source", "stderr"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)

                def mutate(argv, **kwargs):
                    answer = _answer(argv, **kwargs)
                    value = json.loads(answer.stdout)
                    if kind == "ruby":
                        value["ruby"] = True
                    elif kind == "gem":
                        value["gems"]["rubocop"] = "1.89.1"
                    elif kind == "source":
                        value["source_sha256"]["lib/rubocop/config_loader.rb"] = "0" * 64
                    else:
                        answer.stderr = b"error\n"
                    answer.stdout = json.dumps(value).encode()
                    return answer

                with (
                    patch.object(recorder.subprocess, "run", side_effect=mutate) as run,
                    self.assertRaises(ValueError),
                ):
                    _collect("ruby", base / "work", base / "out.json")
                self.assertEqual(run.call_count, 1)

    def test_partial_failure_keeps_private_raw_without_publishing(self):
        """途中の本家終了値が違えば、private rawだけを残して保存を止める。"""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)

            def fail(argv, **kwargs):
                answer = _answer(argv, **kwargs)
                if Path(kwargs["cwd"]).name == "AllCops-version":
                    answer.returncode = 0
                return answer

            with (
                patch.object(recorder.subprocess, "run", side_effect=fail),
                self.assertRaises(ValueError),
            ):
                _collect("ruby", base / "work", base / "out.json")
            self.assertFalse(os.path.lexists(base / "out.json"))
            self.assertTrue((base / "work/AllCops-version/private-process.json").exists())

    def test_fresh_aliases_ancestors_existing_and_missing_parent(self):
        """保存先の別表記・両方向親子・既存entry・親欠落を実行前に拒否する。"""
        for kind in ("same", "alias", "work-parent", "out-parent", "out", "work", "parent", "case"):
            with (
                self.subTest(kind=kind),
                tempfile.TemporaryDirectory() as temp,
                patch.object(recorder.subprocess, "run") as run,
            ):
                base = Path(temp)
                work, output = base / "work", base / "out.json"
                if kind == "same":
                    output = work
                elif kind == "alias":
                    output = work / "inner" / ".."
                elif kind == "work-parent":
                    output = work / "out.json"
                elif kind == "out-parent":
                    work = output / "work"
                elif kind == "out":
                    output.write_bytes(b"original")
                elif kind == "work":
                    work.mkdir()
                elif kind == "parent":
                    output = base / "missing" / "out.json"
                elif os.name == "nt":
                    output = base / "WORK"
                else:
                    output = work / "."
                with self.assertRaises(ValueError):
                    _collect("ruby", work, output)
                run.assert_not_called()
                if kind == "out":
                    self.assertEqual(output.read_bytes(), b"original")

    def test_real_dangling_work_and_output_are_not_followed(self):
        """実symlink又はjunctionを使い、dangling entryも子起動前に保持する。"""
        for kind in ("work", "output"):
            with (
                self.subTest(kind=kind),
                tempfile.TemporaryDirectory() as temp,
                patch.object(recorder.subprocess, "run") as run,
            ):
                base = Path(temp)
                work, output = base / "work", base / "out.json"
                entry = work if kind == "work" else output
                _dangling_entry(entry, base / "missing-target")
                try:
                    before = entry.lstat()
                    with self.assertRaises(ValueError):
                        _collect("ruby", work, output)
                    run.assert_not_called()
                    self.assertEqual(before.st_mtime_ns, entry.lstat().st_mtime_ns)
                    self.assertTrue(os.path.lexists(entry))
                finally:
                    _remove_entry(entry)

    def test_child_environment_is_a_copy_for_all_seventy_nine_processes(self):
        """注入変数だけを子copyで除き、GEM/PATH/OSと親の値を保持する。"""
        controls = {
            key: "injected"
            for key in (
                "RUBYOPT",
                "RUBYLIB",
                "RUBOCOP_OPTS",
                "RUBOCOP_TARGET_RUBY_VERSION",
                "NO_COLOR",
            )
        }
        controls.update(GEM_PATH="gem-path", PATH="native-path", SYSTEMROOT="windows-root")
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, controls):
            before = dict(os.environ)

            def observe(argv, **kwargs):
                environment = kwargs["env"]
                for key in controls:
                    if key in ("GEM_PATH", "PATH", "SYSTEMROOT"):
                        self.assertEqual(environment[key], controls[key])
                    else:
                        self.assertNotIn(key, environment)
                self.assertEqual(environment["TERM"], "dumb")
                return _answer(argv, **kwargs)

            with patch.object(recorder.subprocess, "run", side_effect=observe) as run:
                _collect("ruby", Path(temp) / "work", Path(temp) / "out.json")
            self.assertEqual(run.call_count, 79)
            self.assertEqual(before, dict(os.environ))

    def test_output_race_preserves_existing_bytes_and_stat(self):
        """全件完了直後に作られたoutもexclusive openで保持する。"""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            output = base / "out.json"
            original_write = recorder._write_json
            observed = []

            def race(path, value):
                if path == output:
                    output.write_bytes(b"concurrent artifact")
                    observed.append(
                        (output.read_bytes(), output.stat().st_mtime_ns, output.stat().st_mode)
                    )
                return original_write(path, value)

            with (
                patch.object(recorder.subprocess, "run", side_effect=_answer),
                patch.object(recorder, "_write_json", side_effect=race),
                self.assertRaises(FileExistsError),
            ):
                _collect("ruby", base / "work", output)
            self.assertEqual(
                observed, [(output.read_bytes(), output.stat().st_mtime_ns, output.stat().st_mode)]
            )


if __name__ == "__main__":
    unittest.main()
