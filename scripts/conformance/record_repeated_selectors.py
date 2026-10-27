"""RuboCop 1.89.0のselector上書きと、別原因の残差を分けて記録する。"""

import argparse
import concurrent.futures
import json
import os
import subprocess
from pathlib import Path

SPACE = "Layout/SpaceAroundOperators"
STRING = "Style/StringLiterals"
UNKNOWN = "Unknown/Cop"
SOURCE = 'x=1\nmessage = "text"\n'
YAML = "AllCops:\n  NewCops: disable\n  DisabledByDefault: true\n  TargetRubyVersion: 2.7\nLayout/SpaceAroundOperators:\n  Enabled: true\nStyle/StringLiterals:\n  Enabled: true\n"


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _forms():
    normal = [
        ("only_space_string", ["--only", SPACE, "--only", STRING]),
        ("only_string_space", ["--only", STRING, "--only", SPACE]),
        ("only_same", ["--only", SPACE, "--only", SPACE]),
        ("only_three", ["--only", UNKNOWN, "--only", STRING, "--only", SPACE]),
        ("only_equals", ["--only=" + SPACE, "--only=" + STRING]),
        ("only_unknown_first", ["--only", UNKNOWN, "--only", SPACE]),
        ("only_forbidden_first", ["--only", "Lint/RedundantCopDisableDirective", "--only", SPACE]),
        ("only_empty_first", ["--only=", "--only", SPACE]),
        ("only_csv_replaced", ["--only", SPACE + "," + STRING, "--only", SPACE]),
        ("only_last_csv", ["--only", SPACE, "--only", STRING + "," + SPACE]),
        ("except_space_string", ["--except", SPACE, "--except", STRING]),
        ("except_string_space", ["--except", STRING, "--except", SPACE]),
        ("except_same", ["--except", SPACE, "--except", SPACE]),
        ("except_equals", ["--except=" + SPACE, "--except=" + STRING]),
        ("except_unknown_first", ["--except", UNKNOWN, "--except", SPACE]),
        ("except_syntax_first", ["--except", "Lint/Syntax", "--except", STRING]),
        ("except_csv_replaced", ["--except", SPACE + "," + STRING, "--except", SPACE]),
        ("except_last_csv", ["--except", SPACE, "--except", STRING + "," + SPACE]),
        (
            "independent_selectors",
            [
                "--only",
                STRING,
                "--only",
                SPACE + "," + STRING,
                "--except",
                SPACE,
                "--except",
                STRING,
            ],
        ),
        ("replacement_then_exclusion", ["--only", SPACE, "--only", STRING, "--except", STRING]),
    ]
    result = [{"id": name, "options": options, "coverage": "full_json"} for name, options in normal]
    result += [
        {
            "id": "dot_env_cli",
            "options": ["--only", SPACE],
            "dotfile": "--only Unknown/Cop",
            "env_options": "--only " + STRING,
            "coverage": "full_json",
        },
        {
            "id": "env_beats_dot",
            "options": [],
            "dotfile": "--only " + SPACE,
            "env_options": "--only " + STRING,
            "coverage": "full_json",
        },
        {
            "id": "cli_beats_dot",
            "options": ["--only", STRING],
            "dotfile": "--only " + SPACE,
            "coverage": "full_json",
        },
        {
            "id": "except_all_sources",
            "options": ["--except", STRING],
            "dotfile": "--except " + SPACE,
            "env_options": "--except " + SPACE,
            "coverage": "full_json",
        },
        {
            "id": "json_out_pair",
            "custom": [
                "-f",
                "json",
                "-o",
                "first.json",
                "--only",
                SPACE,
                "--only",
                STRING,
                "-f",
                "json",
                "-o",
                "second.json",
            ],
            "coverage": "full_json",
        },
        {
            "id": "json_out_prior",
            "custom": [
                "-o",
                "ignored.json",
                "--only",
                SPACE,
                "--only",
                STRING,
                "-f",
                "json",
                "-o",
                "kept.json",
            ],
            "coverage": "full_json",
        },
        {
            "id": "json_out_first_wins",
            "custom": [
                "-f",
                "json",
                "-o",
                "first.json",
                "--only",
                SPACE,
                "-o",
                "ignored.json",
                "--only",
                STRING,
            ],
            "coverage": "full_json",
        },
        {
            "id": "json_out_interleaved",
            "custom": [
                "-f",
                "json",
                "-o",
                "first.json",
                "--only",
                UNKNOWN,
                "-f",
                "json",
                "--only",
                SPACE,
                "-o",
                "second.json",
            ],
            "coverage": "full_json",
        },
    ]
    negative = [
        ("only_missing_first", ["--only", "--only", SPACE], [], "--only"),
        ("only_missing_last", ["--only", SPACE], ["--only"], "--only"),
        ("except_missing_first", ["--except", "--except", SPACE], [], "--except"),
        ("except_missing_last", ["--except", SPACE], ["--except"], "--except"),
        ("except_syntax_last", ["--except", STRING, "--except", "Lint/Syntax"], [], "Syntax"),
        (
            "only_forbidden_last",
            ["--only", SPACE, "--only", "Lint/RedundantCopDisableDirective"],
            [],
            "RedundantCopDisableDirective",
        ),
        (
            "invalid_earlier_severity",
            ["--fail-level", "invalid", "--fail-level", "error", "--only", SPACE, "--only", STRING],
            [],
            "--fail-level",
        ),
        (
            "invalid_earlier_limit",
            [
                "--exclude-limit",
                "invalid",
                "--exclude-limit",
                "1",
                "--only",
                SPACE,
                "--only",
                STRING,
            ],
            [],
            "--exclude-limit",
        ),
        (
            "enable_disable",
            ["--enable-all-cops", "--disable-all-cops", "--only", SPACE, "--only", STRING],
            [],
            "--enable-all-cops",
        ),
        (
            "disable_enable",
            ["--disable-all-cops", "--enable-all-cops", "--only", SPACE, "--only", STRING],
            [],
            "--disable-all-cops",
        ),
        (
            "cache_root_conflict",
            ["--cache-root", "cache", "--only", SPACE, "--only", STRING],
            [],
            "--cache",
        ),
        ("lsp_editor", ["--lsp", "--editor-mode", "--only", SPACE, "--only", STRING], [], "--lsp"),
    ]
    result += [
        {
            "id": name,
            "options": options,
            "tail": tail,
            "coverage": "rejection_envelope",
            "error_token": token,
        }
        for name, options, tail, token in negative
    ]
    result += [
        {
            "id": "only_unknown_last",
            "options": ["--only", SPACE, "--only", UNKNOWN],
            "coverage": "rejection_only",
            "error_token": UNKNOWN,
            "residual_issue": 118,
        },
        {
            "id": "except_unknown_last",
            "options": ["--except", SPACE, "--except", UNKNOWN],
            "coverage": "rejection_only",
            "error_token": UNKNOWN,
            "residual_issue": 118,
        },
    ]
    for flag in ("--only", "--except"):
        result.append(
            {
                "id": flag[2:] + "_empty_last",
                "options": [flag, SPACE, flag + "="],
                "single": [flag + "="],
                "coverage": "single_equivalence",
                "residual_issue": 117,
            }
        )
    return result


