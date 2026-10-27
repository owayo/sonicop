#!/usr/bin/env python3
"""本家のブロック付き splat 補正と、配列の値・副作用の変化を記録する。"""

import argparse
import datetime
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COP = "Lint/RedundantSplatExpansion"
BOOTSTRAP = (
    'gem "parser", "3.3.12.0"; gem "rubocop", "1.89.0"; '
    'load Gem.bin_path("rubocop", "rubocop", "1.89.0")'
)
CONFIG = (
    "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
    "  ParserEngine: parser_whitequark\n"
    f"{COP}:\n  Enabled: true\n"
)
SOURCES = {
    "explicit_before": ("divergence", "ary = [99, *Array.new(2) { 1 }]\np ary\n"),
    "explicit_after": ("divergence", "ary = [*Array.new(2) { 1 }, 99]\np ary\n"),
    "implicit_before": ("divergence", "ary = 99, *Array.new(2) { 1 }\np ary\n"),
    "sole_block": ("control", "ary = [*Array.new(2) { 1 }]\np ary\n"),
    "sole_plain": ("control", "ary = [*Array.new(2, 1)]\np ary\n"),
    "mixed_plain": ("control", "ary = [99, *Array.new(2, 1)]\np ary\n"),
    "literal_array": ("control", "ary = [99, *[1, 2]]\np ary\n"),
    "side_effects": (
        "effects",
        (
            "events = []\n"
            "ary = [(events << :before; 99), *Array.new(2) { events << :block; 1 }, "
            "(events << :after; 100)]\n"
            "p [ary, events]\n"
        ),
    ),
    "multiple_splats": (
        "effects",
        (
            "def tail(events)\n"
            "  events << :second\n"
            "  [2]\n"
            "end\n"
            "events = []\n"
            "ary = [*Array.new(1) { events << :first; 1 }, "
            "*tail(events)]\n"
            "p [ary, events]\n"
        ),
    ),
}


def _environment():
    # 本家CLI・実値の実行・候補の版照会を、同じ固定条件で起動する。
    environment = dict(os.environ)
    for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        environment.pop(key, None)
    return environment


def sha256(raw):
    """本文の証跡を改行変換せずバイト単位で固定する。

    Args:
        raw: 記録する生バイト列。

    Returns:
        SHA-256 の十六進表記。

    """
    return hashlib.sha256(raw).hexdigest()


def execute(ruby, path, env):
    """構文が通っても値や副作用が変わる補正を区別する。

    Args:
        ruby: 参照 MRI の実行ファイル。
        path: 原本または補正結果の独立したファイル。
        env: 版の上書きを除いた子プロセス環境。

    Returns:
        終了コードと stdout・stderr、その digest。

    """
    result = subprocess.run(
        [str(ruby), str(path)], env=env, capture_output=True, timeout=30, check=False
    )
    return {
        "exit": result.returncode,
        "stdout": result.stdout.decode("utf-8"),
        "stderr": result.stderr.decode("utf-8"),
        "stdout_sha256": sha256(result.stdout),
    }


def inspect(command, folder, paths, env):
    """途中終了の JSON を完全な参照記録へ混ぜない。

    Args:
        command: 版と補正モードを指定した CLI コマンド。
        folder: 親設定を拾わない一時ディレクトリ。
        paths: 明示した比較対象の相対パス。
        env: 子プロセス環境。

    Returns:
        完了したプロセス結果と report、ファイル別の診断。

    Raises:
        RuntimeError: CLI が検査・補正を完走できなかった場合。
        ValueError: 完了件数や対象パスが一致しない場合。

    """
    result = subprocess.run(
        command + paths,
        cwd=folder,
        env=env,
        capture_output=True,
        timeout=180,
        check=False,
    )
    if result.returncode not in (0, 1):
        raise RuntimeError(result.stderr.decode("utf-8", errors="replace"))
    report = json.loads(result.stdout)
    if any(
        report["summary"][key] != len(paths)
        for key in ("target_file_count", "inspected_file_count")
    ):
        raise ValueError("参照 CLI が全ファイルを完走しなかった")
    files = {file["path"]: file["offenses"] for file in report["files"]}
    if len(report["files"]) != len(paths) or set(files) != set(paths):
        raise ValueError("CLI の対象ファイル集合が変わった")
    return result, report, files


