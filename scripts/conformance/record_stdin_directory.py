"""ディレクトリstdinの対象展開と共有補正本文を、固定した本家CLIから記録する。"""

import argparse
import concurrent.futures
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile

BASE = (
    "AllCops:\n  DisabledByDefault: true\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
    "Layout/SpaceAroundOperators:\n  Enabled: true\n"
)
FILES = {"src/a.rb": b"disk_a=9\n", "src/b.rb": b"disk_b=8\n"}


def child_environment():
    """親とGem探索を維持し、実Rubyの入力を変える4オプションだけを子copyから除く。"""
    environment = os.environ.copy()
    for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        environment.pop(key, None)
    return environment


def cases():
    """本家のAPIで確認した軸を固定し、候補の対象一覧を期待へ流用しない。"""
    rows = []

    def add(name, files=None, args=None, source=b"x=1\n", stdin="src", paths=None, **extra):
        files = FILES if files is None else files
        rows.append(
            {
                "name": name,
                "files": {path: content.hex() for path, content in files.items()},
                "args": args or [],
                "source_hex": source.hex(),
                "stdin": stdin,
                "config": BASE,
                "paths": sorted(files) if paths is None else paths,
                "kind": "json",
                **extra,
            }
        )

    for mode in [[], ["-a"], ["-A"]]:
        suffix = "inspect" if not mode else mode[0][1:]
        add("empty-" + suffix, {}, mode)
        add("shared-" + suffix, args=mode)
    add("dot", stdin=".")
    add("absolute", stdin="<ABS_SRC>")
    add("trailing-separator", stdin="src/")
    for name in ["named.rb", "", "missing.rb"]:
        add(
            "name-" + (name or "empty"),
            args=["--config", ".rubocop.yml"],
            stdin=name,
            paths=[None] if not name else [name],
        )
    discovery = {
        **FILES,
        "src/c.txt": b"not ruby\n",
        "src/.hidden.rb": b"disk=1\n",
        "src/.hidden/c.rb": b"disk=1\n",
        "src/skip.rb": b"disk=1\n",
    }
    visible = ["src/.hidden.rb", "src/a.rb", "src/b.rb", "src/c.txt"]
    excluded = BASE.replace(
        "Layout/SpaceAroundOperators:",
        "  Exclude:\n    - 'src/skip.rb'\nLayout/SpaceAroundOperators:",
    )
    for flags in [[], ["--force-exclusion"], ["--only-recognized-file-types"]]:
        add(
            "discovery-" + (flags[0][2:] if flags else "default"),
            discovery,
            flags,
            paths=visible,
            config=excluded,
        )
    add(
        "hidden-root",
        {".hidden/a.txt": b"disk=1\n", ".hidden/.child/b.rb": b"disk=1\n"},
        stdin=".hidden",
    )
    add(
        "ordering",
        {
            "src/foo/z.rb": b"disk=1\n",
            "src/foo.rb": b"disk=1\n",
            "src/foo0.rb": b"disk=1\n",
            "src/日.rb": b"disk=1\n",
        },
    )
    nested = {"src/a.rb": b"disk=1\n", "src/z/b.rb": b"disk=1\n", "src/zz/c.rb": b"disk=1\n"}
    quote_config = BASE + "Style/StringLiterals:\n  Enabled: true\n  EnforcedStyle: single_quotes\n"
    child = "AllCops:\n  NewCops: disable\nStyle/StringLiterals:\n  EnforcedStyle: double_quotes\n"
    for flags in [[], ["-A"]]:
        add(
            "nested-" + ("A" if flags else "inspect"),
            nested,
            flags + ["--only", "Style/StringLiterals"],
            source=b"value = 'x'\n",
            config=quote_config,
            configs={"src/z/.rubocop.yml": child},
        )
    fast = {**FILES, "src/c.rb": b"disk_c=7\n"}
    mtimes = {"src/a.rb": 1_700_000_001, "src/b.rb": 1_700_000_003, "src/c.rb": 1_700_000_002}
    for suffix, flags, inspected in [
        ("inspect", [], ["src/b.rb"]),
        ("a", ["-a"], ["src/b.rb", "src/c.rb", "src/a.rb"]),
        ("a-fail-corrected", ["-a", "--fail-level", "autocorrect"], ["src/b.rb"]),
    ]:
        add(
            "fast-" + suffix,
            fast,
            ["--fail-fast"] + flags,
            paths=inspected,
            target_count=3,
            mtimes=mtimes,
        )
    add("list", args=["--list-target-files"], kind="list")
    add(
        "list-fast",
        fast,
        ["--list-target-files", "--fail-fast"],
        paths=["src/b.rb", "src/c.rb", "src/a.rb"],
        kind="list",
        mtimes=mtimes,
    )
    for name, files, source, payload in [
        ("empty", {}, b"x=1", b"x=1"),
        ("shared", FILES, b"x=1", b"x = 1"),
        ("unicode", FILES, "あ=1".encode(), "あ = 1".encode()),
        ("noop", FILES, b"x = 1", b"x = 1"),
    ]:
        add("payload-" + name, files, ["-a"], source, kind="payload", payload_hex=payload.hex())
    add(
        "shared-explicit-crlf-view",
        args=["-a", "--only", "Layout/SpaceAroundOperators,Layout/EndOfLine"],
        source=b"x=1\r\n",
        config=BASE + "Layout/EndOfLine:\n  Enabled: true\n  EnforcedStyle: crlf\n",
    )
    return rows


