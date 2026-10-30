"""固定参照の型・全raw・entry保持と、採取失敗時の実記録保全を検査する。"""

import copy
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_cache_root_warning as recorder

REAL_RUN = subprocess.run
REAL_SETUP = recorder.setup


class CacheRootRecorderContract(unittest.TestCase):
    """期待を返すstubと独立した破損を使い、別errorによる偽成功を拒否する。"""

    def test_schema_all_raw_inputs_identity_and_types_are_fixed(self):
        """ケース欠落・順序・bool混同・API欠落・raw改変を拒否する。"""
        recorder.validate(recorder.template())
        for mutation in ("schema", "order", "exit", "raw", "api", "metadata", "input"):
            value = recorder.template()
            if mutation == "schema":
                value["schema"] = True
            elif mutation == "order":
                value["cases"].reverse()
            elif mutation == "exit":
                value["cases"][0]["exit"] = False
            elif mutation == "raw":
                value["additional_cases"][0]["stdout_hex"] = ""
            elif mutation == "api":
                value["realpath_api"]["first"]["pairs"] = []
            elif mutation == "metadata":
                value["cases"][0]["report"]["metadata"]["ruby_patchlevel"] = False
            else:
                value["cases"][0]["input_hex"] = ""
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                recorder.validate(value)

    def test_bind_changes_only_explicit_roots_without_mutating_input(self):
        """単一の動的binding以外の文字・型と元データを保つ。"""
        value = {"a": ["<LONG>/0", 0, False, None, "unbound"], "b": "<LONG_FWD>/0"}
        original = copy.deepcopy(value)
        self.assertEqual(
            recorder.bind(value, {"LONG": "long"}),
            {
                "a": ["long/0", 0, False, None, "unbound"],
                "b": "<LONG_FWD>/0",
            },
        )
        self.assertEqual(value, original)

    def test_child_environment_keeps_parent_gem_path_and_os_values(self):
        """ENV4と意図した色/rootだけを子copyへ反映する。"""
        source = {
            "RUBYOPT": "bad",
            "RUBYLIB": "bad",
            "RUBOCOP_OPTS": "bad",
            "RUBOCOP_TARGET_RUBY_VERSION": "bad",
            "NO_COLOR": "1",
            "RUBOCOP_CACHE_ROOT": "ambient",
            "PATH": "kept",
            "GEM_PATH": "kept",
            "SystemRoot": "kept",
        }
        with patch.dict(os.environ, source, clear=True):
            before = dict(os.environ)
            child = recorder._environment({"XDG_CACHE_HOME": "isolated"})
            self.assertEqual(dict(os.environ), before)
        for key in (
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "NO_COLOR",
            "RUBOCOP_CACHE_ROOT",
        ):
            self.assertNotIn(key, child)
        for key in ("PATH", "GEM_PATH", "SystemRoot"):
            self.assertEqual(child[key.upper() if os.name == "nt" else key], "kept")
        self.assertEqual(child["XDG_CACHE_HOME"], "isolated")

    def test_existing_aliased_or_nested_output_entries_stop_before_child(self):
        """既存entry・alias・両向き包含・親なしを生成前に拒否する。"""
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            sentinel = base / "out.json"
            sentinel.write_bytes(b"kept")
            before = sentinel.stat()
            pairs = [
                (base / "work", sentinel),
                (base / "same", base / "same"),
                (base / "parent", base / "parent/out"),
                (base / "parent/work", base / "parent"),
                (base / "work", base / "missing/out"),
            ]
            with patch.object(recorder.subprocess, "run") as child:
                for work, output in pairs:
                    with self.subTest(work=work, output=output), self.assertRaises(ValueError):
                        recorder.collect(Path("unused"), work, output)
                child.assert_not_called()
            self.assertEqual(sentinel.read_bytes(), b"kept")
            self.assertEqual(sentinel.stat().st_mtime_ns, before.st_mtime_ns)

    def test_capture_preserves_raw_before_rejecting_source_or_exit(self):
        """本文だけの改変と終了2を、rawを残した上で拒否する。"""
        for mutation in ("bytes", "exit"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                source = root / "input.rb"
                source.write_bytes(b"kept")
                before = source.stat()

                def answer(*args, captured=mutation, path=source, info=before, **kwargs):
                    if captured == "bytes":
                        path.write_bytes(b"lost")
                        os.utime(path, ns=(info.st_atime_ns, info.st_mtime_ns))
                    return SimpleNamespace(
                        returncode=2 if captured == "exit" else 0, stdout=b"actual", stderr=b"error"
                    )

                with (
                    patch.object(recorder.subprocess, "run", side_effect=answer),
                    self.assertRaises(ValueError),
                ):
                    recorder._capture(Path("unused"), [], root, {"input.rb": b"kept"}, {})
                raw = json.loads((root / "private-process.json").read_bytes())
                self.assertEqual(raw["stdout_hex"], b"actual".hex())
                self.assertEqual(raw["stderr_hex"], b"error".hex())
                self.assertTrue((root / "private-run0.json").is_file())

    @unittest.skipUnless(os.name == "nt", "Windowsの実alias構築はUnixへ投影しない")
    def test_real_entry_completes_all_nineteen_stubbed_cli_and_api(self):
        """実entryを通し、19CLI+APIの全照合と完成版のexclusive保存を確かめる。"""
        self._entry(None)

    @unittest.skipUnless(os.name == "nt", "Windowsの実alias構築はUnixへ投影しない")
    def test_real_entry_rejects_bad_api_raw_and_output_race(self):
        """API/CLIの全raw差とout競合を、それぞれ独立に拒否する。"""
        for mutation in ("api", "raw", "race"):
            with self.subTest(mutation=mutation):
                self._entry(mutation)

    def _entry(self, mutation):
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            output, work = base / "reference.json", base / "work"
            bindings = {}
            calls = []
            value = recorder.template()

            def setup(folder):
                bindings.update(REAL_SETUP(folder))
                return bindings

            def answer(argv, **kwargs):
                if isinstance(argv, str):
                    return REAL_RUN(argv, **kwargs)
                cwd = Path(kwargs["cwd"])
                calls.append(cwd.name)
                if argv[1] == "-e":
                    groups = [
                        recorder.bind(value["realpath_api"][key], bindings)
                        for key in ("first", "additional")
                    ]
                    result = {key: value[key] for key in ("runtime", "source_sha256", "path_cases")}
                    result["groups"] = [
                        {key: group[key] for key in ("paths", "pairs")} for group in groups
                    ]
                    if mutation == "api":
                        result["groups"][0]["paths"]["long"]["exists"] = False
                    if mutation == "race":
                        output.write_bytes(b"racing sentinel")
                    return SimpleNamespace(
                        returncode=0, stdout=json.dumps(result).encode(), stderr=b""
                    )
                row = next(
                    row
                    for row in value["cases"] + value["additional_cases"]
                    if row["id"] == cwd.name
                )
                if row in value["cases"]:
                    stderr = row["stderr"].replace("<XDG>", str(cwd / "cache"))
                    stdout = json.dumps(row["report"]).encode()
                else:
                    configured = recorder.root(row["configured_kind"], bindings) + (
                        "/0" if row["append_uid"] else ""
                    )
                    desired = recorder.root(row["desired_kind"], bindings) + "/0/rubocop_cache"
                    stderr = (
                        row["stderr_template"]
                        .replace("<EARLY>", configured + "/rubocop_cache")
                        .replace("<DESIRED>", desired)
                    )
                    stdout = bytes.fromhex(row["stdout_hex"])
                if mutation == "raw":
                    stderr += "unmeasured"
                return SimpleNamespace(returncode=0, stdout=stdout, stderr=stderr.encode())

            with (
                patch.object(recorder, "setup", side_effect=setup),
                patch.object(recorder.subprocess, "run", side_effect=answer),
            ):
                if mutation is None:
                    self.assertEqual(recorder.collect(Path("unused"), work, output), value)
                    self.assertEqual(len(calls), 20)
                    self.assertEqual(json.loads(output.read_bytes()), value)
                else:
                    with self.assertRaises((ValueError, FileExistsError)):
                        recorder.collect(Path("unused"), work, output)
                    self.assertTrue(list(work.glob("*/private-process.json")))
                    if mutation == "race":
                        self.assertEqual(output.read_bytes(), b"racing sentinel")
                    else:
                        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
