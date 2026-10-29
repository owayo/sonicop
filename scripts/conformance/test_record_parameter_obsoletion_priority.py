"""最低版getter追補の限定範囲と、backtrace非公開/原本保持を検査する。"""

import copy
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import record_parameter_obsoletion_priority as recorder


class ParameterPriorityTest(unittest.TestCase):
    """13/8/2の本家対照を他の成功件数と混ぜない。"""

    def test_template_and_api_type_guards_keep_scopes_distinct(self):
        """固定13/8/2と4early/7getter/2残差を空集合やbool版へ置換しない。"""
        priority, getter, colors = recorder._templates()
        self.assertEqual([len(data["cases"]) for data in (priority, getter, colors)], [13, 8, 2])
        for row in priority["cases"]:
            recorder._validate_api(row["api"], row["api"])
        expected = priority["cases"][0]["api"]
        changed = {**expected, "target": False}
        with self.assertRaises(ValueError):
            recorder._validate_api(changed, expected)
        changed = copy.deepcopy(priority)
        changed["schema"] = True
        data = [changed, getter, colors]
        with patch.object(recorder.json, "loads", side_effect=data):
            with self.assertRaises(ValueError):
                recorder._templates()
        for files in ([], [{"path": "unexpected", "offenses": []}]):
            report = json.loads(bytes.fromhex(priority["cases"][0]["out_hex"]))
            report["files"] = files
            if not files:
                report["summary"]["target_file_count"] = True
            with self.assertRaises(ValueError):
                recorder._rejection_report(json.dumps(report).encode())

    def test_real_prefix_projection_does_not_publish_ruby_backtrace(self):
        """namespace先行とgetter先頭行を残し、内部frameはprivate rawへ分離する。"""
        priority, _, _ = recorder._templates()
        case = next(row for row in priority["cases"] if row["id"] == "namespace-before-bool")
        cli = SimpleNamespace(
            returncode=2,
            stdout=b"",
            stderr=bytes.fromhex(case["stderr_prefix_hex"]) + b"PRIVATE_RUBY_FRAME\r\n",
        )
        api = SimpleNamespace(returncode=0, stdout=json.dumps(case["api"]).encode(), stderr=b"")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "work"
            with patch.object(recorder.subprocess, "run", side_effect=[cli, api]):
                self.assertEqual(recorder.record("ruby", case, root, api=True), case)
            self.assertNotIn("PRIVATE_RUBY_FRAME", json.dumps(case))
            raw = json.loads((root / "raw.json").read_bytes())
            self.assertIn(b"PRIVATE_RUBY_FRAME", bytes.fromhex(raw["stderr_hex"]))
        changed = copy.deepcopy(case)
        changed["id"] = "unknown"
        with patch.object(recorder.subprocess, "run") as child:
            with self.assertRaises(ValueError):
                recorder.record("ruby", changed, Path("unused"), api=True)
            child.assert_not_called()

    def test_source_bytes_mtime_and_preserved_out_are_independent_guards(self):
        """正しいCLI/APIを返す偽childでも、各単独変更を拒む。"""
        priority, _, _ = recorder._templates()
        case = next(row for row in priority["cases"] if row["id"] == "bool-target-normal-fatal-1")
        for kind in ("bytes", "mtime", "out"):

            def mutation(*args, kind=kind, **kwargs):
                source = kwargs["cwd"] / ("report.json" if kind == "out" else "a.rb")
                status = source.stat()
                if kind == "mtime":
                    os.utime(source, ns=(status.st_atime_ns, status.st_mtime_ns + 2_000_000_000))
                else:
                    source.write_bytes(b"y=2\n")
                    os.utime(source, ns=(status.st_atime_ns, status.st_mtime_ns))
                    self.assertEqual(source.stat().st_mtime_ns, status.st_mtime_ns)
                return SimpleNamespace(
                    returncode=2, stdout=b"", stderr=bytes.fromhex(case["stderr_prefix_hex"])
                )

            with tempfile.TemporaryDirectory() as temporary:
                with patch.object(recorder.subprocess, "run", side_effect=mutation):
                    with self.assertRaises(ValueError):
                        recorder.record("ruby", case, Path(temporary) / "work", api=True)

    def test_getter_prefix_rejects_unexpected_color_or_warning(self):
        """最低版getterのplain本文を他の警告やANSIで置換した出力を採取しない。"""
        _, _, colors = recorder._templates()
        case = colors["cases"][0]
        for stderr in (
            b"wrong\r\n",
            b"\x1b[31m" + bytes.fromhex(case["stderr_prefix_hex"]),
            b"other warning\r\n" + bytes.fromhex(case["stderr_prefix_hex"]),
        ):
            child = SimpleNamespace(returncode=2, stdout=b"", stderr=stderr)
            with tempfile.TemporaryDirectory() as temporary:
                with patch.object(recorder.subprocess, "run", return_value=child):
                    with self.assertRaises(ValueError):
                        recorder.record("ruby", case, Path(temporary) / "work", api=False)

    def test_api_alone_cannot_mutate_out_after_the_cli_snapshot(self):
        """正しいCLI結果を保存した後でもAPIだけのout本文変更を拒む。"""
        priority, _, _ = recorder._templates()
        case = next(row for row in priority["cases"] if row["id"] == "bool-target-normal-fatal-1")
        calls = 0

        def mutation(*args, **kwargs):
            nonlocal calls
            calls += 1
            if calls == 1:
                return SimpleNamespace(
                    returncode=2, stdout=b"", stderr=bytes.fromhex(case["stderr_prefix_hex"])
                )
            output = kwargs["cwd"] / "report.json"
            status = output.stat()
            output.write_bytes(b"NOPE\n")
            os.utime(output, ns=(status.st_atime_ns, status.st_mtime_ns))
            self.assertEqual(output.stat().st_mtime_ns, status.st_mtime_ns)
            return SimpleNamespace(
                returncode=0, stdout=json.dumps(case["api"]).encode(), stderr=b""
            )

        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(recorder.subprocess, "run", side_effect=mutation):
                with self.assertRaises(ValueError):
                    recorder.record("ruby", case, Path(temporary) / "work", api=True)
        self.assertEqual(calls, 2)

    def test_four_fresh_destinations_precede_every_reference_process(self):
        """追補3出力とworkの衝突を子プロセス開始前に拒む。"""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = [root / name for name in ("work", "priority.json", "getter.json", "color.json")]
            aliases = [
                paths[:2] + [paths[1], paths[3]],
                [root / "absent" / ".." / "same", root / "same", *paths[2:]],
            ]
            if os.name == "nt":
                aliases.append([root / "Same", root / "same", *paths[2:]])
            paths[3].write_bytes(b"KEEP")

            def invoke(destinations):
                work, output, getter, colors = destinations
                args = [
                    "record",
                    "--ruby",
                    "ruby",
                    "--work-dir",
                    str(work),
                    "--output",
                    str(output),
                    "--getter-output",
                    str(getter),
                    "--color-output",
                    str(colors),
                ]
                with (
                    patch.object(sys, "argv", args),
                    patch.object(recorder.subprocess, "run") as child,
                ):
                    with self.assertRaises(ValueError):
                        recorder.main()
                    child.assert_not_called()

            invoke(paths)
            self.assertEqual(paths[3].read_bytes(), b"KEEP")
            paths[3].unlink()
            for destinations in aliases:
                self.assertTrue(all(not os.path.lexists(path) for path in destinations))
                invoke(destinations)


if __name__ == "__main__":
    unittest.main()