def stdout_template(raw, folder):
    """既知の作業rootだけをheader行頭で置き換え、本文の生bytesは保つ。"""
    head, separator, payload = raw.partition(b"=" * 20)
    if not separator:
        return raw
    prefix = (folder.as_posix() + "/").encode("utf-8")
    lines = (
        b"<WORKDIR>/" + line[len(prefix) :] if line.startswith(prefix) else line
        for line in head.splitlines(keepends=True)
    )
    return b"".join(lines) + separator + payload


def record(command, case, environment=None):
    """完了数・対象順・原本を検証してから、全JSONと生の出力を保存する。"""
    environment = child_environment() if environment is None else environment
    with tempfile.TemporaryDirectory() as directory:
        folder = pathlib.Path(directory)
        (folder / "src").mkdir()
        (folder / ".rubocop.yml").write_text(case["config"], encoding="utf-8", newline="\n")
        for path, text in case.get("configs", {}).items():
            target = folder / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text, encoding="utf-8", newline="\n")
        for path, hexed in case["files"].items():
            target = folder / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(bytes.fromhex(hexed))
        for path, timestamp in case.get("mtimes", {}).items():
            os.utime(folder / path, (timestamp, timestamp))
        stdin = str(folder / "src") if case["stdin"] == "<ABS_SRC>" else case["stdin"]
        fmt = "files" if case["kind"] == "payload" else "json"
        args = ["--cache", "false", "-f", fmt]
        if "--only" not in case["args"]:
            args += ["--only", "Layout/SpaceAroundOperators"]
        process = subprocess.run(
            command + args + case["args"] + ["--stdin", stdin],
            cwd=folder,
            env=environment,
            input=bytes.fromhex(case["source_hex"]),
            capture_output=True,
            timeout=120,
            check=False,
        )
        if process.returncode not in (0, 1) or process.stderr:
            raise ValueError(
                f"{case['name']}: unexpected reference failure: {process.returncode} {process.stderr!r}"
            )
        if any(
            (folder / path).read_bytes().hex() != original
            for path, original in case["files"].items()
        ):
            raise ValueError(f"{case['name']}: stdin wrote to disk")
        expected = {
            "exit": process.returncode,
            "stdout_hex": stdout_template(process.stdout, folder).hex(),
            "stderr_hex": process.stderr.hex(),
        }
        if case["kind"] == "json":
            report = json.loads(process.stdout)
            report.pop("metadata", None)
            actual_paths = [file["path"] for file in report["files"]]
            if actual_paths != case["paths"]:
                raise ValueError(f"{case['name']}: unexpected paths: {actual_paths!r}")
            summary = report["summary"]
            inspected = summary["inspected_file_count"]
            target_count = summary["target_file_count"]
            offense_count = summary["offense_count"]
            if (
                type(inspected) is not int
                or type(target_count) is not int
                or type(offense_count) is not int
                or inspected != len(case["paths"])
                or target_count != case.get("target_count", len(case["paths"]))
                or offense_count != sum(len(file["offenses"]) for file in report["files"])
            ):
                raise ValueError(f"{case['name']}: incomplete inspection: {summary!r}")
            expected["report"] = report
        elif case["kind"] == "list":
            paths = process.stdout.decode("utf-8").replace("\r\n", "\n").splitlines()
            if paths != case["paths"]:
                raise ValueError(f"{case['name']}: unexpected listing: {paths!r}")
        else:
            separator = b"=" * 20
            head, found, payload = process.stdout.partition(separator)
            if not found or separator in payload:
                raise ValueError(f"{case['name']}: missing or duplicate stdin boundary")
            payload = payload.removeprefix(b"\r\n").removeprefix(b"\n")
            if payload.hex() != case["payload_hex"]:
                raise ValueError(f"{case['name']}: unexpected shared body: {payload!r}")
        return {
            **case,
            "raw_stdout_sha256": hashlib.sha256(process.stdout).hexdigest(),
            "_raw_stdout_hex": process.stdout.hex(),
            "expected": expected,
        }


