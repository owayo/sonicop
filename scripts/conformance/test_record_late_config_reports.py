"""途中reportの成功prefixと失敗・保存先・原本guardを単独負対照で検査する。"""

import copy
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_late_config_reports as recorder


def _raw(data, work):
    value = bytes.fromhex(data).replace(b"<WORKDIR>/", work.as_posix().encode() + b"/")
    metadata = (
        b'"metadata":{"rubocop_version":"1.89.0","ruby_engine":"ruby","ruby_version":"4.0.7"},'
    )
    return value.replace(b'{"files":', b"{" + metadata + b'"files":')


def _child(form, work, *, mutation=None):
    row = next(row for row in recorder._template()["cases"] if row["id"] == form["id"])
    expected = row["expected"]
    for name, content in {**expected["files_after"], **expected["outputs"]}.items():
        if content is not None:
            path = work / name
            data = _raw(content, work) if name in expected["outputs"] else bytes.fromhex(content)
            if not path.is_file() or path.read_bytes() != data:
                path.write_bytes(data)
    if mutation == "bytes":
        source = work / "good.rb"
        before = source.stat()
        source.write_bytes(b"q=1\n")
        os.utime(source, ns=(before.st_atime_ns, before.st_mtime_ns))
    elif mutation == "mtime":
        source = work / "good.rb"
        before = source.stat()
        os.utime(source, ns=(before.st_atime_ns, before.st_mtime_ns + 1_000_000_000))
    elif mutation == "out":
        (work / "report.json").write_bytes(b"wrong")
    stdout = _raw(expected["stdout_hex"], work)
    stderr = _raw(expected["stderr_hex"], work)
    if mutation == "warning":
        stderr = stderr.replace(
            b"nested/.rubocop.yml: Warning: no department given for MultipleComparison.\r\n", b""
        )
    elif mutation == "message":
        stderr = stderr.replace(
            b"Lint/MultipleComparison or Style/MultipleComparison",
            b"Style/MultipleComparison or Lint/MultipleComparison",
        )
    elif mutation == "offense":
        document = json.loads(stdout)
        document["files"][0]["offenses"] = []
        document["summary"]["offense_count"] = 0
        stdout = json.dumps(document, separators=(",", ":")).encode()
    elif mutation == "exit":
        return SimpleNamespace(returncode=True, stdout=stdout, stderr=stderr)
    return SimpleNamespace(returncode=expected["exit"], stdout=stdout, stderr=stderr)


