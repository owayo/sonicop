"""argv記録が別のRuby版や空の観測を成功扱いせず、親の注入だけを外すことを検査する。"""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

_SPEC = importlib.util.spec_from_file_location(
    "windows_argv_recorder", Path(__file__).resolve().parents[1] / "record_windows_argv.py"
)
_RECORDER = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_RECORDER)


class WindowsArgvCollectorTest(unittest.TestCase):
    """実プロセスの出力を使い、測っていない成功を拒む。"""

    def test_existing_reference_is_kept_without_launching_a_child(self):
        """保存済み参照を再採取で上書きしない。"""
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "reference.json"
            output.write_bytes(b"original")
            with (
                mock.patch.object(sys, "argv", ["record", str(output)]),
                mock.patch.object(_RECORDER.subprocess, "run") as child,
            ):
                with self.assertRaisesRegex(RuntimeError, "already exists"):
                    _RECORDER.main()
                child.assert_not_called()
            self.assertEqual(output.read_bytes(), b"original")

    @unittest.skipUnless(sys.platform == "win32", "raw Windows commandline collector")
    def test_wrong_runtime_is_rejected(self):
        """単語があっても仕様版と違うRubyの記録を保存しない。"""
        with tempfile.TemporaryDirectory() as temporary:
            probe = Path(temporary) / "probe.py"
            probe.write_text(
                "import json; print(json.dumps({'version':'0.0',"
                "'revision':'wrong','words':['--only']}))",
                encoding="utf-8",
                newline="\n",
            )
            with self.assertRaises(RuntimeError):
                _RECORDER._capture(
                    sys.executable, probe, ("wrong", "--only"), _RECORDER._environment()
                )

    @unittest.skipUnless(sys.platform == "win32", "raw Windows commandline collector")
    def test_no_observed_words_is_rejected(self):
        """起動元tailにはoptionがあるため、空ARGVは未測定として拒む。"""
        with tempfile.TemporaryDirectory() as temporary:
            probe = Path(temporary) / "probe.py"
            report = {"version": "4.0.7", "revision": _RECORDER.REVISION, "words": []}
            probe.write_text(
                "print(" + repr(json.dumps(report)) + ")",
                encoding="utf-8",
                newline="\n",
            )
            with self.assertRaises(RuntimeError):
                _RECORDER._capture(
                    sys.executable, probe, ("empty", "--only"), _RECORDER._environment()
                )

    def test_child_cannot_inherit_poison_and_keeps_runtime_sentinels(self):
        """Ruby起動への注入を外しても、gemとOS探索に必要な環境を消さない。"""
        poison = {key: "injected-value" for key in _RECORDER.ENV_KEYS}
        retained = {
            "GEM_HOME": "retained-gem-home",
            "GEM_PATH": "retained-gem-path",
            "PATH": os.environ["PATH"],
            "SystemRoot": os.environ.get("SystemRoot", "retained-system-root"),
            "SONICOP_ARGV_PARENT_SENTINEL": "保持する値",
        }
        with mock.patch.dict(os.environ, {**poison, **retained}):
            parent = dict(os.environ)
            script = (
                "import json,os; print(json.dumps({k:os.environ.get(k) for k in "
                + repr(list(poison) + list(retained))
                + "}))"
            )
            result = subprocess.run(
                [sys.executable, "-c", script],
                env=_RECORDER._environment(),
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stderr, b"")
            self.assertEqual(
                json.loads(result.stdout), {**{key: None for key in poison}, **retained}
            )
            self.assertEqual(dict(os.environ), parent)


if __name__ == "__main__":
    unittest.main()
