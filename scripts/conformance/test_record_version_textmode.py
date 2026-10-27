"""版表示採取器のno-overwrite・ENV4・raw境界を候補なしで検査する。"""

import copy
import json
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_version_textmode as recorder


def fixture():
    """本家だけから採った20条件を読む。

    Returns:
        dict: raw channelsと固定入力を含む参照資料。

    """
    return json.loads(
        (Path(__file__).resolve().parents[2] / "tests/fixtures/version_textmode.json").read_bytes()
    )


class VersionTextmodeTest(unittest.TestCase):
    """版identityを候補へ合わせる経路や、不完全な記録を受け付けない。"""

    def test_reference_coverage_and_channels_are_complete(self):
        """4aliasと輸送/早期exitの20入力だけを固定する。"""
        document = fixture()
        recorder.validate(document)
        self.assertEqual(len(document["cases"]), 20)
        self.assertEqual(len({case["id"] for case in recorder.inputs()}), 20)
        self.assertEqual(
            {case["args"][1] for case in recorder.inputs()},
            {"-v", "-V", "--version", "--verbose-version"},
        )
        self.assertEqual(sum(case["kind"] == "rejection" for case in document["cases"]), 4)

    def test_corrupted_runtime_input_transport_and_original_are_rejected(self):
        """期待本文・改行・出力先・完了集合を個別に壊し、拒否を確認する。"""
        document = fixture()
        changes = [
            lambda row: row["runtime"].update(ruby="0"),
            lambda row: row["runtime"].update(platform="another-platform"),
            lambda row: row["runtime"]["gems"].update(rubocop="0"),
            lambda row: row["runtime"]["source_sha256"].update(**{"cli.rb": "0"}),
            lambda row: row["cases"].pop(),
            lambda row: row["cases"][0]["args"].append("--stderr"),
            lambda row: row["cases"][0].update(stdout_hex=b"1.89.0\n".hex()),
            lambda row: row["cases"][0].update(stderr_hex=b"unexpected".hex()),
            lambda row: row["cases"][0].update(exit=2),
            lambda row: row["cases"][0].update(out_exists=True),
            lambda row: row["cases"][0].update(original_bytes_mtime_mode_equal=False),
            lambda row: row["cases"][0]["after_hex"].update(**{"good.rb": ""}),
            lambda row: next(case for case in row["cases"] if case["kind"] == "rejection").update(
                stderr_hex=b"internal error\r\n".hex()
            ),
        ]
        for change in changes:
            mutated = copy.deepcopy(document)
            change(mutated)
            with self.assertRaises(ValueError):
                recorder.validate(mutated)

    def test_existing_output_is_rejected_before_work_and_children(self):
        """既存sentinelを通常entrypointからも保持し、子を起動しない。"""
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            output = folder / "record.json"
            output.write_bytes(b"KEEP\x00\r\n")
            before = (output.read_bytes(), output.stat().st_mtime_ns)
            with patch.object(recorder.subprocess, "run") as calls:
                with self.assertRaises(FileExistsError):
                    recorder.record(Path("ruby"), output, folder / "work")
                calls.assert_not_called()
                with patch(
                    "sys.argv",
                    [
                        "record",
                        "--ruby",
                        "ruby",
                        "--output",
                        str(output),
                        "--work-dir",
                        str(folder / "work"),
                    ],
                ):
                    with self.assertRaises(FileExistsError):
                        recorder.main()
                calls.assert_not_called()
            self.assertFalse((folder / "work").exists())
            self.assertEqual((output.read_bytes(), output.stat().st_mtime_ns), before)

    def test_all_children_use_a_clean_copy_and_parent_search_paths_are_kept(self):
        """metadataを含む21子へ同じENV4境界を使い、親/Gem/PATH/OSを保持する。"""
        document = fixture()
        keys = {"RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"}
        sentinel = {"GEM_HOME": "home", "GEM_PATH": "gems", "PATH": "path", "SystemRoot": "os"}
        rows = {case["id"]: case for case in document["cases"]}

        def run(command, **kwargs):
            env = kwargs["env"]
            self.assertTrue(keys.isdisjoint(env))
            for name, value in sentinel.items():
                self.assertEqual(
                    next(item for key, item in env.items() if key.upper() == name.upper()), value
                )
            if "-e" in command:
                return SimpleNamespace(
                    stdout=json.dumps(document["runtime"]).encode(), stderr=b"", returncode=0
                )
            row = rows[kwargs["cwd"].name]
            return SimpleNamespace(
                stdout=bytes.fromhex(row["stdout_hex"]),
                stderr=bytes.fromhex(row["stderr_hex"]),
                returncode=row["exit"],
            )

        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            with patch.dict(os.environ, {**{key: "poison" for key in keys}, **sentinel}):
                parent = dict(os.environ)
                with patch.object(recorder.subprocess, "run", side_effect=run) as calls:
                    recorder.record(Path("ruby"), folder / "record.json", folder / "work")
                self.assertEqual(calls.call_count, 21)
                self.assertEqual(dict(os.environ), parent)
            self.assertEqual(json.loads((folder / "record.json").read_bytes()), document)

    def test_output_race_is_preserved_by_exclusive_write(self):
        """最後の子と公開writeの間にできた別記録も、上書きしない。"""
        document = fixture()
        rows = {case["id"]: case for case in document["cases"]}
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            output = folder / "record.json"
            count = 0
            snapshot = None

            def run(command, **kwargs):
                nonlocal count, snapshot
                count += 1
                if "-e" in command:
                    return SimpleNamespace(
                        stdout=json.dumps(document["runtime"]).encode(), stderr=b"", returncode=0
                    )
                row = rows[kwargs["cwd"].name]
                if count == 21:
                    output.write_bytes(b"OTHER\r\n")
                    snapshot = (output.read_bytes(), output.stat().st_mtime_ns)
                return SimpleNamespace(
                    stdout=bytes.fromhex(row["stdout_hex"]),
                    stderr=bytes.fromhex(row["stderr_hex"]),
                    returncode=row["exit"],
                )

            with patch.object(recorder.subprocess, "run", side_effect=run) as calls:
                with self.assertRaises(FileExistsError):
                    recorder.record(Path("ruby"), output, folder / "work")
            self.assertEqual(calls.call_count, 21)
            self.assertEqual((output.read_bytes(), output.stat().st_mtime_ns), snapshot)


if __name__ == "__main__":
    unittest.main()