def main():
    """複製へ原本を戻し、各補正モードを同じバイトから測る。

    Raises:
        ValueError: 原本を実行できない、候補が値を変える、本家の破壊が再現しない場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, default=shutil.which("ruby"))
    parser.add_argument("--sonicop", type=Path, required=True)
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT / "tests/fixtures/splat_block_reference.json",
    )
    args = parser.parse_args()
    if args.ruby is None:
        parser.error("--ruby に参照 Ruby の実行ファイルを指定する")
    ruby, candidate = args.ruby.resolve(), args.sonicop.resolve()
    env = _environment()
    metadata_script = (
        'gem "parser", "3.3.12.0"; gem "rubocop", "1.89.0"; '
        'require "rubocop"; require "json"; '
        "puts JSON.generate({ruby: RUBY_VERSION, "
        'rubocop: Gem.loaded_specs.fetch("rubocop").version.to_s, '
        'parser: Gem.loaded_specs.fetch("parser").version.to_s, '
        'rubocop_ast: Gem.loaded_specs.fetch("rubocop-ast").version.to_s, '
        'prism: Gem.loaded_specs.fetch("prism").version.to_s})'
    )
    metadata = json.loads(
        subprocess.check_output([str(ruby), "-e", metadata_script], env=env)
    )
    record = {
        "issue": 87,
        "reference": metadata,
        "candidate_version": subprocess.check_output(
            [str(candidate), "--version"], env=env
        )
        .decode()
        .strip(),
        "candidate_binary_sha256": sha256(candidate.read_bytes()),
        "recorded_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "exit_scope": (
            "reference_exit / candidate_exit are whole 18-file invocation statuses; "
            "dedicated CLI regressions also check each protected input "
            "exits 1 individually."
        ),
        "config": CONFIG,
        "commands": {
            "reference": [
                "${RUBY}",
                "-e",
                BOOTSTRAP,
                "--",
                "--raise-cop-error",
                "-c",
                ".rubocop.yml",
                "--cache",
                "false",
                "--only",
                COP,
                "-f",
                "json",
                "${MODE}",
                "${PATHS}",
            ],
            "candidate": [
                "${SONICOP}",
                "-c",
                ".rubocop.yml",
                "--cache",
                "false",
                "--only",
                COP,
                "-f",
                "json",
                "${MODE}",
                "${PATHS}",
            ],
        },
        "runs": [],
        "cases": [],
        "controls": [],
        "effect_cases": [],
    }
    inspection_offenses = None
    for mode, flag in (("safe", "-a"), ("unsafe", "-A")):
        with tempfile.TemporaryDirectory(prefix="splat-block-reference-") as tmp:
            folder = Path(tmp)
            (folder / ".rubocop.yml").write_text(CONFIG, encoding="utf-8", newline="")
            rows, paths = [], []
            for name, (category, source) in SOURCES.items():
                for ending, newline in (("lf", "\n"), ("crlf", "\r\n")):
                    path = f"{name}-{ending}.rb"
                    original = source.replace("\n", newline).encode()
                    (folder / path).write_bytes(original)
                    paths.append(path)
                    rows.append(
                        {
                            "id": f"{name}-{mode}-{ending}",
                            "name": name,
                            "category": category,
                            "mode": mode,
                            "ending": ending,
                            "original_hex": original.hex(),
                            "original_sha256": sha256(original),
                            "original_runtime": execute(ruby, folder / path, env),
                        }
                    )
            common = [
                "-c",
                ".rubocop.yml",
                "--cache",
                "false",
                "--only",
                COP,
                "-f",
                "json",
                flag,
            ]
            if inspection_offenses is None:
                result, report, inspection_offenses = inspect(
                    [str(ruby), "-e", BOOTSTRAP, "--", "--raise-cop-error"]
                    + common[:-1],
                    folder,
                    paths,
                    env,
                )
                record["runs"].append(
                    {
                        "side": "reference",
                        "mode": "inspect",
                        "exit": result.returncode,
                        "stdout": result.stdout.decode("utf-8"),
                        "stderr": result.stderr.decode("utf-8")
                        .replace(str(folder), "${INPUT}")
                        .replace(folder.as_posix(), "${INPUT}"),
                        "summary": report["summary"],
                    }
                )
            for row, path in zip(rows, paths, strict=True):
                row["reference_inspection_offenses"] = inspection_offenses[path]
            for side, command in (
                (
                    "reference",
                    [str(ruby), "-e", BOOTSTRAP, "--", "--raise-cop-error"],
                ),
                ("candidate", [str(candidate)]),
            ):
                for row, path in zip(rows, paths, strict=True):
                    (folder / path).write_bytes(bytes.fromhex(row["original_hex"]))
                result, report, offenses = inspect(command + common, folder, paths, env)
                record["runs"].append(
                    {
                        "side": side,
                        "mode": mode,
                        "exit": result.returncode,
                        "stdout": result.stdout.decode("utf-8"),
                        "stderr": result.stderr.decode("utf-8")
                        .replace(str(folder), "${INPUT}")
                        .replace(folder.as_posix(), "${INPUT}"),
                        "summary": report["summary"],
                    }
                )
                for row, path in zip(rows, paths, strict=True):
                    written = (folder / path).read_bytes()
                    row[f"{side}_hex"] = written.hex()
                    row[f"{side}_sha256"] = sha256(written)
                    row[f"{side}_offenses"] = offenses[path]
                    row[f"{side}_runtime"] = execute(ruby, folder / path, env)
                    row[f"{side}_exit"] = result.returncode
            for row in rows:
                if row["original_runtime"]["exit"] != 0:
                    raise ValueError(f"原本を実行できなかった: {row['id']}")
                if row["original_runtime"] != row["candidate_runtime"]:
                    raise ValueError(f"候補が値・評価順・回数を変えた: {row['id']}")
                if row["category"] in ("divergence", "effects"):
                    if (
                        row["candidate_offenses"]
                        != row["reference_inspection_offenses"]
                    ):
                        raise ValueError(
                            f"候補の診断が本家の補正前と異なる: {row['id']}"
                        )
                    if row["candidate_hex"] != row["original_hex"]:
                        raise ValueError(f"保護対象の原本が変わった: {row['id']}")
                    if row["reference_runtime"] == row["original_runtime"]:
                        raise ValueError(f"本家の値破壊を再現しなかった: {row['id']}")
                key = {
                    "divergence": "cases",
                    "control": "controls",
                    "effects": "effect_cases",
                }[row["category"]]
                record[key].append(row)
    args.output.write_text(
        json.dumps(record, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="",
    )
    print(
        f"{len(record['cases'])} primary / "
        f"{len(record['effect_cases'])} effects / "
        f"{len(record['controls'])} controls"
    )


if __name__ == "__main__":
    main()
