#!/usr/bin/env python3
"""所有する最小入力だけを実RuboCopへ渡し、混在改行/単独CRの補正境界を記録する。"""
import hashlib
import json
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ONLY = "Layout/SpaceBeforeFirstArg,Lint/AmbiguousOperator,Lint/AmbiguousRegexpLiteral"
CONFIG = "AllCops:\n  TargetRubyVersion: 2.7\n  ParserEngine: parser_whitequark\n"
ORACLE = ["ruby", "-e", "gem 'rubocop', '= 1.89.0'; load Gem.bin_path('rubocop','rubocop','1.89.0')", "--"]


def preserve_reference_code(original, reference, codec):
    # 本家の実編集をcodeに使い、親49/60の明示方針でcopyした改行とDATAを保持する。
    before = original.decode(codec).split("\n")
    after = reference.decode("utf-8").split("\n")
    marker = next(i for i, line in enumerate(before) if line.rstrip("\r") == "__END__")
    assert next(i for i, line in enumerate(after) if line.rstrip("\r") == "__END__") == marker
    code = ""
    for i in range(marker + 1):
        assert "\r" not in after[i]
        code += after[i] + ("\r\n" if before[i].endswith("\r") else "\n")
    return code.encode(codec) + "\n".join(before[marker + 1:]).encode(codec)


def main():
    originals = []
    for shape, encoding, codec, clash in [
        ("mixed_initial", "EUC-JP", "euc_jp", False),
        ("mixed_bare_later", "Windows-31J", "cp932", True),
        ("pure_bare_later", "Windows-31J", "cp932", True),
    ]:
        for danger in [True, False]:
            if danger:
                code = f"# encoding: {encoding}\n/(?<あ>x)/{'e' if codec == 'euc_jp' else 's'} =~ 'x'\nあ{'    ' if clash else ' '}+''\n"
            else:
                code = f"# encoding: {encoding}\ndef local_scope\n  /(?<あ>x)/{'e' if codec == 'euc_jp' else 's'} =~ 'x'\nend\ndef あ(value)\n  value\nend\nあ{'    ' if clash else ' '}+''\n"
            lines = (code + "__END__\n").splitlines()
            if shape.startswith("pure"):
                text = "\r\n".join(lines) + "\r\n" + "a\r\r\nb\r\n"
            else:
                text = "".join(line + ("\r\n" if i % 2 == 0 else "\n") for i, line in enumerate(lines))
                text += "a\r\r\nb\n" if clash else "a\r\nb\n"
            originals.append(dict(id=shape + ("_guard" if danger else "_control"), shape=shape, danger=danger, encoding=encoding, codec=codec, source_hex=text.encode(codec).hex()))
    records = []
    for case in originals:
        for mode in ["inspect", "-a", "-A"]:
            with tempfile.TemporaryDirectory() as tmp:
                directory = Path(tmp)
                (directory / ".rubocop.yml").write_text(CONFIG)
                raw = bytes.fromhex(case["source_hex"])
                path = directory / "example.rb"
                path.write_bytes(raw)
                args = ORACLE + ["--cache", "false", "--only", ONLY, "-f", "json"]
                if mode != "inspect":
                    args.append(mode)
                result = subprocess.run(args + ["example.rb"], cwd=directory, capture_output=True, timeout=90)
                report = json.loads(result.stdout)
                assert report["summary"]["target_file_count"] == report["summary"]["inspected_file_count"] == 1
                assert len(report["files"]) == 1 and report["files"][0]["path"] == "example.rb"
                assert result.returncode in [0, 1]
                written = path.read_bytes()
                expected = raw if mode == "inspect" or case["danger"] else preserve_reference_code(raw, written, case["codec"])
                records.append(dict(**case, mode=mode, only=ONLY, config=CONFIG, upstream_exit=result.returncode, upstream_offenses=report["files"][0]["offenses"], upstream_written_hex=written.hex(), expected_safe_hex=expected.hex()))
    out = ROOT / "tests/fixtures/capture_line_endings.json"
    out.write_text(json.dumps(dict(rubocop_version="1.89.0", ruby_version=subprocess.check_output(["ruby", "--version"], text=True).strip(), records=records), ensure_ascii=False, indent=2) + "\n")
    print(len(originals), "inputs", len(records), "official records", hashlib.sha256(out.read_bytes()).hexdigest())


if __name__ == "__main__":
    main()
