#!/usr/bin/env python3
"""本家が符号化エラーで止まる無害対照を、元codecへ戻した実CLIの連鎖で記録する。"""

import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RUBY = os.environ.get("RUBY", "ruby")
LOAD = "gem 'rubocop','=1.89.0'; gem 'rubocop-ast','=1.50.0'; gem 'parser','=3.3.12.0'; require 'rubocop'; "
ORACLE = [RUBY, "-e", LOAD + "load Gem.bin_path('rubocop','rubocop','1.89.0')", "--"]


def run(directory, case, mode):
    args = ORACLE + ["--cache", "false", "--only", case["only"], "-f", "json"]
    if mode != "inspect":
        args.append(mode)
    result = subprocess.run(
        args + ["example.rb"], cwd=directory, capture_output=True, timeout=90
    )
    report = json.loads(result.stdout)
    assert (
        report["summary"]["target_file_count"]
        == report["summary"]["inspected_file_count"]
        == 1
    )
    assert len(report["files"]) == 1 and report["files"][0]["path"] == "example.rb"
    assert result.returncode in [0, 1]
    return dict(
        exit=result.returncode, offenses=report["files"][0]["offenses"], report=report
    )


def restore_reference(original, current, written, codec):
    if current == written:
        return current
    before = original.decode(codec).split("\n")
    # Windowsの実File.writeはLFをCRLFへ展開する。そのrawも別途保存してからcodeを復元する。
    after = written.decode("utf-8")
    if os.name == "nt":
        after = after.replace("\r\n", "\n")
    after = after.split("\n")
    marker = next(i for i, line in enumerate(before) if line.rstrip("\r") == "__END__")
    assert (
        next(i for i, line in enumerate(after) if line.rstrip("\r") == "__END__")
        == marker
    )
    code = ""
    for i in range(marker + 1):
        assert "\r" not in after[i]
        code += after[i] + ("\r\n" if before[i].endswith("\r") else "\n")
    return code.encode(codec) + "\n".join(before[marker + 1 :]).encode(codec)


def key(offense):
    # 実Offense::COMPARISON_ATTRIBUTES。span長で重複を分けず、最初の位置の記録を保つ。
    location = offense["location"]
    return (
        location["start_line"],
        location["start_column"],
        offense["cop_name"],
        offense["message"],
        offense["severity"],
    )


def main():
    raw_path = ROOT / "tests/fixtures/capture_line_endings.json"
    raw_bytes = raw_path.read_bytes()
    raw = json.loads(raw_bytes)
    environment = json.loads(
        subprocess.check_output(
            [
                RUBY,
                "-e",
                LOAD
                + "require 'json'; puts JSON.generate(ruby:RUBY_DESCRIPTION,gems:Gem.loaded_specs.transform_values{|s|s.version.to_s},offense_comparison_attributes:RuboCop::Cop::Offense::COMPARISON_ATTRIBUTES)",
            ]
        )
    )
    records = []
    for case in raw["records"]:
        if case["danger"] or case["mode"] == "inspect":
            continue
        original = bytes.fromhex(case["source_hex"])
        current = original
        steps = []
        corrected = {}
        known_encoding_errors = [
            o for o in case["upstream_offenses"] if o["cop_name"] == "Lint/Syntax"
        ]
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory / ".rubocop.yml").write_bytes(case["config"].encode())
            path = directory / "example.rb"
            for _ in range(4):
                path.write_bytes(current)
                inspection = run(directory, case, "inspect")
                assert not any(
                    o["cop_name"] == "Lint/Syntax" for o in inspection["offenses"]
                )
                correction = run(directory, case, case["mode"])
                written = path.read_bytes()
                syntax = [
                    o for o in correction["offenses"] if o["cop_name"] == "Lint/Syntax"
                ]
                assert not syntax or syntax == known_encoding_errors
                restored = restore_reference(original, current, written, case["codec"])
                steps.append(
                    dict(
                        input_hex=current.hex(),
                        inspection=inspection,
                        correction=correction,
                        written_hex=written.hex(),
                        restored_hex=restored.hex(),
                    )
                )
                for offense in correction["offenses"]:
                    if offense["cop_name"] != "Lint/Syntax":
                        assert offense["corrected"]
                        corrected.setdefault(key(offense), offense)
                if restored == current:
                    assert correction["exit"] == 0 and correction["offenses"] == []
                    assert written == current
                    break
                current = restored
            else:
                raise AssertionError("owned safe control did not converge")
        assert "あ(+'')" in current.decode(case["codec"])
        assert current.count(b"(") == original.count(b"(") + 1
        records.append(
            dict(
                id=case["id"],
                mode=case["mode"],
                codec=case["codec"],
                source_hex=case["source_hex"],
                steps=steps,
                expected_safe_hex=current.hex(),
                expected_exit=0,
                expected_offenses=[corrected[k] for k in sorted(corrected)],
            )
        )
    assert len(records) == 6 and raw_path.read_bytes() == raw_bytes
    data = dict(
        environment=environment,
        raw_fixture_sha256=hashlib.sha256(raw_bytes).hexdigest(),
        windows_text_write_lf_to_crlf=os.name == "nt",
        records=records,
    )
    out = ROOT / "tests/fixtures/capture_line_endings_safe.json"
    out.write_text(
        json.dumps(data, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    print(
        len(records),
        "safe chains",
        sum(len(r["steps"]) for r in records),
        "real CLI steps",
        hashlib.sha256(out.read_bytes()).hexdigest(),
    )


if __name__ == "__main__":
    main()
