#!/usr/bin/env python3
"""承認済みの Windows 非 Unicode 引数境界を、固定した本家 CLI から再記録する。"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def _environment():
    # Ruby/native 共通の子 copy だけを隔離し、親とGem/PATH/OSの探索環境を保つ。
    environment = dict(os.environ)
    for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        environment.pop(key, None)
    return environment


def _normalize_case(value, folder):
    """動的な case root だけを除き、拡張形式の接頭辞や相対名は保つ。

    Returns:
        case root の文字列だけを置き換えた JSON 互換の値。

    """
    if isinstance(value, str):
        return value.replace(str(folder), "<CASE>").replace(
            str(folder).replace("\\", "/"), "<CASE>"
        )
    if isinstance(value, list):
        return [_normalize_case(item, folder) for item in value]
    if isinstance(value, dict):
        return {key: _normalize_case(item, folder) for key, item in value.items()}
    return value


def main():
    """固定した本家を再実行し、候補は承認済み拒否契約へ照合する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument(
        "--sonicop", help="指定時は承認済み拒否契約を検証する。期待値の取得元にはしない"
    )
    parser.add_argument("--output", type=Path)
    options = parser.parse_args()
    if sys.platform != "win32":
        parser.error("Windows 専用。Unix の本家参照は未実測")
    ruby = shutil.which(options.ruby)
    if not ruby:
        parser.error("Ruby executable not found")
    sonicop = shutil.which(options.sonicop) if options.sonicop else None
    if options.sonicop and not sonicop:
        parser.error("Sonicop executable not found")
    loader = (
        'gem "parser", "3.3.12.0"; gem "rubocop", "1.89.0"; '
        'load Gem.bin_path("rubocop", "rubocop", "1.89.0")'
    )
    environment = _environment()
    records = []
    with tempfile.TemporaryDirectory(prefix="sonicop-nonunicode-") as temporary:
        root = Path(temporary)
        # ソース・設定コンテナは LF、Ruby 原本は監査済みの明示した CRLF バイト。
        (root / "config.yml").write_bytes(
            b"AllCops:\n  NewCops: disable\n  TargetRubyVersion: 3.3\n"
        )
        for location in ["path", "argv0"]:
            for spelling in (
                ["ordinary", "extended"] if location == "path" else ["ordinary"]
            ):
                for present in [False, True] if location == "path" else [False]:
                    for mode, flag in [
                        ("inspect", []),
                        ("safe", ["-a"]),
                        ("unsafe", ["-A"]),
                    ]:
                        folder = root / f"{location}-{spelling}-{present}-{mode}"
                        folder.mkdir()
                        raw = folder / (
                            "raw-\ud800.rb" if location == "path" else "valid.rb"
                        )
                        replacement = folder / "raw-\ufffd.rb"
                        original = bytes.fromhex("783d310d0a")
                        replaced = bytes.fromhex("793d320d0a") if present else None
                        raw.write_bytes(original)
                        if replaced is not None:
                            replacement.write_bytes(replaced)
                        argument = str(raw)
                        if spelling == "extended":
                            argument = "\\\\?\\" + argument
                        arguments = [
                            "--config",
                            str(root / "config.yml"),
                            "--cache",
                            "false",
                            "--only",
                            "Layout/SpaceAroundOperators",
                            "--format",
                            "json",
                            *flag,
                            argument,
                        ]
                        invocation = [ruby, "-e", loader, "--", *arguments]
                        if location == "argv0":
                            invocation[0] = "raw-\ud800.exe"
                        output = subprocess.run(
                            invocation,
                            check=False,
                            executable=ruby,
                            cwd=root,
                            env=environment,
                            capture_output=True,
                            timeout=90,
                        )
                        report = json.loads(output.stdout)
                        assert report["metadata"]["rubocop_version"] == "1.89.0"
                        assert report["metadata"]["ruby_version"] == "4.0.7"

                        reference = {
                            "exit": output.returncode,
                            "stdout": _normalize_case(report, folder),
                            "stderr": _normalize_case(
                                output.stderr.decode("utf-8"), folder
                            ),
                            "raw_after_hex": raw.read_bytes().hex(),
                            "replacement_after_hex": replacement.read_bytes().hex()
                            if replacement.exists()
                            else None,
                        }
                        index = 0 if location == "argv0" else 9 + bool(flag)
                        contract = {
                            "exit": 2,
                            "stdout": "",
                            "stderr": (
                                f"Error: command-line argument {index} "
                                "is not valid Unicode\n"
                            ),
                            "raw_after_hex": original.hex(),
                            "replacement_after_hex": replaced.hex()
                            if replaced is not None
                            else None,
                        }
                        if sonicop:
                            raw.write_bytes(original)
                            if replaced is not None:
                                replacement.write_bytes(replaced)
                            native = [sonicop, *arguments]
                            if location == "argv0":
                                native[0] = "raw-\ud800.exe"
                            actual = subprocess.run(
                                native,
                                check=False,
                                executable=sonicop,
                                cwd=root,
                                env=environment,
                                capture_output=True,
                                timeout=90,
                            )
                            assert (
                                actual.returncode == contract["exit"]
                                and actual.stdout == b""
                            )
                            assert actual.stderr.rstrip(b"\r\n") == contract[
                                "stderr"
                            ].encode().rstrip(b"\r\n")
                            assert raw.read_bytes() == original
                            assert (
                                replacement.read_bytes()
                                if replacement.exists()
                                else None
                            ) == replaced
                        name = (
                            "raw-\ud800.rb" if location == "path" else "raw-\ud800.exe"
                        )
                        wide = name.encode("utf-16le", "surrogatepass")
                        records.append(
                            {
                                "location": location,
                                "spelling": spelling,
                                "replacement_present": present,
                                "mode": mode,
                                "argument_filename_utf16_code_units": [
                                    int.from_bytes(wide[index : index + 2], "little")
                                    for index in range(0, len(wide), 2)
                                ],
                                "raw_before_hex": original.hex(),
                                "replacement_before_hex": replaced.hex()
                                if replaced is not None
                                else None,
                                "reference": reference,
                                "sonicop": contract,
                            }
                        )
    fixture = {
        "issue": 100,
        "measured_platform": "windows",
        "reference": {"rubocop": "1.89.0", "ruby": "4.0.7", "parser": "3.3.12.0"},
        "unix_reference": (
            "not measured; cfg(unix) regression configures invalid UTF-8 bytes "
            "and argv0, but was not run on Windows"
        ),
        "unmeasured_reference_platforms": ["unix"],
        "cases": records,
    }
    encoded = (json.dumps(fixture, indent=2) + "\n").encode("ascii")
    if options.output:
        options.output.parent.mkdir(parents=True, exist_ok=True)
        options.output.write_bytes(encoded)
    else:
        sys.stdout.buffer.write(encoded)


if __name__ == "__main__":
    main()