def echo_cases():
    """承認済み102の保護期待へ同じ原本を結び、複数対象の最終echoを検査する。"""
    source_fixture = (
        pathlib.Path(__file__).resolve().parents[2] / "tests/fixtures/windows_stdio.json"
    )
    oracle = json.loads(source_fixture.read_text(encoding="utf-8"))
    approved = {
        row["name"]: row
        for key in ["safety_cases", "literal_boundary_cases", "malformed_controls"]
        for row in oracle[key]
    }
    rows = []

    def add(name, source, mode="-A", files=None, **extra):
        rows.append(
            {
                "name": name,
                "source_hex": source.hex(),
                "mode": mode,
                "files": {
                    path: content.hex()
                    for path, content in (FILES if files is None else files).items()
                },
                "config": BASE,
                "only": "Layout/SpaceAroundOperators",
                "json_only": False,
                "policy": "reference",
                **extra,
            }
        )

    for mode in ["-a", "-A"]:
        add("zero-echo-" + mode[1:], b"x = 1\r\n", mode, {})
    add("last-clean-lf", b"x=1\n", "-a")
    add("last-clean-crlf", b"x=1\r\n")
    add(
        "shared-raw-end-of-line",
        b"x=1\r\n",
        config=BASE + "Layout/EndOfLine:\n  Enabled: true\n  EnforcedStyle: native\n",
        only="Layout/SpaceAroundOperators,Layout/EndOfLine",
        json_only=True,
    )
    add(
        "shared-continuation-crlf",
        b"x=1\r\nvalue = 'a' \\\r\n  ' b'\r\n",
        files={"src/a.rb": b"disk=1\n", "src/z/b.rb": b"disk=1\n"},
        config=BASE
        + "Layout/LineContinuationLeadingSpace:\n  Enabled: true\n  EnforcedStyle: trailing\n",
        configs={
            "src/z/.rubocop.yml": "AllCops:\n  NewCops: disable\nLayout/LineContinuationLeadingSpace:\n  EnforcedStyle: leading\n"
        },
        only="Layout/SpaceAroundOperators,Layout/LineContinuationLeadingSpace",
    )
    add("list-echo", b"x=1\n", "-a", list_only=True)
    add(
        "shared-mixed-data",
        b"x=1000\r\nSTDOUT.binmode; print DATA.binmode.read.unpack1('H*')\n__END__\nA\r\nB\nC\rD\n",
        files={"src/a.rb": b"disk=1\n", "src/z/b.rb": b"disk=1\n"},
        config=BASE + "Style/NumericLiterals:\n  Enabled: true\n  MinDigits: 5\n",
        configs={
            "src/z/.rubocop.yml": "AllCops:\n  NewCops: disable\nStyle/NumericLiterals:\n  MinDigits: 3\n"
        },
        only="Layout/SpaceAroundOperators,Style/NumericLiterals",
        policy="preserve-tail",
        runtime=True,
    )
    add(
        "shared-ascii8bit",
        b"# coding: ASCII-8BIT\r\nx=1\r\nvalue = 'caf\xe9'\r\nrequire 'json'\r\nSTDOUT.binmode; STDOUT.write(JSON.generate(bytes: value.bytes, encoding: value.encoding.name))\r\n",
        files={"src/a.rb": b"disk=1\n", "src/z/b.rb": b"disk=1\n", "src/zz/c.rb": b"disk=1\n"},
        config=BASE + "Style/StringLiterals:\n  Enabled: true\n  EnforcedStyle: single_quotes\n",
        configs={
            "src/z/.rubocop.yml": "AllCops:\n  NewCops: disable\nStyle/StringLiterals:\n  EnforcedStyle: double_quotes\n"
        },
        only="Layout/SpaceAroundOperators,Style/StringLiterals",
        runtime=True,
    )
    add(
        "configs-cycle-crlf",
        b"value = 'x'\r\n",
        files={"src/a.rb": b"disk=1\n", "src/z/b.rb": b"disk=1\n", "src/zz/c.rb": b"disk=1\n"},
        config=BASE + "Style/StringLiterals:\n  Enabled: true\n  EnforcedStyle: single_quotes\n",
        configs={
            "src/z/.rubocop.yml": "AllCops:\n  NewCops: disable\nStyle/StringLiterals:\n  EnforcedStyle: double_quotes\n"
        },
        only="Style/StringLiterals",
    )
    for name in [
        "data",
        "nul-tail",
        "escaped-double-string",
        "escaped-double-string-rewritten",
        "heredoc-crlf",
    ]:
        row = approved[name]
        add(
            name,
            bytes.fromhex(row["source_hex"]),
            policy="approved102",
            binding=name,
            safe_payload_hex=row["safe"]["stdout_hex"],
            safe_exit=row["safe"]["exit"],
        )
    add("invalid", bytes.fromhex(approved["unterminated-double"]["source_hex"]))
    add("file-rollback", b"result = foo, '\\n'\n", only="Lint/UselessAssignment", policy="original")
    add("json-only-heredoc", bytes.fromhex(approved["heredoc-crlf"]["source_hex"]), json_only=True)
    add(
        "json-only-escaped",
        bytes.fromhex(approved["escaped-double-string"]["source_hex"]),
        json_only=True,
    )
    add(
        "rollback-after-shared-rewrite",
        b"x=1\r\nresult = foo, '\\n'\r\n",
        files={"src/a.rb": b"disk=1\n", "src/b.rb": b"disk=1\n", "src/c.rb": b"disk=1\n"},
        config=BASE + "Lint/UselessAssignment:\n  Enabled: true\n  Exclude:\n    - 'src/a.rb'\n",
        only="Layout/SpaceAroundOperators,Lint/UselessAssignment",
        policy="original",
    )
    for mode in ["-a", "-A"]:
        add(
            "windows1252-success-" + mode[1:],
            b"# coding: Windows-1252\nx=1\nvalue = 'caf\xe9'\nrequire 'json'\nSTDOUT.binmode; STDOUT.write(JSON.generate(bytes: value.bytes, encoding: value.encoding.name))\n",
            mode,
            files={"sample.rb": b"disk=9\n"},
            stdin_name="sample.rb",
            policy="original-encoding",
            runtime=True,
        )
    return rows, hashlib.sha256(source_fixture.read_bytes()).hexdigest()


