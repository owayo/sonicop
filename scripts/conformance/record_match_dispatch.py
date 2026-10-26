"""保存済み入力だけを使い、本家 match dispatch の CLI 記録を再生成する。"""

import argparse
import concurrent.futures
import hashlib
import json
import os
import subprocess
from pathlib import Path


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _expand(part):
    return (part["prefix"] + part["padding"] * part["count"] + part["suffix"]).encode("utf-8")


def _compact(data, original):
    text = data.decode("utf-8")
    repeated = original["padding"] * original["count"]
    _require(text.count(repeated) == 1, "補正後のpaddingを一意に復元できない")
    prefix, suffix = text.split(repeated)
    return {
        "prefix": prefix,
        "padding": original["padding"],
        "count": original["count"],
        "suffix": suffix,
        "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": len(data),
    }


def _record_group(index_group, *, args, fixture):
    index, group = index_group
    directory = Path(args.work) / f"group-{index:02}"
    directory.mkdir(parents=True, exist_ok=True)
    (directory / ".rubocop.yml").write_bytes(group["yaml"].encode("utf-8"))
    records = []
    compact = isinstance(group["cases"][0]["source"], dict)
    for case in group["cases"]:
        source = _expand(case["source"]) if compact else case["source"].encode("utf-8")
        filename = case["filename"]
        _require(
            Path(filename).name == filename and filename.endswith(".rb"),
            "入力名が相対Ruby fileではない",
        )
        (directory / filename).write_bytes(source)
        record = {
            key: value
            for key, value in case.items()
            if key in ("name", "filename", "source", "unsafe")
        }
        if compact:
            record["unsafe"] = case["unsafe"]
        else:
            record["source_sha256"] = hashlib.sha256(source).hexdigest()
        records.append(record)
    command = [
        args.ruby,
        args.launcher,
        "--config",
        ".rubocop.yml",
        "--cache",
        "false",
        "--only",
        group.get("cop", "Style/RedundantParentheses"),
        "-f",
        "json",
    ]
    if group["mode"]:
        command.append(group["mode"])
    command.extend(case["filename"] for case in records)
    # fixture の設定名を保ったまま、起動元のオプションで本家の診断を変えない。
    # 固定 gem を置く GEM_HOME / GEM_PATH とプロセス全体の環境はそのまま保つ。
    environment = os.environ.copy()
    for name in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        environment.pop(name, None)
    done = subprocess.run(
        command,
        cwd=directory,
        capture_output=True,
        check=False,
        timeout=180,
        env=environment,
    )
    _require(done.returncode in (0, 1), (index, done.returncode, done.stderr))
    _require(not done.stderr, (index, done.stderr))
    report = json.loads(done.stdout)
    metadata = report.pop("metadata")
    _require(metadata["rubocop_version"] == fixture["reference"]["rubocop"], "RuboCop版不一致")
    _require(metadata["ruby_engine"] == "ruby", "本家Ruby VMの出力ではない")
    _require(metadata["ruby_version"] == fixture["reference"]["ruby"], "Ruby版不一致")
    _require("mingw" in metadata["ruby_platform"], "Windowsの本家記録ではない")
    _require(
        [item["path"] for item in report["files"]] == [case["filename"] for case in records],
        "対象path/順序不一致",
    )
    summary = report["summary"]
    _require(summary["target_file_count"] == len(records), "target数不一致")
    _require(summary["inspected_file_count"] == len(records), "inspected数不一致")
    offenses = [offense for item in report["files"] for offense in item["offenses"]]
    _require(summary["offense_count"] == len(offenses), "offense集計不一致")
    _require(
        all(isinstance(offense["corrected"], bool) for offense in offenses), "correctedはboolが必要"
    )
    _require(
        done.returncode == int(any(not offense["corrected"] for offense in offenses)),
        "終了値と未補正診断が不一致",
    )
    for record in records:
        data = (directory / record["filename"]).read_bytes()
        if compact:
            record["after"] = _compact(data, record["source"])
        else:
            record["after_hex"] = data.hex()
            record["after_lf"] = data.replace(b"\r\n", b"\n").decode("utf-8")
            record["after_sha256"] = hashlib.sha256(data).hexdigest()
    result = {
        key: value
        for key, value in group.items()
        if key in ("version", "engine", "ending", "cop", "mode", "yaml")
    }
    result.update({"exit": done.returncode, "report": report, "cases": records})
    return result


def main():
    """元fixtureの入力部を読み、再生成した本家の期待部を別ファイルへ保存する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("fixture", "out", "work", "ruby", "launcher"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--jobs", type=int, default=2, choices=(1, 2, 3))
    args = parser.parse_args()
    input_path = Path(args.fixture).resolve()
    output_path = Path(args.out).resolve()
    _require(input_path != output_path, "元の本家記録は上書きしない")
    fixture = json.loads(input_path.read_bytes())
    groups = fixture["groups"]
    _require(len(groups) in (18, 36), "設定group数不一致")
    expected_count = 234 if len(groups) == 18 else 720
    _require(sum(len(group["cases"]) for group in groups) == expected_count, "入力総数不一致")
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as executor:
        recorded = list(
            executor.map(
                lambda item: _record_group(item, args=args, fixture=fixture),
                enumerate(groups),
            )
        )
    output = {"reference": fixture["reference"], "groups": recorded}
    output_path.write_text(
        json.dumps(output, ensure_ascii=False, indent=2), encoding="utf-8", newline="\n"
    )
    print(f"recorded {expected_count} inputs in {len(recorded)} groups")


if __name__ == "__main__":
    main()
