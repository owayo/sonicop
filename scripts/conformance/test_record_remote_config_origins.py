"""remote採取の実entryと、部分完了・属性変化・dangling entryの拒否を検査する。"""

import copy
import hashlib
import http.client
import json
import os
import re
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_remote_config_origins as recorder
from test_record_cache_parallel_preflight import _dangling_entry, _remove_entry


class RemoteOriginRecorderContract(unittest.TestCase):
    """実I/Oと独立した破損を使い、誤ったerrorによる偽成功を拒否する。"""

    def test_fixture_keeps_all_fifteen_rows_types_raw_and_completed_scope(self):
        """昇格した三行の欠落・型混同・raw/出典/比較範囲の変更を拒否する。"""
        value = recorder.template()
        self.assertEqual(len(value["cases"]) + len(value["blocked_cases"]), 15)
        for mutation in ("empty", "blocked", "exit", "scope", "source", "raw", "cache"):
            data = copy.deepcopy(value)
            if mutation == "empty":
                data["cases"] = []
            elif mutation == "blocked":
                data["blocked_cases"] = [copy.deepcopy(data["cases"][-1])]
            elif mutation == "exit":
                data["cases"][0]["exit"] = True
            elif mutation == "scope":
                data["coverage"]["subset"] = True
            elif mutation == "source":
                data["source_sha256"]["lib/rubocop/cli.rb"] = "0" * 64
            elif mutation == "raw":
                data["cases"][0]["stdout_hex"] = ""
            else:
                data["cases"][0]["reference_cache_files"][0]["content_template"] = "changed"
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                recorder.validate(data)

    def test_environment_preserves_parent_gem_path_and_os_variables(self):
        """意図したroot/色だけを子へ渡し、親とGEM/PATH/OS情報を保つ。"""
        source = {
            "RUBYOPT": "bad",
            "RUBYLIB": "bad",
            "RUBOCOP_OPTS": "bad",
            "RUBOCOP_TARGET_RUBY_VERSION": "bad",
            "NO_COLOR": "1",
            "RUBOCOP_CACHE_ROOT": "ambient",
            "GEM_PATH": "kept",
            "PATH": "kept",
            "SystemRoot": "kept",
        }
        with patch.dict(os.environ, source, clear=True):
            original = dict(os.environ)
            child = recorder._environment({"RUBOCOP_CACHE_ROOT": "isolated"})
            self.assertEqual(dict(os.environ), original)
        for name in (
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "NO_COLOR",
        ):
            self.assertNotIn(name, child)
        for name in ("GEM_PATH", "PATH", "SystemRoot"):
            self.assertEqual(child[name.upper() if os.name == "nt" else name], "kept")
        self.assertEqual(child["RUBOCOP_CACHE_ROOT"], "isolated")

    def test_existing_alias_nested_and_missing_parent_stop_before_child(self):
        """保存先由来のerrorを明示し、別OS拒否でguardの偽成功を隠さない。"""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            sentinel = base / "out.json"
            sentinel.write_bytes(b"kept")
            before = sentinel.stat()
            pairs = [
                (base / "work", sentinel, "保存先entry"),
                (base / "same", base / "same", "保存先が重な"),
                (base / "x", base / "x/out", "保存先が重な"),
                (base / "x/work", base / "x", "保存先が重な"),
                (base / "work", base / "missing/out", "親directory"),
            ]
            with patch.object(recorder.subprocess, "run") as child:
                for work, out, error in pairs:
                    with (
                        self.subTest(work=work, out=out),
                        self.assertRaisesRegex(ValueError, error),
                    ):
                        recorder.collect(Path("unused"), work, out)
                child.assert_not_called()
            self.assertEqual(sentinel.read_bytes(), b"kept")
            self.assertEqual(sentinel.stat().st_mtime_ns, before.st_mtime_ns)
            self.assertEqual(sentinel.stat().st_mode, before.st_mode)

    def test_real_dangling_output_and_new_dangling_tree_are_rejected(self):
        """実link/junctionのentryを観測し、exists/is_fileだけの見逃しを拒否する。"""
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            link = root / "dangling"
            _dangling_entry(link, root / "missing")
            try:
                with (
                    patch.object(recorder.subprocess, "run") as child,
                    self.assertRaisesRegex(ValueError, "保存先entry"),
                ):
                    recorder.collect(Path("unused"), root / "work", link)
                child.assert_not_called()
                with self.assertRaisesRegex(ValueError, "link/reparse"):
                    recorder._cache_files(root)
            finally:
                _remove_entry(link)

    def test_source_bytes_mtime_and_mode_mutations_preserve_raw_then_fail(self):
        """本文だけ・mtimeだけ・modeだけを独立に変え、元rawを先に残す。"""
        for mutation in ("bytes", "mtime", "mode"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                path = root / "a.rb"
                path.write_bytes(b"x=1\n")
                original = path.stat()

                def answer(*args, kind=mutation, target=path, info=original, **kwargs):
                    if kind == "bytes":
                        target.write_bytes(b"y=2\n")
                        os.utime(target, ns=(info.st_atime_ns, info.st_mtime_ns))
                    elif kind == "mtime":
                        os.utime(target, ns=(info.st_atime_ns, info.st_mtime_ns + 10_000_000_000))
                    else:
                        target.chmod(stat.S_IRUSR)
                    return SimpleNamespace(returncode=1, stdout=b"actual", stderr=b"warning")

                try:
                    with (
                        patch.object(recorder.subprocess, "run", side_effect=answer),
                        self.assertRaises(ValueError),
                    ):
                        recorder._capture(Path("unused"), [], root, {"a.rb": "x=1\n"}, {}, 1)
                    data = json.loads((root / "private-process.json").read_bytes())
                    self.assertEqual(data["stdout_hex"], b"actual".hex())
                    observed = json.loads((root / "private-run0.json").read_bytes())
                    self.assertNotEqual(observed["before"], observed["after"])
                finally:
                    path.chmod(original.st_mode)

    def test_timeout_and_wrong_exit_keep_actual_partial_output(self):
        """timeoutと非期待終了を同一の完了データとして受け入れない。"""
        for timeout in (False, True):
            with self.subTest(timeout=timeout), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                response = (
                    subprocess.TimeoutExpired(["unused"], 60, output=b"partial", stderr=b"error")
                    if timeout
                    else None
                )
                kwargs = (
                    {"side_effect": response}
                    if timeout
                    else {
                        "return_value": SimpleNamespace(
                            returncode=2, stdout=b"partial", stderr=b"error"
                        )
                    }
                )
                with (
                    patch.object(recorder.subprocess, "run", **kwargs),
                    self.assertRaises(ValueError),
                ):
                    recorder._capture(Path("unused"), [], root, {}, {}, 1)
                data = json.loads((root / "private-process.json").read_bytes())
                self.assertEqual(data["stdout_hex"], b"partial".hex())
                self.assertEqual(data["stderr_hex"], b"error".hex())
                self.assertEqual(data.get("timed_out", False), timeout)

    @unittest.skipUnless(os.name == "nt", "実entryの15条件は固定Windows参照専用")
    def test_real_entry_completes_api_and_all_fifteen_cli_including_blocked_rows(self):
        """実socket要求・cache artifact・markerを作り、全16 processを完了する。"""
        self._entry(None)

    @unittest.skipUnless(os.name == "nt", "実entryの15条件は固定Windows参照専用")
    def test_real_entry_rejects_api_raw_requests_cache_and_final_output_race(self):
        """出典/全raw/要求/保存本文の破損とout競合を独立に拒否する。"""
        for kind in ("api", "raw", "requests", "cache", "race"):
            with self.subTest(kind=kind):
                self._entry(kind)

    def _entry(self, mutation):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            work, output = base / "work", base / "reference.json"
            data = recorder.template()
            calls = []
            racing = []

            def answer(argv, **kwargs):
                root = Path(kwargs["cwd"])
                calls.append(root.name)
                if argv[1] == "-e":
                    bindings = {"CWD_FWD": root.as_posix(), "DRIVE": root.drive}
                    rows = []
                    for row in data["api"]:
                        item = {k: v for k, v in row.items() if k != "cache_path_template"}
                        item["cache_path"] = recorder.bind(row["cache_path_template"], bindings)
                        rows.append(item)
                    actual = {
                        "runtime": data["runtime"],
                        "source_sha256": data["source_sha256"],
                        "rows": rows,
                    }
                    if mutation == "api":
                        actual = copy.deepcopy(actual)
                        actual["source_sha256"]["lib/rubocop/cli.rb"] = "0" * 64
                    if mutation == "race":
                        output.write_bytes(b"racing sentinel")
                        racing.append(output.stat())
                    return SimpleNamespace(
                        returncode=0, stdout=json.dumps(actual).encode(), stderr=b""
                    )
                row = next(
                    row for row in data["cases"] + data["blocked_cases"] if row["id"] == root.name
                )
                files = "\n".join((root / name).read_text(encoding="utf8") for name in row["files"])
                url = re.search(r"http://127\.0\.0\.1:(\d+)", files)
                port = int(url.group(1))
                bindings = {"BASE": url.group(), "CWD": str(root), "CWD_FWD": root.as_posix()}
                for i, path in enumerate(sorted(row["routes"])):
                    bindings[f"HASH_{i}"] = hashlib.md5((url.group() + path).encode()).hexdigest()
                if mutation != "requests":
                    for path in row["requests"]:
                        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
                        connection.request("GET", path)
                        reply = connection.getresponse()
                        self.assertEqual(reply.status, 200)
                        reply.read()
                        connection.close()
                for entry in row["reference_cache_files"]:
                    path = root / recorder.bind(entry["path_template"], bindings)
                    path.parent.mkdir(parents=True, exist_ok=True)
                    text = recorder.bind(entry["content_template"], bindings)
                    path.write_bytes(b"changed" if mutation == "cache" else text.encode())
                if row["marker_hex"] is not None:
                    (root / "loaded_marker").write_bytes(bytes.fromhex(row["marker_hex"]))
                stderr = recorder.bind(row["stderr_template"], bindings)
                if mutation == "raw":
                    stderr += "unmeasured"
                return SimpleNamespace(
                    returncode=row["exit"],
                    stdout=bytes.fromhex(row["stdout_hex"]),
                    stderr=stderr.encode(),
                )

            with patch.object(recorder.subprocess, "run", side_effect=answer):
                if mutation is None:
                    self.assertEqual(recorder.collect(Path("unused"), work, output), data)
                    self.assertEqual(len(calls), 16)
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
