"""pending採取の全field・原本保持・保存先を、正しい応答と独立した改変で検査する。"""

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

import record_pending_cops as recorder

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


def _data():
    return recorder._template()


def _answer(argv, **kwargs):
    data = _data()
    root = Path(kwargs["cwd"])
    if argv[1] == "-e":
        if argv[2] == recorder.SCHEMA_API:
            value = data["primary"]
        elif argv[2] == recorder.VERSION_API:
            value = data["version_api"]
        elif argv[2] == recorder.QUALIFIED_API:
            value = data["qualified_api"]
        else:
            value = next(
                row["values"] for row in data["policy_api"] if root.name == "api-" + row["id"]
            )
        return SimpleNamespace(returncode=0, stdout=json.dumps(value).encode(), stderr=b"")
    case = next(row for row in data["cases"] if row["id"] == root.name)
    return SimpleNamespace(
        returncode=case["exit"],
        stdout=bytes.fromhex(case["stdout_hex"]),
        stderr=bytes.fromhex(case["stderr_hex"]),
    )


class PendingRecorderContract(unittest.TestCase):
    """全応答が自己整合していても、未測定・属性変化・不完全出力を通さない。"""

    def test_fixed_schema_all_raw_runtime_and_finite_counts(self):
        """行の欠落・順序・型・出典・入力・生出力の変更を子起動前に拒否する。"""
        data = _data()
        recorder.validate_fixture(data)
        self.assertEqual(len(recorder.case_inputs()), 36)
        for kind in ("empty", "order", "schema", "type", "source", "input", "raw", "api"):
            broken = copy.deepcopy(data)
            if kind == "empty":
                broken["cases"] = []
            elif kind == "order":
                broken["cases"].reverse()
            elif kind == "schema":
                broken["schema"] = True
            elif kind == "type":
                broken["cases"][0]["exit"] = False
            elif kind == "source":
                broken["version_api"]["version_source_sha256"] = "0" * 64
            elif kind == "input":
                broken["cases"][0]["files"]["a.rb"] += "# changed\n"
            elif kind == "raw":
                broken["cases"][0]["stderr_hex"] = ""
            else:
                broken["qualified_api"]["rows"].pop()
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                recorder.validate_fixture(broken)
        forms = recorder.case_inputs()
        forms[0]["files"]["a.rb"] = "changed"
        self.assertNotEqual(forms, recorder.case_inputs())

    def test_complete_collection_is_exact_and_exclusive(self):
        """36CLIと8APIの全観測だけを保存し、既存artifactを保持する。"""
        with (
            tempfile.TemporaryDirectory() as temp,
            patch.object(recorder.subprocess, "run", side_effect=_answer) as run,
        ):
            base = Path(temp)
            work, output = base / "work", base / "out.json"
            recorder.collect("ruby", work, output)
            self.assertEqual(run.call_count, 44)
            self.assertEqual(json.loads(output.read_bytes()), _data())
            self.assertEqual(len(list(work.glob("*/private-run0.json"))), 44)
            before = (output.read_bytes(), output.stat().st_mtime_ns, output.stat().st_mode)
            with self.assertRaises(ValueError):
                recorder.collect("ruby", base / "other", output)
            self.assertEqual(
                before, (output.read_bytes(), output.stat().st_mtime_ns, output.stat().st_mode)
            )
            self.assertEqual(run.call_count, 44)

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
                    recorder.collect("ruby", work, output)
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
                        recorder.collect("ruby", work, output)
                    run.assert_not_called()
                    self.assertEqual(before.st_mtime_ns, entry.lstat().st_mtime_ns)
                    self.assertTrue(os.path.lexists(entry))
                finally:
                    _remove_entry(entry)

    def test_each_input_attribute_is_independently_required_for_cli_and_api(self):
        """全streamが正しくても、bytes・同byte再書込mtime・modeの各変化を拒否する。"""
        for phase in ("cli", "api"):
            for kind in ("bytes", "mtime", "mode"):
                with self.subTest(phase=phase, kind=kind), tempfile.TemporaryDirectory() as temp:
                    base = Path(temp)
                    case = _data()["cases"][0]
                    root = base / case["id"] if phase == "cli" else base / "api-version"
                    changed = root / ("a.rb" if phase == "cli" else "config.yml")

                    def mutate(argv, **kwargs):
                        answer = _answer(argv, **kwargs)
                        info = changed.stat()
                        if kind == "bytes":
                            changed.write_bytes(changed.read_bytes() + b"# changed\n")
                        elif kind == "mtime":
                            os.utime(
                                changed, ns=(info.st_atime_ns, info.st_mtime_ns + 2_000_000_000)
                            )
                        else:
                            changed.chmod(stat.S_IREAD)
                        return answer

                    try:
                        with (
                            patch.object(recorder.subprocess, "run", side_effect=mutate),
                            self.assertRaises(ValueError),
                        ):
                            if phase == "cli":
                                recorder._record("ruby", case, base / "reference.rb", root)
                            else:
                                recorder._api(
                                    "ruby",
                                    recorder.VERSION_API,
                                    root,
                                    {"config.yml": "test\n"},
                                    _data()["version_api"],
                                )
                    finally:
                        if changed.exists():
                            changed.chmod(stat.S_IREAD | stat.S_IWRITE)

    def test_new_dangling_output_is_rejected_for_cli_and_api(self):
        """正しいCLI/API応答に加わる新規dangling entryを読まずに拒否する。"""
        for phase in ("cli", "api"):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                case = _data()["cases"][0]
                root = base / case["id"] if phase == "cli" else base / "api-version"
                entry = root / "report.json"
                target = base / "missing-target"

                def mutate(argv, **kwargs):
                    answer = _answer(argv, **kwargs)
                    _dangling_entry(entry, target)
                    return answer

                try:
                    with (
                        patch.object(recorder.subprocess, "run", side_effect=mutate),
                        self.assertRaises(ValueError),
                    ):
                        if phase == "cli":
                            recorder._record("ruby", case, base / "reference.rb", root)
                        else:
                            recorder._api(
                                "ruby", recorder.VERSION_API, root, {}, _data()["version_api"]
                            )
                    self.assertFalse(target.exists())
                    self.assertTrue(os.path.lexists(entry))
                finally:
                    if os.path.lexists(entry):
                        _remove_entry(entry)

    def test_new_regular_output_is_rejected(self):
        """想定しない通常Fileの生成も、stdoutだけの成功へ読み替えない。"""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            case = _data()["cases"][0]
            root = base / case["id"]

            def mutate(argv, **kwargs):
                answer = _answer(argv, **kwargs)
                (root / "report.json").write_bytes(b"extra")
                return answer

            with (
                patch.object(recorder.subprocess, "run", side_effect=mutate),
                self.assertRaises(ValueError),
            ):
                recorder._record("ruby", case, base / "reference.rb", root)

    def test_raw_stream_color_footer_and_completion_cannot_change_together(self):
        """通知の色とfooterの同時改変、stream入替、JSON未完了を拒否する。"""
        for kind in ("color-footer", "swap", "incomplete", "exit"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                case = next(row for row in _data()["cases"] if row["id"] == "force-color")

                def mutate(argv, **kwargs):
                    answer = _answer(argv, **kwargs)
                    if kind == "color-footer":
                        answer.stderr = answer.stderr.replace(b"\x1b[33m", b"\x1b[31m").replace(
                            b"For more information:", b"Changed information:"
                        )
                    elif kind == "swap":
                        answer.stdout, answer.stderr = answer.stderr, answer.stdout
                    elif kind == "incomplete":
                        answer.stdout = b'{"summary":{"inspected_file_count":0}}'
                    else:
                        answer.returncode = 2
                    return answer

                with (
                    patch.object(recorder.subprocess, "run", side_effect=mutate),
                    self.assertRaises(ValueError),
                ):
                    recorder._record("ruby", case, base / "reference.rb", base / case["id"])

    def test_api_runtime_boolean_and_source_types_are_strict(self):
        """同値に見えるbool/数値やruntime/sourceの改変も全fieldで拒否する。"""
        for kind in ("boolean", "version", "source", "stderr"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)

                def mutate(argv, **kwargs):
                    answer = _answer(argv, **kwargs)
                    value = json.loads(answer.stdout)
                    if kind == "boolean":
                        value["raw_unchanged"] = 1
                    elif kind == "version":
                        value["ruby"] = "4.0.8"
                    elif kind == "source":
                        value["sources"]["config/default.yml"] = "0" * 64
                    else:
                        answer.stderr = b"error\n"
                    answer.stdout = json.dumps(value).encode()
                    return answer

                with (
                    patch.object(recorder.subprocess, "run", side_effect=mutate),
                    self.assertRaises(ValueError),
                ):
                    recorder._api(
                        "ruby", recorder.SCHEMA_API, base / "api-schema", {}, _data()["primary"]
                    )

    def test_child_environment_is_a_copy_for_all_forty_four_processes(self):
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
                recorder.collect("ruby", Path(temp) / "work", Path(temp) / "out.json")
            self.assertEqual(run.call_count, 44)
            self.assertEqual(before, dict(os.environ))

    def test_partial_collection_never_publishes(self):
        """途中の本家失敗は私用観測だけを残し、完了fixtureを作らない。"""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)

            def fail(argv, **kwargs):
                answer = _answer(argv, **kwargs)
                if Path(kwargs["cwd"]).name == "normal-enable":
                    answer.returncode = 2
                return answer

            with (
                patch.object(recorder.subprocess, "run", side_effect=fail),
                self.assertRaises(ValueError),
            ):
                recorder.collect("ruby", base / "work", base / "out.json")
            self.assertFalse(os.path.lexists(base / "out.json"))

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
                recorder.collect("ruby", base / "work", output)
            self.assertEqual(
                observed, [(output.read_bytes(), output.stat().st_mtime_ns, output.stat().st_mode)]
            )


if __name__ == "__main__":
    unittest.main()