def record_echo(command, case, environment=None):
    """本家の最終payloadと全JSONを記録し、保護時は原本の検査を別に得る。"""
    environment = child_environment() if environment is None else environment
    with tempfile.TemporaryDirectory() as directory:
        folder = pathlib.Path(directory)
        (folder / "src").mkdir()
        (folder / ".rubocop.yml").write_text(case["config"], encoding="utf-8", newline="\n")
        for path, text in case.get("configs", {}).items():
            target = folder / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text, encoding="utf-8", newline="\n")
        for path, hexed in case["files"].items():
            target = folder / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(bytes.fromhex(hexed))
        args = ["--cache", "false", "--only", case["only"], "-f", "json"]
        if case.get("list_only"):
            args = [
                "--cache",
                "false",
                "--only",
                case["only"],
                "-f",
                "files",
                "--list-target-files",
            ]
        elif not case["json_only"]:
            args += ["-o", "report.json", "-f", "files"]
        source = bytes.fromhex(case["source_hex"])
        stdin_name = case.get("stdin_name", "src")
        result = subprocess.run(
            command + args + [case["mode"], "--stdin", stdin_name],
            cwd=folder,
            env=environment,
            input=source,
            capture_output=True,
            timeout=120,
            check=False,
        )
        if result.returncode not in (0, 1) or result.stderr:
            raise ValueError(
                f"{case['name']}: unexpected reference result {result.returncode}: {result.stderr!r}"
            )
        expected_paths = sorted(case["files"]) if stdin_name == "src" else [stdin_name]
        if case.get("list_only"):
            listed = result.stdout.partition(b"=" * 20)[0].decode("utf-8").splitlines()
            if listed != expected_paths:
                raise ValueError(f"{case['name']}: incomplete target listing")
            report = None
        else:
            raw_report = (
                result.stdout if case["json_only"] else (folder / "report.json").read_bytes()
            )
            report = json.loads(raw_report)
            report.pop("metadata", None)
            if [file["path"] for file in report["files"]] != expected_paths:
                raise ValueError(f"{case['name']}: incomplete directory report")
            counts = {
                "target_file_count": len(expected_paths),
                "inspected_file_count": len(expected_paths),
                "offense_count": sum(len(file["offenses"]) for file in report["files"]),
            }
            if any(
                type(report["summary"].get(key)) is not int or report["summary"][key] != count
                for key, count in counts.items()
            ):
                raise ValueError(f"{case['name']}: incomplete reference summary")
        expected = {
            "exit": result.returncode,
            "report": report,
            "reference_report": report,
            "stdout_hex": stdout_template(result.stdout, folder).hex(),
            "stderr_hex": result.stderr.hex(),
        }
        if not case["json_only"]:
            _, separator, payload = result.stdout.partition(b"=" * 20)
            if not separator:
                raise ValueError(f"{case['name']}: reference did not echo stdin")
            expected["reference_payload_hex"] = (
                payload.removeprefix(b"\r\n").removeprefix(b"\n").hex()
            )
            expected["payload_hex"] = expected["reference_payload_hex"]
        if case["policy"] == "approved102":
            expected["exit"] = case["safe_exit"]
            expected["payload_hex"] = case["safe_payload_hex"]
        if case["policy"] == "preserve-tail":
            # DATA 以外は実本家の最終 code、未変更 tail は原本の実バイトから構成する。
            before = source.index(b"__END__\n") + len(b"__END__\n")
            payload = bytes.fromhex(expected["reference_payload_hex"])
            after = payload.index(b"__END__\r\n") + len(b"__END__\r\n")
            expected["payload_hex"] = (payload[:after] + source[before:]).hex()
        if case["policy"] == "original-encoding":
            # 限定した原本のASCII編集とnative改行だけを導出し、値のE9をUTF-8化しない。
            if source.count(b"x=1") != 1 or b"\r" in source:
                raise ValueError("unexpected Windows-1252 control source")
            expected["payload_hex"] = source.replace(b"x=1", b"x = 1").replace(b"\n", b"\r\n").hex()
        if case["policy"] == "original" or expected["exit"] == 2:
            if not case["json_only"]:
                (folder / "report.json").unlink()
            inspection = subprocess.run(
                command + args + ["--stdin", stdin_name],
                cwd=folder,
                env=environment,
                input=source,
                capture_output=True,
                timeout=120,
                check=False,
            )
            if inspection.returncode not in (0, 1) or inspection.stderr:
                raise ValueError(f"{case['name']}: original inspection failed")
            inspected = (
                inspection.stdout if case["json_only"] else (folder / "report.json").read_bytes()
            )
            expected["report"] = json.loads(inspected)
            expected["report"].pop("metadata", None)
            if [file["path"] for file in expected["report"]["files"]] != expected_paths:
                raise ValueError(f"{case['name']}: original inspection is incomplete")
            expected["payload_hex"] = source.hex()
            expected["exit"] = 2
        if case.get("runtime"):
            values = {}
            for side, data in [
                ("original", source),
                ("reference", bytes.fromhex(expected["reference_payload_hex"])),
                ("safe", bytes.fromhex(expected["payload_hex"])),
            ]:
                (folder / "value.rb").write_bytes(data)
                value = subprocess.run(
                    [command[0], "value.rb"],
                    cwd=folder,
                    env=environment,
                    capture_output=True,
                    timeout=30,
                    check=False,
                )
                values[side] = {
                    "exit": value.returncode,
                    "stdout_hex": value.stdout.hex(),
                    "stderr_hex": value.stderr.hex(),
                }
            if values["original"]["exit"] != 0 or values["safe"] != values["original"]:
                raise ValueError(f"{case['name']}: safe output changed the original value")
            expected["values"] = values
        if any(
            (folder / path).read_bytes().hex() != original
            for path, original in case["files"].items()
        ):
            raise ValueError(f"{case['name']}: stdin changed disk input")
        reference = {
            "exit": result.returncode,
            "report": report,
            "stdout_hex": stdout_template(result.stdout, folder).hex(),
            "stderr_hex": result.stderr.hex(),
        }
        if "reference_payload_hex" in expected:
            reference["payload_hex"] = expected["reference_payload_hex"]
        if not case["json_only"]:
            head, separator, payload = bytes.fromhex(expected["stdout_hex"]).partition(b"=" * 20)
            ending = b"\r\n" if payload.startswith(b"\r\n") else b"\n"
            expected["stdout_hex"] = (
                head + separator + ending + bytes.fromhex(expected["payload_hex"])
            ).hex()
        return {
            **case,
            "raw_stdout_sha256": hashlib.sha256(result.stdout).hexdigest(),
            "_raw_stdout_hex": result.stdout.hex(),
            "reference": reference,
            "expected": expected,
        }


