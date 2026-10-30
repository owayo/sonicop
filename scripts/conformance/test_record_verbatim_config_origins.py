"""origin採取の完了・原本・全rawと、11実processのentryを検査する。"""

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

import record_verbatim_config_origins as recorder
from test_record_cache_parallel_preflight import _dangling_entry, _remove_entry


class VerbatimOriginRecorderContract(unittest.TestCase):
    """本家以外の期待値生成や部分成功、保存先の上書きを拒否する。"""

    def test_reference_keeps_types_complete_raw_and_finite_scope(self):
        """入力・型・出典・全raw・比較件数の独立した改変を検出する。"""
        value = recorder.template()
        self.assertEqual(len(value["cases"]), 10)
        self.assertEqual(len(value["api"]), 10)
        self.assertEqual(sum(len(row["files"]) for row in value["cases"]), 64)
        for mutation in ("empty", "api", "type", "source", "raw", "scope"):
            data = copy.deepcopy(value)
            if mutation == "empty":
                data["cases"] = []
            elif mutation == "api":
                data["api"].pop()
            elif mutation == "type":
                data["cases"][0]["exit"] = True
            elif mutation == "source":
                data["source_sha256"]["lib/rubocop/config_loader.rb"] = "0" * 64
            elif mutation == "raw":
                data["cases"][0]["stdout_hex"] = ""
            else:
                data["coverage"]["windows_cli"] = 9
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                recorder.validate(data)

    def test_entry_guards_run_before_any_child_or_platform_gate(self):
        """通常entry・dangling entry・包含・親なしのerror原因を区別する。"""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            sentinel = root / "out.json"
            sentinel.write_bytes(b"kept")
            before = sentinel.stat()
            for work, output, message in (
                (root / "work", sentinel, "保存先entry"),
                (root / "same", root / "same", "保存先が重な"),
                (root / "x", root / "x/out", "保存先が重な"),
                (root / "x/work", root / "x", "保存先が重な"),
                (root / "work", root / "missing/out", "親directory"),
            ):
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaisesRegex(ValueError, message):
                        recorder.collect(Path("unused"), work, output)
                    child.assert_not_called()
            link = root / "dangling-out"
            _dangling_entry(link, root / "absent")
            try:
                with patch.object(recorder.subprocess, "run") as child:
                    with self.assertRaisesRegex(ValueError, "保存先entry"):
                        recorder.collect(Path("unused"), root / "work", link)
                    child.assert_not_called()
            finally:
                _remove_entry(link)
            self.assertEqual(sentinel.read_bytes(), b"kept")
            self.assertEqual(sentinel.stat().st_mtime_ns, before.st_mtime_ns)
            self.assertEqual(sentinel.stat().st_mode, before.st_mode)

    def test_changed_bytes_mtime_or_mode_keep_raw_before_rejection(self):
        """mtimeを戻す本文改変と、mtimeだけ・modeだけの変化を別々に拒否する。"""
        for mutation in ("bytes", "mtime", "mode"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                path = root / "a.rb"
                path.write_bytes(b"x=1\n")
                before = path.stat()

                def answer(*args, **kwargs):
                    if mutation == "bytes":
                        path.write_bytes(b"y=2\n")
                        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
                    elif mutation == "mtime":
                        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns + 10_000_000_000))
                    else:
                        path.chmod(stat.S_IRUSR)
                    return SimpleNamespace(returncode=1, stdout=b"actual\xff", stderr=b"warning")

                try:
                    with patch.object(recorder.subprocess, "run", side_effect=answer):
                        with self.assertRaises(ValueError):
                            recorder._capture(Path("unused"), [], root, {"a.rb": "x=1\n"}, 1)
                    raw = json.loads((root / "private-process.json").read_bytes())
                    self.assertEqual(raw["stdout_hex"], b"actual\xff".hex())
                    observed = json.loads((root / "private-run0.json").read_bytes())
                    self.assertNotEqual(observed["before"], observed["after"])
                finally:
                    path.chmod(before.st_mode)

    def test_partial_timeout_and_wrong_exit_cannot_complete_the_capture(self):
        """timeoutのpartialも元rawへ残し、非期待終了と成功を混ぜない。"""
        for timeout in (False, True):
            with self.subTest(timeout=timeout), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                kwargs = (
                    {
                        "side_effect": subprocess.TimeoutExpired(
                            ["unused"],
                            60,
                            output=b"partial",
                            stderr=b"\x1b[31merror",
                        )
                    }
                    if timeout
                    else {
                        "return_value": SimpleNamespace(
                            returncode=2,
                            stdout=b"partial",
                            stderr=b"\x1b[31merror",
                        )
                    }
                )
                with patch.object(recorder.subprocess, "run", **kwargs):
                    with self.assertRaises(ValueError):
                        recorder._capture(Path("unused"), [], root, {}, 1)
                raw = json.loads((root / "private-process.json").read_bytes())
                self.assertEqual(raw["stdout_hex"], b"partial".hex())
                self.assertEqual(raw["stderr_hex"], b"\x1b[31merror".hex())
                self.assertEqual(raw.get("timed_out", False), timeout)

    def test_new_dangling_tree_is_rejected_without_following_it(self):
        """実symlink/junctionを観測し、新規entryをexistsだけで見落とさない。"""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            link = root / "new-entry"

            def answer(*args, **kwargs):
                _dangling_entry(link, root / "absent")
                return SimpleNamespace(returncode=0, stdout=b"actual", stderr=b"")

            try:
                with patch.object(recorder.subprocess, "run", side_effect=answer):
                    with self.assertRaisesRegex(ValueError, "link/reparse"):
                        recorder._capture(Path("unused"), [], root, {}, 0)
                self.assertTrue((root / "private-process.json").is_file())
            finally:
                if os.path.lexists(link):
                    _remove_entry(link)

    @unittest.skipUnless(os.name == "nt", "実Windowsのbasename APIを使うentry検査")
    def test_complete_entry_runs_eleven_children_and_rejects_independent_corruptions(self):
        """全entryのraw・API・markerと保存raceを独立に壊し、偽の完了を拒否する。"""
        data = recorder.template()
        for mutation in (None, "api", "raw", "marker", "race"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp:
                work, output = Path(temp) / "work", Path(temp) / "out.json"
                calls = []

                def answer(args, *, cwd, **kwargs):
                    root = Path(cwd)
                    calls.append(root.name)
                    bindings = {"CWD": str(root), "CWD_FWD": root.as_posix()}
                    if root.name == "api-runtime":
                        bindings["SHORT_FILE"] = Path(
                            recorder._windows_name(
                                root / "long-configuration-filename.yml",
                                True,
                            )
                        ).name
                        rows = [
                            {key: recorder.bind(value, bindings) for key, value in row.items()}
                            for row in data["api"]
                        ]
                        payload = {
                            "runtime": data["runtime"],
                            "source_sha256": data["source_sha256"],
                            "rows": rows,
                        }
                        if mutation == "api":
                            payload["runtime"] = {**payload["runtime"], "ruby": "wrong"}
                        return SimpleNamespace(
                            returncode=0, stdout=json.dumps(payload).encode(), stderr=b""
                        )
                    row = next(row for row in data["cases"] if row["id"] == root.name)
                    self.assertEqual(
                        args[2:], [recorder.bind(arg, bindings) for arg in row["args"]]
                    )
                    if row["marker_hex"] is not None:
                        (root / "loaded_marker").write_bytes(
                            b"wrong" if mutation == "marker" else bytes.fromhex(row["marker_hex"])
                        )
                    if mutation == "race" and root.name == data["cases"][-1]["id"]:
                        output.write_bytes(b"racing sentinel")
                    return SimpleNamespace(
                        returncode=row["exit"],
                        stdout=bytes.fromhex(row["stdout_hex"])
                        + (b"\n" if mutation == "raw" else b""),
                        stderr=recorder.bind(row["stderr_template"], bindings).encode(),
                    )

                with patch.object(recorder.subprocess, "run", side_effect=answer):
                    if mutation is None:
                        self.assertEqual(recorder.collect(Path("unused"), work, output), data)
                        self.assertEqual(len(calls), 11)
                        self.assertEqual(json.loads(output.read_bytes()), data)
                    else:
                        with self.assertRaises((ValueError, FileExistsError)):
                            recorder.collect(Path("unused"), work, output)
                        self.assertTrue(list(work.glob("*/private-process.json")))
                        if mutation == "race":
                            self.assertEqual(output.read_bytes(), b"racing sentinel")
                        else:
                            self.assertFalse(os.path.lexists(output))


if __name__ == "__main__":
    unittest.main()
