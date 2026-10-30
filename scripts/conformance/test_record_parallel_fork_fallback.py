"""fork通知採取の完了集合・phase・全raw・原本属性と保存先を独立に検査する。"""

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

import record_parallel_fork_fallback as recorder

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


def _collect(ruby, work, output):
    return recorder.record(ruby, output, work)


def _row(name="two-positive"):
    return next(row for row in _data()["cases"] if row["id"] == name)


def _answer(argv, **kwargs):
    data = _data()
    root = Path(kwargs["cwd"])
    if argv[1] == "-e":
        value = {"runtime": data["runtime"], "source_sha256": data["source_sha256"]}
        return SimpleNamespace(returncode=0, stdout=json.dumps(value).encode(), stderr=b"")
    row = _row(root.name)
    expected = row["expected"]
    for name, raw in expected["outputs_hex"].items():
        (root / name).write_bytes(bytes.fromhex(raw))
    stderr = (
        bytes.fromhex(expected["stderr_hex"])
        if expected["stderr_hex"] is not None
        else b"Is a directory @ rb_sysopen - outdir\r\n/public/reference.rb:1\r\n"
    )
    return SimpleNamespace(
        returncode=expected["exit"], stdout=bytes.fromhex(expected["stdout_hex"]), stderr=stderr
    )


