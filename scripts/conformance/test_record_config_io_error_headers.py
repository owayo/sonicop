"""設定I/O採取の実lock、全entry、属性・raw・保存先の拒否を検査する。"""

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

import record_config_io_error_headers as recorder
from test_record_cache_parallel_preflight import _dangling_entry, _remove_entry


class ConfigIoRecorderContract(unittest.TestCase):
    """先頭行の一致と内部frameの参照保持を分け、不完全な採取を拒否する。"""

    def test_reference_keeps_complete_raw_types_api_and_scope(self):
        """API・raw・frame・型・原本・比較範囲の独立改変を拒否する。"""
        value = recorder.template()
        self.assertEqual(len(value["api"]), 5)
        self.assertEqual(len(value["cases"]), 10)
        self.assertEqual(sum(len(row["files"]) for row in value["cases"]), 49)
        for mutation in ("empty", "api", "type", "frame", "source", "scope", "out"):
            data = copy.deepcopy(value)
            if mutation == "empty":
                data["cases"] = []
            elif mutation == "api":
                data["api"].pop()
            elif mutation == "type":
                data["cases"][0]["exit"] = True
            elif mutation == "frame":
                data["cases"][0]["stderr_template"] = data["cases"][0]["first_line_template"]
            elif mutation == "source":
                data["source_sha256"]["cli.rb"] = "0" * 64
            elif mutation == "scope":
                data["coverage"]["ruby_backtrace_reference_only"] = False
            else:
                data["cases"][0]["out_hex"] = ""
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                recorder.validate(data)

    def test_existing_nested_parentless_and_dangling_entries_stop_before_child(self):
        """保存先の原因を明示し、別OS拒否をguard成功の代用にしない。"""
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

    def test_each_original_and_directory_attribute_change_keeps_raw_then_fails(self):
        """本文だけ・mtimeだけ・modeだけ・directory mtimeの変化を独立に拒否する。"""
        for mutation in ("bytes", "mtime", "mode", "directory"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                path = root / "a.rb"
                path.write_bytes(b"x=1\n")
                directory = root / "configdir"
                directory.mkdir()
                info, dir_info = path.stat(), directory.stat()

                def answer(
                    *args,
                    mutation=mutation,
                    path=path,
                    info=info,
                    directory=directory,
                    dir_info=dir_info,
                    **kwargs,
                ):
                    if mutation == "bytes":
                        path.write_bytes(b"y=2\n")
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif mutation == "mtime":
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns + 10_000_000_000))
                    elif mutation == "mode":
                        path.chmod(stat.S_IRUSR)
                    else:
                        os.utime(
                            directory,
                            ns=(dir_info.st_atime_ns, dir_info.st_mtime_ns + 10_000_000_000),
                        )
                    return SimpleNamespace(returncode=2, stdout=b"actual", stderr=b"error")

                try:
                    with patch.object(recorder.subprocess, "run", side_effect=answer):
                        with self.assertRaises(ValueError):
                            recorder._capture(
                                Path("unused"), [], root, {"a.rb": "x=1\n"}, ["configdir"], {}, None
                            )
                    raw = json.loads((root / "private-process.json").read_bytes())
                    self.assertEqual(raw["stdout_hex"], b"actual".hex())
                    data = json.loads((root / "private-run0.json").read_bytes())
                    self.assertTrue(
                        data["before"] != data["after"]
                        or data["directories_before"] != data["directories_after"]
                    )
                finally:
                    path.chmod(info.st_mode)

    def test_timeout_partial_is_saved_before_incomplete_capture_rejection(self):
        """timeoutのpartialを欠けた成功reportへ置き換えない。"""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            error = subprocess.TimeoutExpired(
                ["unused"], 60, output=b"partial", stderr=b"\x1b[31merror"
            )
            with patch.object(recorder.subprocess, "run", side_effect=error):
                with self.assertRaisesRegex(ValueError, "timeout"):
                    recorder._capture(Path("unused"), [], root, {}, [], {}, None)
            raw = json.loads((root / "private-process.json").read_bytes())
            self.assertTrue(raw["timed_out"])
            self.assertEqual(raw["stdout_hex"], b"partial".hex())
            self.assertEqual(raw["stderr_hex"], b"\x1b[31merror".hex())

    def test_new_dangling_entry_is_rejected_before_tree_traversal(self):
        """実symlink/junctionを辿らず、元rawを保持して拒否する。"""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            link = root / "new-entry"

            def answer(*args, **kwargs):
                _dangling_entry(link, root / "absent")
                return SimpleNamespace(returncode=0, stdout=b"actual", stderr=b"")

            try:
                with patch.object(recorder.subprocess, "run", side_effect=answer):
                    with self.assertRaisesRegex(ValueError, "link/reparse"):
                        recorder._capture(Path("unused"), [], root, {}, [], {}, None)
                self.assertTrue((root / "private-process.json").is_file())
            finally:
                if os.path.lexists(link):
                    _remove_entry(link)

    @unittest.skipUnless(os.name == "nt", "Windowsの実共有違反とhandle解放を検査する")
    def test_exclusive_read_lock_is_real_and_is_released_on_exception(self):
        """readを実際に拒否させ、例外後に同じ原本を読み直せることを確認する。

        Raises:
            RuntimeError: assertRaises内で発生・捕捉し、例外後のhandle解放を検査する。

        """
        with tempfile.TemporaryDirectory() as temp:
            file = Path(temp) / "locked.yml"
            file.write_bytes(b"original")
            before = file.stat()
            with self.assertRaisesRegex(RuntimeError, "stop"):
                with recorder._locked(file):
                    with self.assertRaises(PermissionError):
                        file.read_bytes()
                    raise RuntimeError("stop")
            self.assertEqual(file.read_bytes(), b"original")
            self.assertEqual(file.stat().st_mtime_ns, before.st_mtime_ns)
            self.assertEqual(file.stat().st_mode, before.st_mode)

    @unittest.skipUnless(os.name == "nt", "実Windowsのlockとbasename APIを使うentry検査")
    def test_complete_eleven_process_entry_checks_raw_out_directory_api_and_race(self):
        """全entryとAPI/raw/out/directory/保存raceの独立した破損を検査する。"""
        data = recorder.template()
        for mutation in (None, "api", "raw", "out", "directory", "race"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp:
                work, output = Path(temp) / "work", Path(temp) / "out.json"
                calls = []
                gem_root = (Path(temp) / "rubocop-1.89.0").as_posix()
                racing = []

                def answer(
                    args,
                    *,
                    cwd,
                    mutation=mutation,
                    work=work,
                    output=output,
                    calls=calls,
                    gem_root=gem_root,
                    racing=racing,
                    **kwargs,
                ):
                    root = Path(cwd)
                    calls.append(root.name)
                    bindings = {
                        "CWD": str(root),
                        "CWD_FWD": root.as_posix(),
                        "GEM_ROOT": gem_root,
                        "LAUNCHER": (work / "reference.rb").as_posix(),
                    }
                    if root.name == "api-runtime":
                        rows = [
                            {key: recorder.bind(item, bindings) for key, item in row.items()}
                            for row in data["api"]
                        ]
                        payload = {
                            "runtime": data["runtime"],
                            "source_sha256": data["source_sha256"],
                            "gem_root": gem_root,
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
                    report = root / "out.json"
                    if mutation == "out" or not report.exists():
                        report.write_bytes(
                            b"wrong" if mutation == "out" else bytes.fromhex(row["out_hex"])
                        )
                    if mutation == "directory":
                        path = root / "configdir"
                        info = path.stat()
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns + 10_000_000_000))
                    if mutation == "race" and root.name == data["cases"][-1]["id"]:
                        output.write_bytes(b"racing sentinel")
                        racing.append(output.stat())
                    return SimpleNamespace(
                        returncode=row["exit"],
                        stdout=bytes.fromhex(row["stdout_hex"]),
                        stderr=recorder.bind(row["stderr_template"], bindings).encode()
                        + (b"extra" if mutation == "raw" else b""),
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
                            self.assertEqual(output.stat().st_mtime_ns, racing[0].st_mtime_ns)
                            self.assertEqual(output.stat().st_mode, racing[0].st_mode)
                        else:
                            self.assertFalse(os.path.lexists(output))


if __name__ == "__main__":
    unittest.main()
