"""空config記録のraw・Options・完了判定・原本保持を有限の負対照で守る。"""

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

import record_empty_config_arguments as recorder


def fixture():
    """本家原参照を単体対照へ渡し、候補を起動しない。

    Returns:
        dict: schema付きの20CLIとOptions API記録。

    """
    return json.loads(
        (Path(__file__).parents[2] / "tests/fixtures/empty_config_arguments.json").read_bytes()
    )


class EmptyConfigArgumentsTest(unittest.TestCase):
    """20条件の境界を独立に壊し、空reportや別phaseを受理させない。"""

    def test_exact_inputs_options_and_schema_match_the_reference(self):
        """固定args/files/kind/environmentと元APIを一緒に検査する。"""
        document = fixture()
        recorder.validate(document)
        forms = recorder._selected_inputs()
        self.assertEqual(tuple(row["id"] for row in forms), recorder.CASE_IDS)
        self.assertEqual(len(set(recorder.CASE_IDS)), 20)
        for form, row in zip(forms, document["cases"], strict=True):
            self.assertEqual(form, {key: row[key] for key in form})
            self.assertEqual(row["options"], recorder._options(form))
        for broken in ([], forms[:-1], forms[:-1] + [forms[0]], list(reversed(forms))):
            with patch.object(recorder, "inputs", return_value=broken):
                with self.assertRaises(ValueError):
                    recorder._selected_inputs()

    def test_wrong_runtime_source_input_or_api_is_not_evidence(self):
        """型が似ていてもOptionsのboolを整数へ変えず、source情報も欠落させない。"""
        mutations = [
            lambda d: d.update(schema=True),
            lambda d: d["cases"].pop(),
            lambda d: d["runtime"]["gems"].update(rubocop="0"),
            lambda d: d["source_hashes"].update(**{"options.rb": ""}),
            lambda d: d["cases"][0]["args"].append("unrecorded"),
            lambda d: d["cases"][0]["files"].update(Gemfile="changed"),
            lambda d: d["cases"][0].update(kind="success"),
            lambda d: d["cases"][0]["environment"].update(RUBYOPT="poison"),
            lambda d: d["cases"][0]["options"].update(config_present=1),
            lambda d: d["cases"][10]["options"].update(config_present=0),
            lambda d: d["cases"][0]["options"].update(config=None),
            lambda d: d["cases"][12]["options"].update(error_class=None),
            lambda d: d["cases"][17]["options"].update(config=""),
            lambda d: d["cases"][0]["options"].update(id="other"),
        ]
        for mutate in mutations:
            value = copy.deepcopy(fixture())
            mutate(value)
            with self.assertRaises(ValueError):
                recorder.validate(value)

    def test_stdout_io_out_and_original_guards_are_independent(self):
        """通知欠落・偽phase・sentinel喪失・本文変更を完了扱いにしない。"""
        mutations = [
            lambda d: d["cases"][0].update(exit=True),
            lambda d: d["cases"][3].update(stdout_hex=""),
            lambda d: d["cases"][0].update(stderr_first_line="missing argument: --config"),
            lambda d: d["cases"][0].update(out_hex="00"),
            lambda d: d["cases"][7].update(out_hex=None),
            lambda d: d["cases"][0]["after_hex"].update(**{"case.rb": ""}),
            lambda d: d["cases"][14].update(stdout_hex=""),
        ]
        for mutate in mutations:
            value = copy.deepcopy(fixture())
            mutate(value)
            with self.assertRaises(ValueError):
                recorder.validate(value)

    def test_complete_json_rejects_identity_path_counts_and_all_offenses(self):
        """bool件数・候補identity・検査不足・件数だけ一致した偽reportを拒否する。"""
        row = fixture()["cases"][9]
        report = json.loads(bytes.fromhex(row["out_hex"]))
        recorder._report(json.dumps(report))
        for key, value in [
            ("inspected_file_count", 0),
            ("target_file_count", 0),
            ("inspected_file_count", True),
            ("target_file_count", True),
            ("offense_count", False),
        ]:
            broken = copy.deepcopy(report)
            broken["summary"][key] = value
            with self.assertRaises(ValueError):
                recorder._report(json.dumps(broken))
        for kind in ("identity", "path", "offenses", "empty"):
            broken = copy.deepcopy(report)
            if kind == "identity":
                broken["metadata"]["ruby_engine"] = "sonicop"
            elif kind == "path":
                broken["files"][0]["path"] = "other.rb"
            elif kind == "offenses":
                broken["files"][0]["offenses"] = [{"message": "hidden"}]
            else:
                broken = {}
            with self.assertRaises((ValueError, KeyError)):
                recorder._report(json.dumps(broken))

    def test_parent_environment_is_preserved_and_only_explicit_opts_return(self):
        """ENV4を子だけから除き、last winsの入力RUBOCOP_OPTSだけ明示して戻す。"""
        parent = {
            "RUBYOPT": "poison",
            "RUBYLIB": "poison",
            "RUBOCOP_OPTS": "poison",
            "RUBOCOP_TARGET_RUBY_VERSION": "0",
            "GEM_HOME": "gem-home",
            "GEM_PATH": "gem-path",
            "PATH": "tools",
            "SystemRoot": "os",
            "LANG": "ja_JP.UTF-8",
        }
        for case in (recorder.inputs()[0], recorder.inputs()[-1]):
            calls = []

            def run(_command, **kwargs):
                calls.append(kwargs["env"])
                return subprocess.CompletedProcess([], 2, b"", b"Is a directory\r\n")

            with (
                tempfile.TemporaryDirectory() as folder,
                patch.dict(os.environ, parent, clear=True),
            ):
                before = dict(os.environ)
                base = recorder.reference_environment()
                with patch.object(recorder.subprocess, "run", side_effect=run):
                    recorder._capture(Path("ruby"), Path("wrapper.rb"), Path(folder), case, base)
                self.assertEqual(dict(os.environ), before)
                expected = {
                    key: value
                    for key, value in before.items()
                    if key
                    not in {"RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"}
                }
                self.assertEqual(calls, [{**expected, **case["environment"]}])

    def test_source_bytes_and_mtime_fail_without_another_invalid_channel(self):
        """正しいchannel/outを返しmtimeを戻しても、同長本文の差だけで拒否する。"""
        row = fixture()["cases"][9]
        case = recorder.inputs()[9]
        for kind in ("bytes", "mtime"):

            def run(_command, **kwargs):
                cwd = kwargs["cwd"]
                source = cwd / "case.rb"
                before = source.stat()
                (cwd / "out.json").write_bytes(bytes.fromhex(row["out_hex"]))
                if kind == "bytes":
                    source.write_bytes(b"y = 2\r\n")
                os.utime(
                    source,
                    ns=(
                        before.st_atime_ns,
                        before.st_mtime_ns + (1_000_000_000 if kind == "mtime" else 0),
                    ),
                )
                if kind == "bytes":
                    self.assertEqual(source.stat().st_mtime_ns, before.st_mtime_ns)
                return subprocess.CompletedProcess(
                    [], row["exit"], bytes.fromhex(row["stdout_hex"]), b""
                )

            with (
                tempfile.TemporaryDirectory() as folder,
                patch.object(recorder.subprocess, "run", side_effect=run),
            ):
                with self.assertRaises(ValueError):
                    recorder._capture(Path("ruby"), Path("wrapper.rb"), Path(folder), case, {})

    def test_fresh_entries_and_aliases_reject_before_any_child(self):
        """dangling symlink・dotdot・Windows case差でも同一出力を作成しない。"""
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            paths = [root / "fresh-work", root / "fresh-out.json"]
            original_lstat = os.lstat
            for target in paths:

                def _lstat(path, *args, **kwargs):
                    if Path(path) == target:
                        return SimpleNamespace(st_mode=stat.S_IFLNK)
                    return original_lstat(path, *args, **kwargs)

                with patch("os.lstat", side_effect=_lstat):
                    with self.assertRaises(ValueError):
                        recorder._fresh_paths(*paths)
                target.write_bytes(b"KEEP")
                with patch.object(recorder.subprocess, "run") as run:
                    with self.assertRaises(ValueError):
                        recorder.record(Path("ruby"), paths[1], paths[0])
                    run.assert_not_called()
                self.assertEqual(target.read_bytes(), b"KEEP")
                target.unlink()
            aliases = [
                (root / "absent" / ".." / "same", root / "same"),
                (root / "same", root / "same"),
            ]
            if os.name == "nt":
                aliases.append((root / "Same", root / "same"))
            for work, output in aliases:
                with patch.object(recorder.subprocess, "run") as run:
                    with self.assertRaises(ValueError):
                        recorder.record(Path("ruby"), output, work)
                    run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