class LateConfigReportsContract(unittest.TestCase):
    """実本家rawの独立channelを守り、候補を仕様に混ぜない。"""

    def test_fixed_inputs_scope_types_and_original_raw(self):
        """空・重複・欠落・別scope・bool終了値を拒み、38全ID順を固定する。"""
        data = recorder._template()
        recorder.validate_fixture(data)
        self.assertEqual(len(recorder.case_inputs()), 38)
        for change in ("empty", "duplicate", "order", "schema", "scope", "exit", "raw"):
            broken = copy.deepcopy(data)
            if change == "empty":
                broken["cases"] = []
            elif change == "duplicate":
                broken["cases"][-1] = broken["cases"][0]
            elif change == "order":
                broken["cases"].reverse()
            elif change == "schema":
                broken["schema"] = True
            elif change == "scope":
                broken["cases"][0]["coverage"] = "deferred_158"
            elif change == "exit":
                broken["cases"][0]["expected"]["exit"] = True
            else:
                broken["cases"][0]["expected"]["raw_stderr_sha256"] = "wrong"
            with self.assertRaises(ValueError):
                recorder.validate_fixture(broken)

    def test_primary_query_checks_failure_before_parsing_and_passes_source_keys(self):
        """失敗したRuby照会の空stdoutをJSONとして読み進めず、sourceはdata argvで渡す。"""
        with tempfile.TemporaryDirectory() as folder:
            for child in (
                SimpleNamespace(returncode=2, stdout=b"", stderr=b"failed"),
                SimpleNamespace(returncode=True, stdout=b"", stderr=b""),
            ):
                with (
                    patch.object(recorder.subprocess, "run", return_value=child),
                    self.assertRaises(ValueError),
                ):
                    recorder._runtime("ruby", Path(folder))
            value = {
                "ruby": "4.0.7",
                "versions": recorder.VERSIONS,
                "sources": recorder._template()["sources"],
            }
            with patch.object(
                recorder.subprocess,
                "run",
                return_value=SimpleNamespace(
                    returncode=0, stdout=json.dumps(value).encode(), stderr=b""
                ),
            ) as run:
                recorder._runtime("ruby", Path(folder))
                self.assertEqual(json.loads(run.call_args.args[0][-1]), list(value["sources"]))

    def test_record_preserves_child_environment_and_measured_rows(self):
        """ENV4とNO_COLORだけを子copyから除き、親とGEM/OSを保持する。"""
        form = recorder.case_inputs()[1]
        with tempfile.TemporaryDirectory() as folder:
            work = Path(folder) / "case"
            before = dict(os.environ)
            with patch.object(
                recorder.subprocess, "run", side_effect=lambda *a, **kw: _child(form, work)
            ) as run:
                row = recorder.record("ruby", form, work)
            self.assertEqual(dict(os.environ), before)
            child = run.call_args.kwargs["env"]
            self.assertEqual(child, recorder._environment(form["env"]))
            self.assertEqual(
                recorder._comparable(row["expected"]),
                recorder._comparable(recorder._template()["cases"][1]["expected"]),
            )
            self.assertTrue((work / "private-raw.json").is_file())

    def test_source_bytes_and_mtime_fail_independently(self):
        """本文変更はmtimeを戻し、mtime変更は本文を保った正しいchildで拒む。"""
        form = recorder.case_inputs()[1]
        for mutation in ("bytes", "mtime"):
            with tempfile.TemporaryDirectory() as folder:
                work = Path(folder) / "case"
                with patch.object(
                    recorder.subprocess,
                    "run",
                    side_effect=lambda *a, **kw: _child(form, work, mutation=mutation),
                ):
                    with self.assertRaises(ValueError):
                        recorder.record("ruby", form, work)

    def test_output_warning_message_exit_and_offense_guards_are_independent(self):
        """他channelを正しく保ち、out/警告/候補順/型/全offenseの各単独破壊を拒む。"""
        for mutation in ("out", "warning", "message", "exit", "offense"):
            form = next(
                form
                for form in recorder.case_inputs()
                if form["id"] == ("json-out" if mutation == "out" else "prefix-ambiguous")
            )
            with tempfile.TemporaryDirectory() as folder:
                work = Path(folder) / "case"
                with patch.object(
                    recorder.subprocess,
                    "run",
                    side_effect=lambda *a, **kw: _child(form, work, mutation=mutation),
                ):
                    with self.assertRaises(ValueError):
                        recorder.record("ruby", form, work)

    def test_completion_requires_actual_prefix_and_integer_counts(self):
        """failed-firstの0完了と正のprefixを区別し、bool件数や偽完了を拒む。"""
        form = recorder.case_inputs()[1]
        reference = recorder._template()["cases"][1]["expected"]
        report = json.loads(_raw(reference["stdout_hex"], Path("work")))
        for field in ("offense_count", "target_file_count", "inspected_file_count"):
            broken = copy.deepcopy(report)
            broken["summary"][field] = True
            with self.assertRaises(ValueError):
                recorder.validate_completion(
                    form, json.dumps(broken, separators=(",", ":")).encode(), b"", []
                )
        for paths in ([], report["files"] * 2):
            broken = copy.deepcopy(report)
            broken["files"] = paths
            with self.assertRaises(ValueError):
                recorder.validate_completion(
                    form, json.dumps(broken, separators=(",", ":")).encode(), b"", []
                )
        with self.assertRaises(ValueError):
            recorder.validate_completion(form, b"", b"", [])
        early = next(form for form in recorder.case_inputs() if form["id"] == "initial")
        with self.assertRaises(ValueError):
            recorder.validate_completion(
                early, json.dumps(report, separators=(",", ":")).encode(), b"", []
            )
        broken = copy.deepcopy(report)
        broken["metadata"]["ruby_engine"] = "sonicop"
        with self.assertRaises(ValueError):
            recorder.validate_completion(
                form, json.dumps(broken, separators=(",", ":")).encode(), b"", []
            )

    def test_unknown_or_changed_input_is_rejected_before_process(self):
        """IDを保ったargs差も採取前に拒み、未知条件を成功へ数えない。"""
        for field, value in (("id", "unknown"), ("args", [])):
            form = copy.deepcopy(recorder.case_inputs()[0])
            form[field] = value
            with (
                tempfile.TemporaryDirectory() as folder,
                patch.object(recorder.subprocess, "run") as run,
            ):
                with self.assertRaises(ValueError):
                    recorder.record("ruby", form, Path(folder) / "case")
                run.assert_not_called()

    def test_fresh_paths_precede_process_and_preserve_existing_entry(self):
        """既存entryと別表記aliasを独立に拒み、既存本文を変更しない。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            work, output = root / "work", root / "output"
            output.write_bytes(b"KEEP")
            args = ["record", "--ruby", "ruby", "--work-dir", str(work), "--output", str(output)]
            with patch.object(sys, "argv", args), patch.object(recorder.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    recorder.main()
                run.assert_not_called()
            self.assertEqual(output.read_bytes(), b"KEEP")
            output.unlink()
            for alias in (root / "sub" / ".." / "work", root / "WORK" if os.name == "nt" else work):
                self.assertFalse(os.path.lexists(work))
                self.assertFalse(os.path.lexists(alias))
                with patch.object(recorder.subprocess, "run") as run:
                    with self.assertRaises(ValueError):
                        recorder._fresh_paths(work, alias)
                    run.assert_not_called()

    def test_dangling_entries_and_exclusive_save_are_preserved(self):
        """danglingを存在扱いし、排他openが既存成果を上書きしないことを確認する。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            path = root / "dangling"
            try:
                path.symlink_to(root / "missing")
            except OSError:
                # symlink権限が無いhostでもlexistsの独立分岐は子起動前に検査する。
                real = os.path.lexists
                with patch.object(
                    recorder.os.path,
                    "lexists",
                    side_effect=lambda item: Path(item) == path or real(item),
                ):
                    with self.assertRaises(ValueError):
                        recorder._fresh_paths(root / "work", path)
            else:
                with self.assertRaises(ValueError):
                    recorder._fresh_paths(root / "work", path)
                path.unlink()
            path.write_bytes(b"KEEP")
            with self.assertRaises(FileExistsError):
                with path.open("x"):
                    pass
            self.assertEqual(path.read_bytes(), b"KEEP")

    def test_normal_correction_and_early_outputs_are_real_rows(self):
        """補正済み本文とsentinel保持、空finishedを本家の別条件で照合する。"""
        for name in ("normal-correct", "single", "initial-out", "json-out", "stdin-directory-a"):
            form = next(form for form in recorder.case_inputs() if form["id"] == name)
            with tempfile.TemporaryDirectory() as folder:
                work = Path(folder) / "case"
                with patch.object(
                    recorder.subprocess, "run", side_effect=lambda *a, **kw: _child(form, work)
                ):
                    recorder.record("ruby", form, work)


if __name__ == "__main__":
    unittest.main()