class ForkRecorderContract(unittest.TestCase):
    """正しい応答と独立した変化を使い、部分成功や自己整合だけの偽成功を拒否する。"""

    def test_fixed_schema_all_twenty_five_inputs_raw_types_and_scopes(self):
        """欠落・順序・bool/数値・出典・入力・比較scopeの変化を拒否する。"""
        data = _data()
        recorder.validate_fixture(data)
        self.assertEqual(len(recorder.inputs()), 25)
        for kind in ("empty", "order", "schema", "fork", "exit", "source", "input", "raw", "scope"):
            changed = copy.deepcopy(data)
            if kind == "empty":
                changed["cases"] = []
            elif kind == "order":
                changed["cases"].reverse()
            elif kind == "schema":
                changed["schema"] = True
            elif kind == "fork":
                changed["runtime"]["fork_supported"] = 0
            elif kind == "exit":
                changed["cases"][0]["expected"]["exit"] = True
            elif kind == "source":
                changed["source_sha256"]["parallel.rb"] = "0" * 64
            elif kind == "input":
                changed["cases"][0]["files"]["good.rb"] += "# changed\n"
            elif kind == "raw":
                changed["cases"][0]["expected"]["stdout_hex"] = ""
            else:
                changed["coverage"]["subset"] = True
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                recorder.validate_fixture(changed)
        forms = recorder.inputs()
        forms[0]["files"]["good.rb"] = "changed"
        self.assertNotEqual(forms, recorder.inputs())

    def test_all_report_phase_paths_counts_and_integer_types_are_required(self):
        """通常・空・途中終了を区別し、欠落・重複・ゼロ化・bool件数を拒否する。"""
        for name in (
            "two-positive",
            "zero-targets",
            "fail-fast",
            "late-config-error",
            "unknown-selection",
        ):
            recorder.validate(_row(name))
        for kind in ("empty", "duplicate", "missing", "zero", "boolean", "offenses", "identity"):
            row = copy.deepcopy(_row())
            report = json.loads(bytes.fromhex(row["expected"]["stdout_hex"]))
            if kind == "empty":
                report["files"] = []
            elif kind == "duplicate":
                report["files"][1] = report["files"][0]
            elif kind == "missing":
                report["files"].pop()
            elif kind == "zero":
                report["summary"]["inspected_file_count"] = 0
            elif kind == "boolean":
                report["summary"]["offense_count"] = True
            elif kind == "offenses":
                report["files"][0]["offenses"] = []
            else:
                report["metadata"]["ruby_engine"] = "sonicop"
            row["expected"]["stdout_hex"] = json.dumps(report, separators=(",", ":")).encode().hex()
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                recorder.validate(row)

    def test_complete_collection_and_private_io_backtraces_remain_distinct(self):
        """26processだけを完成記録へ保存し、IO二件のbacktraceを公開へ混ぜない。"""
        with (
            tempfile.TemporaryDirectory() as temp,
            patch.object(recorder.subprocess, "run", side_effect=_answer) as run,
        ):
            base = Path(temp)
            _collect("ruby", base / "work", base / "out.json")
            self.assertEqual(run.call_count, 26)
            self.assertEqual(json.loads((base / "out.json").read_bytes()), _data())
            self.assertEqual(len(list((base / "work").glob("*/private-run0.json"))), 26)
            for name in recorder.IO_IDS:
                raw = json.loads((base / "work" / name / "private-run0.json").read_bytes())
                self.assertTrue(bytes.fromhex(raw["stderr_hex"]).startswith(b"Is a directory"))
                self.assertIsNone(_row(name)["expected"]["stderr_hex"])
            before = ((base / "out.json").read_bytes(), (base / "out.json").stat().st_mtime_ns)
            with self.assertRaises(ValueError):
                _collect("ruby", base / "other", base / "out.json")
            self.assertEqual(
                before, ((base / "out.json").read_bytes(), (base / "out.json").stat().st_mtime_ns)
            )
            self.assertEqual(run.call_count, 26)

    def test_source_bytes_mtime_and_mode_are_independent_contracts(self):
        """全streamが正しくても、原本の各属性だけを変えると拒否する。"""
        for kind in ("bytes", "mtime", "mode"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                root = base / "two-positive"
                source = root / "good.rb"

                def mutate(argv, **kwargs):
                    answer = _answer(argv, **kwargs)
                    info = source.stat()
                    if kind == "bytes":
                        source.write_bytes(source.read_bytes() + b"# changed\n")
                        os.utime(source, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif kind == "mtime":
                        os.utime(source, ns=(info.st_atime_ns, info.st_mtime_ns + 2_000_000_000))
                    else:
                        source.chmod(stat.S_IREAD)
                    return answer

                try:
                    with (
                        patch.object(recorder.subprocess, "run", side_effect=mutate),
                        self.assertRaises(ValueError),
                    ):
                        recorder.record_case("ruby", _row(), base / "reference.rb", root)
                finally:
                    if source.exists():
                        source.chmod(stat.S_IREAD | stat.S_IWRITE)

    def test_unexpected_regular_and_dangling_outputs_for_api_and_cli(self):
        """正しいAPI/CLI応答へ加わる通常fileとdangling entryを拒否する。"""
        for phase in ("api", "cli"):
            for kind in ("regular", "dangling"):
                with self.subTest(phase=phase, kind=kind), tempfile.TemporaryDirectory() as temp:
                    base = Path(temp)
                    root = base / "work" / ("api-runtime" if phase == "api" else "two-positive")
                    entry = root / "report.json"
                    created = []

                    def mutate(argv, **kwargs):
                        answer = _answer(argv, **kwargs)
                        if Path(kwargs["cwd"]) == root:
                            if kind == "regular":
                                entry.write_bytes(b"extra")
                            else:
                                _dangling_entry(entry, root.parent / "missing-target")
                            created.append(os.path.lexists(entry))
                        return answer

                    try:
                        with (
                            patch.object(recorder.subprocess, "run", side_effect=mutate),
                            self.assertRaises(ValueError),
                        ):
                            _collect("ruby", base / "work", base / "out.json")
                        self.assertEqual(created, [True])
                        self.assertTrue(os.path.lexists(entry))
                        if kind == "regular":
                            self.assertEqual(entry.read_bytes(), b"extra")
                        else:
                            self.assertFalse(entry.exists())
                        self.assertFalse(os.path.lexists(base / "out.json"))
                    finally:
                        if kind == "dangling" and os.path.lexists(entry):
                            _remove_entry(entry)

    def test_expected_artifact_cannot_become_a_dangling_entry(self):
        """宣言されたFile出力もlstatで検査し、リンク先を読まず拒否する。"""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            root = base / "json-file"
            entry = root / "output.json"
            created = []

            def mutate(argv, **kwargs):
                answer = _answer(argv, **kwargs)
                entry.unlink()
                _dangling_entry(entry, base / "missing-target")
                created.append(os.path.lexists(entry))
                return answer

            try:
                with (
                    patch.object(recorder.subprocess, "run", side_effect=mutate),
                    self.assertRaises(ValueError),
                ):
                    recorder.record_case("ruby", _row("json-file"), base / "reference.rb", root)
                self.assertEqual(created, [True])
                self.assertTrue(os.path.lexists(entry))
                self.assertFalse(entry.exists())
                self.assertFalse((base / "missing-target").exists())
            finally:
                if os.path.lexists(entry):
                    _remove_entry(entry)

    def test_notice_order_stream_and_full_raw_are_not_relaxed(self):
        """同じ通知回数でも後置・stream入替・footer欠落を成功へ数えない。"""
        for kind in ("late", "swap", "footer", "duplicate"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)
                root = base / "progress-stderr"

                def mutate(argv, **kwargs):
                    answer = _answer(argv, **kwargs)
                    if kind == "late":
                        answer.stderr = (
                            answer.stderr.replace(recorder.NOTICE, b"") + recorder.NOTICE
                        )
                    elif kind == "swap":
                        answer.stdout, answer.stderr = answer.stderr, answer.stdout
                    elif kind == "footer":
                        answer.stderr = answer.stderr.split(b"2 files inspected")[0]
                    else:
                        answer.stderr += recorder.NOTICE
                    return answer

                with (
                    patch.object(recorder.subprocess, "run", side_effect=mutate),
                    self.assertRaises(ValueError),
                ):
                    recorder.record_case(
                        "ruby", _row("progress-stderr"), base / "reference.rb", root
                    )

    def test_runtime_api_types_versions_and_sources_are_strict(self):
        """forkのbool/数値混同や版・出典の改変を、CLI実行前に拒否する。"""
        for kind in ("boolean", "version", "source", "stderr"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temp:
                base = Path(temp)

                def mutate(argv, **kwargs):
                    answer = _answer(argv, **kwargs)
                    value = json.loads(answer.stdout)
                    if kind == "boolean":
                        value["runtime"]["fork_supported"] = 0
                    elif kind == "version":
                        value["runtime"]["ruby"] = "4.0.8"
                    elif kind == "source":
                        value["source_sha256"]["parallel.rb"] = "0" * 64
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

    def test_partial_failure_never_publishes_a_complete_fixture(self):
        """途中の本家失敗は私用rawを残し、outを完成させない。"""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)

            def fail(argv, **kwargs):
                answer = _answer(argv, **kwargs)
                if Path(kwargs["cwd"]).name == "two-positive":
                    answer.returncode = 2
                return answer

            with (
                patch.object(recorder.subprocess, "run", side_effect=fail),
                self.assertRaises(ValueError),
            ):
                _collect("ruby", base / "work", base / "out.json")
            self.assertFalse(os.path.lexists(base / "out.json"))
            self.assertTrue((base / "work/two-positive/private-process.json").exists())

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

    def test_child_environment_is_a_copy_for_all_twenty_six_processes(self):
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
            self.assertEqual(run.call_count, 26)
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