def main():
    """記録のRuby/gem版を固定し、Sonicopを起動せずに公開fixtureを生成する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--raw-output", type=pathlib.Path)
    parser.add_argument("--echo-only", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as directory:
        launcher = pathlib.Path(directory) / "reference.rb"
        launcher.write_text(
            "gem 'rubocop', '1.89.0'\n"
            "gem 'parser', '3.3.12.0'\n"
            "gem 'prism', '1.8.1'\n"
            "gem 'rubocop-ast', '1.50.0'\n"
            "abort 'Ruby 4.0.7 is required' unless RUBY_VERSION == '4.0.7'\n"
            "abort 'Windows Ruby is required' unless /mingw|mswin/.match?(RUBY_PLATFORM)\n"
            "ENV.delete('RUBOCOP_TARGET_RUBY_VERSION')\n"
            "load Gem.bin_path('rubocop', 'rubocop', '1.89.0')\n",
            encoding="utf-8",
            newline="\n",
        )
        command = [args.ruby, str(launcher)]
        environment = child_environment()
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            records = (
                json.loads(args.output.read_text(encoding="utf-8"))["cases"]
                if args.echo_only
                else list(pool.map(lambda case: record(command, case, environment), cases()))
            )
            echo_rows, binding = echo_cases()
            echoes = list(pool.map(lambda case: record_echo(command, case, environment), echo_rows))
    raw_outputs = {
        group: {row["name"]: row.pop("_raw_stdout_hex", None) for row in rows}
        for group, rows in [("cases", records), ("echo_cases", echoes)]
    }
    if args.raw_output:
        args.raw_output.write_text(
            json.dumps(raw_outputs, indent=2) + "\n", encoding="utf-8", newline="\n"
        )
    args.output.write_text(
        json.dumps(
            {
                "rubocop": "1.89.0",
                "parser": "3.3.12.0",
                "ruby": "4.0.7",
                "stdout_template": "exact workdir forward-slash prefix at header line starts -> <WORKDIR>/; payload unchanged",
                "cases": records,
                "echo_source_fixture_sha256": binding,
                "echo_cases": echoes,
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
        newline="\n",
    )
    print(f"recorded {len(records)} directory cases and {len(echoes)} final echo cases")


if __name__ == "__main__":
    main()
