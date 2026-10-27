"""RuboCop 1.89.0だけからdebugの出力eventとredirect境界を記録する。"""

import argparse
import concurrent.futures
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

SPACE = "Layout/SpaceAroundOperators"
SOURCE = "x=1\n"
CONFIG = "AllCops:\n  NewCops: disable\n  UseProjectIndex: false\n  TargetRubyVersion: 3.3\n"
AMBIENT = ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION")
TIMER = re.compile(r"Finished in [0-9]+(?:\.[0-9]+)?(?:e[+-]?\d+)? seconds(\r?\n)\Z")


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _environment():
    env = os.environ.copy()
    for key in AMBIENT:
        env.pop(key, None)
    return env


def _verify_runtime(ruby):
    source = (
        "gem 'parser', '=3.3.12.0'; gem 'rubocop', '=1.89.0'; "
        "require 'rubocop'; require 'json'; "
        "puts JSON.generate({ruby: RUBY_VERSION, parser: Parser::VERSION, "
        "rubocop: RuboCop::Version::STRING, platform: RUBY_PLATFORM})"
    )
    done = subprocess.run(
        [ruby, "-e", source], env=_environment(), capture_output=True, check=False, timeout=60
    )
    _require(done.returncode == 0 and not done.stderr, "実行runtimeを確認できない")
    value = json.loads(done.stdout)
    _require(
        (value["ruby"], value["parser"], value["rubocop"]) == ("4.0.7", "3.3.12.0", "1.89.0")
        and "mingw" in value["platform"],
        "実行runtimeの版が不一致",
    )
    return value


def _forms():
    one = {"case.rb": SOURCE}
    two = {"a.rb": SOURCE, "b.rb": "y=2\n"}
    normal = [
        ("json_out", ["-f", "json", "-o", "report.out"]),
        ("json_out_stderr", ["--stderr", "-f", "json", "-o", "report.out"]),
        ("simple_out", ["-f", "simple", "-o", "report.out"]),
        ("simple_out_stderr", ["--stderr", "-f", "simple", "-o", "report.out"]),
        ("simple_console", ["-f", "simple"]),
        ("simple_console_stderr", ["--stderr", "-f", "simple"]),
        ("json_console", ["-f", "json"]),
        ("json_console_stderr", ["--stderr", "-f", "json"]),
        ("json_stdin_out", ["-f", "json", "-o", "report.out"]),
        ("json_no_target", ["-f", "json", "-o", "report.out"]),
        ("mixed_slots", ["--stderr", "-f", "json", "-o", "report.out", "-f", "simple"]),
        ("simple_two", ["-f", "simple"]),
        ("simple_two_stderr", ["--stderr", "-f", "simple"]),
        ("json_two_out", ["-f", "json", "-o", "report.out"]),
        ("fail_fast_simple", ["--fail-fast", "-f", "simple"]),
        (
            "fail_fast_mixed",
            ["--fail-fast", "--stderr", "-f", "json", "-o", "report.out", "-f", "simple"],
        ),
        ("no_debug_json_out", ["-f", "json", "-o", "report.out"]),
        ("no_debug_simple_stderr", ["--stderr", "-f", "simple"]),
        ("no_offense", ["-f", "simple"]),
        ("stdin_safe_stderr", ["-a", "--stderr", "-f", "simple"]),
        ("default_progress_two", []),
    ]
    forms = []
    for name, options in normal:
        inputs = (
            two
            if name
            in {
                "simple_two",
                "simple_two_stderr",
                "json_two_out",
                "fail_fast_simple",
                "fail_fast_mixed",
                "default_progress_two",
            }
            else one
        )
        if name == "json_no_target":
            inputs = {}
        if name == "no_offense":
            inputs = {"case.rb": "x = 1\n"}
        stdin = name in {"json_stdin_out", "stdin_safe_stderr"}
        debug = not name.startswith("no_debug")
        target = ["--stdin", "case.rb"] if stdin else list(inputs)
        scanned = list(inputs)[:1] if "--fail-fast" in options else list(inputs)
        forms.append(
            {
                "id": name,
                "options": options,
                "inputs": inputs,
                "target": target,
                "scanned": scanned if debug else [],
                "debug": debug,
                "selector": SPACE,
                "coverage": "interface",
                "exit": 0 if name in {"json_no_target", "no_offense", "stdin_safe_stderr"} else 1,
            }
        )
    guards = [
        ("version", ["--version"], "phase_guard", 0, None),
        ("missing_only", ["--only"], "phase_guard", 2, "--only"),
        (
            "except_syntax",
            ["--except", "Lint/Syntax", "-f", "json", "case.rb"],
            "phase_guard",
            2,
            "Syntax checking",
        ),
        (
            "invalid_fail_level",
            ["--fail-level", "invalid", "case.rb"],
            "phase_guard",
            2,
            "--fail-level",
        ),
        (
            "unknown_cop",
            ["--only", "Unknown/Cop", "-f", "json", "-o", "report.out", "case.rb"],
            "deferred_error_residual",
            2,
            "Unknown/Cop",
        ),
        ("missing_config", ["case.rb"], "phase_guard", 2, "missing.yml"),
    ]
    forms += [
        {
            "id": name,
            "options": options,
            "inputs": one,
            "target": [],
            "debug": True,
            "selector": None,
            "coverage": coverage,
            "exit": code,
            "error_token": token,
        }
        for name, options, coverage, code, token in guards
    ]
    return forms