def _report(data):
    report = json.loads(data)
    metadata = report.pop("metadata")
    _require(
        metadata["rubocop_version"] == "1.89.0"
        and metadata["ruby_engine"] == "ruby"
        and metadata["ruby_version"] == "4.0.7",
        "本家Ruby/RuboCop版が不一致",
    )
    _require("mingw" in metadata["ruby_platform"], "本記録はWindows参照専用")
    return report


def _record(job, args):
    index, form, stdin = job
    mode = "stdin" if stdin else "file"
    directory = Path(args.work) / f"{index:02}-{mode}"
    # 既存のdotfileやformatter出力を新しいOracle記録へ混ぜない。
    directory.mkdir(parents=True, exist_ok=False)
    (directory / ".rubocop.yml").write_bytes(YAML.encode())
    (directory / "input.rb").write_bytes(SOURCE.encode())
    if "dotfile" in form:
        (directory / ".rubocop").write_bytes(form["dotfile"].encode())
    env = os.environ.copy()
    for key in ("RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION", "RUBYOPT", "RUBYLIB"):
        env.pop(key, None)
    if "env_options" in form:
        env["RUBOCOP_OPTS"] = form["env_options"]
    target = ["--stdin", "input.rb"] if stdin else ["input.rb"]

    def invoke(options, custom=False):
        command = (
            ["--cache", "false"]
            + ([] if custom else ["-f", "json"])
            + options
            + target
            + form.get("tail", [])
        )
        done = subprocess.run(
            [args.ruby, args.launcher] + command,
            cwd=directory,
            env=env,
            input=SOURCE.encode(),
            capture_output=True,
            check=False,
            timeout=60,
        )
        _require(done.returncode in (0, 1, 2), "本家CLIが異常終了")
        result = {
            "args": command,
            "exit": done.returncode,
            "stdout_hex": done.stdout.hex(),
            "stderr_hex": done.stderr.hex(),
        }
        if done.stdout.startswith(b"{"):
            result["report"] = _report(done.stdout)
        return result

    reference = invoke(form.get("custom", form.get("options", [])), "custom" in form)
    outputs = {}
    for name in ("first.json", "second.json", "ignored.json", "kept.json"):
        path = directory / name
        if path.exists():
            data = path.read_bytes()
            outputs[name] = {"raw_hex": data.hex(), "report": _report(data)}
        else:
            outputs[name] = None
    if form["coverage"] == "full_json":
        _require(
            reference["exit"] in (0, 1) and not reference["stderr_hex"], "正常対照の本家実行が失敗"
        )
        reports = ([reference["report"]] if "report" in reference else []) + [
            value["report"] for value in outputs.values() if value is not None
        ]
        _require(reports, "対象JSONが無い")
        for report in reports:
            _require(
                [file["path"] for file in report["files"]] == ["input.rb"], "対象path/順序が不一致"
            )
            _require(
                report["summary"]["target_file_count"] == 1
                and report["summary"]["inspected_file_count"] == 1,
                "対象未検査",
            )
            _require(
                report["summary"]["offense_count"]
                == sum(len(file["offenses"]) for file in report["files"]),
                "診断集計が不一致",
            )
    elif form["coverage"] != "single_equivalence":
        _require(reference["exit"] == 2 and reference["stderr_hex"], "否定対照が拒否されなかった")
    single = invoke(form["single"]) if "single" in form else None
    if single is not None:
        _require(
            single["report"] == reference["report"]
            and single["exit"] == reference["exit"]
            and single["stderr_hex"] == reference["stderr_hex"],
            "本家の単独/重複が非同値",
        )
    _require((directory / "input.rb").read_bytes() == SOURCE.encode(), "本家が非補正原本を変更")
    return {
        "id": form["id"] + "_" + mode,
        "mode": mode,
        "coverage": form["coverage"],
        "error_token": form.get("error_token"),
        "residual_issue": form.get("residual_issue"),
        "dotfile": form.get("dotfile"),
        "env_options": form.get("env_options"),
        "reference": reference,
        "single_reference": single,
        "outputs": outputs,
    }


def main():
    """候補のsource/outputを参照せず、有限の自作入力だけを本家へ渡す。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "launcher", "work", "out"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    forms = _forms()
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        cases = list(
            pool.map(
                lambda job: _record(job, args),
                [(i, form, stdin) for i, form in enumerate(forms) for stdin in (False, True)],
            )
        )
    fixture = {
        "reference_version": {
            "rubocop": "1.89.0",
            "parser": "3.3.12.0",
            "ruby": "4.0.7",
            "platform": "Windows",
        },
        "source": SOURCE,
        "yaml": YAML,
        "cases": cases,
    }
    Path(args.out).write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2), encoding="utf-8", newline="\n"
    )
    print(f"recorded {len(cases)} cases")


if __name__ == "__main__":
    main()