def _report(raw):
    value = json.loads(raw)
    metadata = value.pop("metadata")
    _require(metadata["rubocop_version"] == "1.89.0", "RuboCop版が不一致")
    _require(
        metadata["ruby_engine"] == "ruby" and metadata["ruby_version"] == "4.0.7", "Ruby版が不一致"
    )
    _require("mingw" in metadata["ruby_platform"], "Windows参照専用")
    return value


def _common_projection(text, cwd):
    text, timer_count = TIMER.subn(lambda match: "Finished in <elapsed> seconds" + match[1], text)
    reports = []
    result = ""
    while '{"metadata":' in text:
        start = text.index('{"metadata":')
        value, end = json.JSONDecoder().raw_decode(text[start:])
        reports.append(_report(json.dumps(value)))
        result += text[:start] + f"<JSON[{len(reports) - 1}]>"
        text = text[start + end :]
    return {
        "text": (result + text).replace(cwd, "<cwd>"),
        "reports": reports,
        "timer_count": timer_count,
    }


def _project_reference(data, cwd):
    text = data.decode()
    headers = []
    kept = []
    for line in text.splitlines(keepends=True):
        if line.startswith("configuration from "):
            headers.append(line.replace(cwd, "<cwd>"))
        elif line.startswith("Default configuration from "):
            headers.append("Default configuration from <reference-default-config>\r\n")
        else:
            kept.append(line)
    projection = _common_projection("".join(kept), cwd)
    projection["reference_headers"] = headers
    return projection


def _validate_report(report, target_count, inspected):
    _require([file["path"] for file in report["files"]] == inspected, "JSON path/順序が不一致")
    _require(report["summary"]["target_file_count"] == target_count, "JSON target件数が不一致")
    _require(report["summary"]["inspected_file_count"] == len(inspected), "JSON inspectionが未完了")
    _require(
        report["summary"]["offense_count"]
        == sum(len(file["offenses"]) for file in report["files"]),
        "JSON診断集計が不一致",
    )


def _record(job, args):
    index, form = job
    directory = Path(args.work).resolve() / form["id"]
    directory.mkdir(parents=True, exist_ok=False)
    (directory / "config.yml").write_bytes(CONFIG.encode())
    for name, source in form["inputs"].items():
        path = directory / name
        path.write_bytes(source.encode())
        # 120のmtime順差は本件へ混ぜず、全原本のstatを同じにする。
        os.utime(path, ns=(1_700_000_000_000_000_000,) * 2)
    config = "missing.yml" if form["id"] == "missing_config" else "config.yml"
    command = ["--config", config, "--cache", "false", "--no-color"]
    if form["debug"]:
        command += ["--debug"]
    if form["selector"]:
        command += ["--only", form["selector"]]
    command += form["options"] + form["target"]
    done = subprocess.run(
        [args.ruby, args.launcher] + command,
        cwd=directory,
        env=_environment(),
        input=SOURCE.encode(),
        capture_output=True,
        check=False,
        timeout=60,
    )
    _require(done.returncode == form["exit"], f"本家終了が不一致: {form['id']}")
    _require(b"An error occurred" not in done.stderr, "本家内部エラー")
    cwd = str(directory).replace("\\", "/")
    stdout = _project_reference(done.stdout, cwd)
    stderr = _project_reference(done.stderr, cwd)
    scanned = re.findall(r"Scanning <cwd>/([^\r\n]+)", stdout["text"] + stderr["text"])
    outputs = {}
    for name in ("report.out", "other.out"):
        path = directory / name
        if not path.exists():
            outputs[name] = None
            continue
        raw = path.read_bytes()
        outputs[name] = {
            "raw_hex": raw.hex(),
            "report": _report(raw) if raw.startswith(b"{") else None,
        }
    if form["coverage"] == "interface":
        _require(scanned == form["scanned"], "Scanning対象が不一致")
        _require(not stderr["reference_headers"], "configがstderrへ出た")
        _require(
            len(stdout["reference_headers"]) == (2 if form["debug"] else 0),
            "config metadataが未完了",
        )
        _require(
            stdout["timer_count"] == int(form["debug"]) and not stderr["timer_count"],
            "ensure timerのstreamが不一致",
        )
        actual_outputs = {name for name, value in outputs.items() if value is not None}
        _require(
            actual_outputs == ({"report.out"} if "-o" in form["options"] else set()),
            "保存先集合が不一致",
        )
        inspected = (
            list(form["inputs"])[:1] if "--fail-fast" in form["options"] else list(form["inputs"])
        )
        reports = (
            stdout["reports"]
            + stderr["reports"]
            + [value["report"] for value in outputs.values() if value and value["report"]]
        )
        for report in reports:
            _validate_report(report, len(form["inputs"]), inspected)
    else:
        if form.get("error_token"):
            _require(form["error_token"] in done.stderr.decode(), "想定した拒否ではない")
        timer = int(form["id"] in {"version", "unknown_cop", "missing_config"})
        _require(
            stdout["timer_count"] == timer and not stderr["timer_count"], "拒否のphaseが不一致"
        )
        if form["coverage"] == "deferred_error_residual":
            _require(scanned == ["case.rb"] and outputs["report.out"], "late error境界が不一致")
            _validate_report(outputs["report.out"]["report"], 1, [])
    for name, source in form["inputs"].items():
        _require((directory / name).read_bytes() == source.encode(), "原本byteが変わった")
    raw = {
        "args": command,
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
    }
    (directory / "raw.json").write_text(
        json.dumps(raw, ensure_ascii=False, indent=2), encoding="utf-8", newline="\n"
    )
    _require(cwd not in stdout["text"] + stderr["text"], "動的pathの未投影")
    return {
        "id": form["id"],
        "args": command,
        "inputs": form["inputs"],
        "stdin_source": SOURCE,
        "coverage": form["coverage"],
        "debug": form["debug"],
        "exit": done.returncode,
        "target_count": len(form["inputs"]) if form["coverage"] == "interface" else None,
        "residual_issue": 118 if form["coverage"] == "deferred_error_residual" else None,
        "stdout": stdout,
        "stderr": stderr,
        "outputs": outputs,
        "raw_stdout_sha256": hashlib.sha256(done.stdout).hexdigest(),
        "raw_stderr_sha256": hashlib.sha256(done.stderr).hexdigest(),
    }


def main():
    """動的metadataだけを投影し、候補を読まず有限の全記録が揃ってから保存する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "launcher", "work", "out"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    runtime = _verify_runtime(args.ruby)
    forms = _forms()
    _require(len(forms) == 27 and len({form["id"] for form in forms}) == 27, "有限件数が不一致")
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        cases = list(pool.map(lambda job: _record(job, args), enumerate(forms)))
    fixture = {
        "reference_version": {
            "rubocop": "1.89.0",
            "parser": "3.3.12.0",
            "ruby": "4.0.7",
            "platform": "Windows",
        },
        "config": CONFIG,
        "runtime": runtime,
        "cases": cases,
    }
    Path(args.out).write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2), encoding="utf-8", newline="\n"
    )
    print(f"recorded {len(cases)} cases")


if __name__ == "__main__":
    main()
