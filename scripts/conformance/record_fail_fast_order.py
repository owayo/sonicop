"""固定 RuboCop の実 CLI と Ruby sort_by から fail-fast の順序を採取する。"""

import argparse
import hashlib
import json
import os
import pathlib
import random
import subprocess
import tempfile

CONFIG = (
    "AllCops:\n  NewCops: disable\n  UseProjectIndex: false\n"
    "  TargetRubyVersion: 3.3\n  ParserEngine: parser_prism\n"
)
RUNTIME = {
    "ruby": "4.0.7",
    "revision": "229531a6cfbf07e3caef30dbac24a2a3f3fed482",
    "rubocop": "1.89.0",
    "parser": "3.3.12.0",
    "prism": "1.8.1",
    "rubocop_ast": "1.50.0",
    "platform": "x64-mingw-ucrt",
    "long_bits": 32,
}
LAUNCHER = """gem 'parser', '3.3.12.0'
gem 'prism', '1.8.1'
gem 'rubocop-ast', '1.50.0'
gem 'rubocop', '1.89.0'
require 'rubocop'
require 'json'
if ARGV == ['--record-runtime']
  require 'rbconfig/sizeof'
  puts JSON.generate({ruby: RUBY_VERSION, revision: RUBY_REVISION, platform: RUBY_PLATFORM,
    long_bits: RbConfig::SIZEOF.fetch('long') * 8, rubocop: RuboCop::Version::STRING,
    parser: Gem.loaded_specs.fetch('parser').version.to_s,
    prism: Gem.loaded_specs.fetch('prism').version.to_s,
    rubocop_ast: Gem.loaded_specs.fetch('rubocop-ast').version.to_s})
elsif ARGV == ['--record-sort']
  require 'objspace'
  input = JSON.parse(STDIN.binmode.read)
  raise 'empty sort evidence' unless input.length == 720
  puts JSON.generate(input.map { |keys|
    uniform = keys.all? { |key| !JSON.parse(ObjectSpace.dump(key)).is_a?(Hash) }
    {keys: keys, indices: keys.each_index.sort_by { |i| keys[i] }, uniform: uniform}
  })
else
  exit RuboCop::CLI.new.run(ARGV)
end
"""


def _scenario(
    name,
    files,
    times,
    targets,
    *,
    stdin=False,
    cached=False,
    repeat=0,
    group=None,
    windows_only=False,
    fail_fast=True,
    config=CONFIG,
):
    return {
        "name": name,
        "group": group or name,
        "repeat": repeat,
        "windows_only": windows_only,
        "files": {path: body.hex() for path, body in files.items()},
        "mtime_ns": dict(zip(files, times, strict=True)),
        "targets": targets,
        "stdin_hex": "783d310a" if stdin else None,
        "cached": cached,
        "fail_fast": fail_fast,
        "config": config,
    }


def scenarios():
    """入力だけを構成し、期待順は実 Ruby / CLI から取得する。

    Returns:
        list: 名前・原本・mtime・引数と実行条件。

    """
    second = 1_000_000_000
    pair = {"a.rb": b"x=1\n", "b.rb": b"x=1\n"}
    prefix = dict.fromkeys(
        ["z.rb", "b.rb", "a/zz.rb", "a.x/zz.rb", "a-/zz.rb", "a/z/x.rb", "a/y.rb", "a0/xx.rb"],
        b"x=1\n",
    )
    groups = dict.fromkeys(["d1/a.rb", "d1/b.rb", "d2/a.rb", "d2/b.rb", "marker.rb"], b"x=1\n")
    group_times = [value * second for value in [100, 101, 200, 201, 300]]
    finite = [
        _scenario("zero-subsecond", pair, [100_000_000, 900_000_000], ["."]),
        _scenario(
            "small-subsecond-reverse",
            pair,
            [100 * second + 900_000_000, 100 * second + 100_000_000],
            ["."],
        ),
        _scenario(
            "small-distinct", pair, [100 * second + 900_000_000, 101 * second + 100_000_000], ["."]
        ),
        _scenario(
            "modern-subsecond-reverse",
            pair,
            [1700000000 * second + 900_000_000, 1700000000 * second + 100_000_000],
            ["."],
            windows_only=True,
        ),
        _scenario("fixnum-min-key-tie", pair, [(2**30) * second] * 2, ["."]),
        _scenario(
            "bignum-negative-key-tie", pair, [(2**30 + 1) * second] * 2, ["."], windows_only=True
        ),
        _scenario(
            "negative-fraction-tie", pair, [-900_000_000, -100_000_000], ["."], windows_only=True
        ),
        _scenario("cross-epoch", pair, [-100_000_000, 100_000_000], ["."], windows_only=True),
        _scenario("prefix-zero", prefix, [0] * len(prefix), ["."]),
        _scenario(
            "prefix-modern", prefix, [1700000000 * second] * len(prefix), ["."], windows_only=True
        ),
        _scenario("explicit-groups", groups, group_times, ["d1", "marker.rb", "d2"]),
        _scenario("reversed-groups", groups, group_times, ["d2", "marker.rb", "d1"]),
        _scenario("dedupe-groups", groups, group_times, ["d1", "d1/a.rb", "d2", "d1", "d2/a.rb"]),
        _scenario("stdin-zero", pair | {"c.txt": b"x=1\n"}, [0, 0, 0], ["."], stdin=True),
        _scenario(
            "stdin-modern",
            pair | {"c.txt": b"x=1\n"},
            [1700000000 * second] * 3,
            ["."],
            stdin=True,
            windows_only=True,
        ),
        _scenario("default-lexical", prefix, [0] * len(prefix), ["."], fail_fast=False),
        _scenario("default-explicit-reverse", pair, [0, second], ["b.rb", "a.rb"], fail_fast=False),
        _scenario("implicit-cwd", pair, [100 * second, 101 * second], []),
    ]
    # cache の無効・cold・warm は同じ元54実行と同じ条件。warmでは再書込しない。
    for profile, mtimes in {
        "same_second": [1700000000 * second + 100_000_000, 1700000000 * second + 900_000_000],
        "equal": [1700000000 * second + 100_000_000] * 2,
        "different_second": [1700000000 * second + 100_000_000, 1700000001 * second + 100_000_000],
    }.items():
        for syntax, targets in {
            "explicit": ["a.rb", "b.rb"],
            "reverse": ["b.rb", "a.rb"],
            "directory": ["."],
        }.items():
            for cached in (False, True):
                group = f"cache-{profile}-{syntax}-{str(cached).lower()}"
                for repeat in range(2 if cached else 1):
                    finite.append(
                        _scenario(
                            f"{group}-{repeat}",
                            {"a.rb": b"x=1\n", "b.rb": b"y=2\n"},
                            mtimes,
                            targets,
                            cached=cached,
                            repeat=repeat,
                            group=group,
                            windows_only=True,
                        )
                    )
    return finite


def _normalize_transport(text):
    return text.replace("\r\n", "\n")


def _expected_paths(case):
    result = set()
    for target in case["targets"] or ["."]:
        if target == ".":
            result.update(case["files"])
        elif target in case["files"]:
            result.add(target)
        else:
            result.update(path for path in case["files"] if path.startswith(target + "/"))
    if case["stdin_hex"] is None:
        result = {path for path in result if path.endswith(".rb")}
    return result


def validate_listing(exit_code, stdout, stderr, case):
    """空・欠落・余分・重複の path 記録を拒否する。

    Returns:
        list: 実CLIが返した順序付きpath。

    Raises:
        ValueError: 完了条件、path集合またはstdin echoが不正。

    """
    if exit_code != 0 or stderr:
        raise ValueError("list CLI did not complete")
    text = _normalize_transport(stdout.decode("utf-8"))
    paths = text.splitlines()
    if not paths or len(paths) != len(set(paths)) or set(paths) != _expected_paths(case):
        raise ValueError("list target set is incomplete or duplicated")
    return paths


def validate_report(exit_code, stdout, stderr, case, paths):
    """fail-fastの正の検査完了と順序・全summary整合を検査する。

    Returns:
        tuple: 生metadata付きreportとJSON後の本文。

    Raises:
        ValueError: exit・完了件数・集合・順序・summary・echoが不正。

    """
    if exit_code not in (0, 1) or stderr:
        raise ValueError("inspection CLI did not complete")
    text = stdout.decode("utf-8")
    report, offset = json.JSONDecoder().raw_decode(text)
    after = text[offset:]
    # 補正しない stdin と JSON Formatter は、JSON の後に改行も本文も出さない。
    if after:
        raise ValueError("unexpected trailing output")
    files = report.get("files", [])
    actual = [row.get("path") for row in files]
    expected = paths[:1] if case["fail_fast"] else paths
    if not actual or len(actual) != len(set(actual)) or actual != expected:
        raise ValueError("inspection target order/set is incomplete")
    total = sum(len(row.get("offenses", [])) for row in files)
    if total != len(files) or report.get("summary") != {
        "offense_count": total,
        "target_file_count": len(paths),
        "inspected_file_count": len(files),
    }:
        raise ValueError("inspection summary is inconsistent")
    if exit_code != 1 or report.get("metadata", {}).get("rubocop_version") != "1.89.0":
        raise ValueError("wrong CLI status or oracle version")
    return report, after


def _run(command, cwd, stdin=None):
    # gem探索やWindowsの実行環境は保ち、検査条件を注入する4keyだけ子から除く。
    env = os.environ.copy()
    for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        env.pop(key, None)
    return subprocess.run(command, cwd=cwd, input=stdin, capture_output=True, check=False, env=env)


def _stamp(project, files):
    return {
        name: {
            "mtime_ns": (project / name).stat().st_mtime_ns,
            "size": (project / name).stat().st_size,
            "sha256": hashlib.sha256((project / name).read_bytes()).hexdigest(),
        }
        for name in files
    }


def _cache(root):
    return {
        path.relative_to(root).as_posix(): {
            "mtime_ns": path.stat().st_mtime_ns,
            "size": path.stat().st_size,
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def _arguments(case):
    args = [
        "--config",
        "../config.yml",
        "--cache",
        str(case["cached"]).lower(),
        "--only",
        "Layout/SpaceAroundOperators",
    ]
    if case["fail_fast"]:
        args.append("--fail-fast")
    if case["cached"]:
        args.extend(["--cache-root", "../cache"])
    if case["stdin_hex"] is not None:
        args.extend(["--stdin", case["targets"][0]])
    else:
        args.extend(case["targets"])
    return args


def _write(path, document):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8", newline="\n")


def _runtime(ruby, launcher, root):
    result = _run([ruby, str(launcher), "--record-runtime"], root)
    if result.returncode != 0 or result.stderr:
        raise ValueError("runtime probe failed")
    found = json.loads(result.stdout)
    if any(found.get(key) != value for key, value in RUNTIME.items()):
        raise ValueError(f"oracle runtime is not fixed: {found}")
    if found.get("long_bits") not in (32, 64):
        raise ValueError("unrecorded Ruby C long width")
    return found


def _sort_records(ruby, launcher, root, runtime):
    rng = random.Random(120)
    cases = []
    for size in [0, 1, 2, 5, 8, 9, 15, 16, 17, 18, 31, 32, 63, 64, 257]:
        for _ in range(24):
            cases.append([rng.choice([-2, -1, 0, 1, 2]) for _ in range(size)])
            cases.append(
                [rng.choice([-1700000000, -1700000001, -1, 0, 1, 1700000000]) for _ in range(size)]
            )
    raw_input = json.dumps(cases).encode()
    result = _run([ruby, str(launcher), "--record-sort"], root, raw_input)
    if result.returncode != 0 or result.stderr:
        raise ValueError("sort oracle failed")
    rows = json.loads(result.stdout)
    if len(rows) != 720:
        raise ValueError("sort oracle is incomplete")
    for keys, row in zip(cases, rows, strict=True):
        indices = row.get("indices", [])
        if (
            row.get("keys") != keys
            or sorted(indices) != list(range(len(keys)))
            or not isinstance(row.get("uniform"), bool)
        ):
            raise ValueError("sort oracle keys/permutation is incomplete")
        if [keys[index] for index in indices] != sorted(keys):
            raise ValueError("sort oracle numeric order is inconsistent")
    return {
        "runtime": runtime,
        "input_sha256": hashlib.sha256(raw_input).hexdigest(),
        "stdout_hex": result.stdout.hex(),
        "stderr_hex": result.stderr.hex(),
        "cases": rows,
    }


def collect(ruby, root, *, names=None, collect_sorts=True):
    """独立入力から本家だけを実行し、全完了ガードを満たした記録を返す。

    Returns:
        tuple: CLI fixture と720組のsort fixture。

    Raises:
        ValueError: 版・実行集合・完了・原本の保存条件が満たされない。

    """
    root.mkdir(parents=True, exist_ok=True)
    launcher = root / "reference.rb"
    launcher.write_text(LAUNCHER, encoding="utf-8", newline="\n")
    runtime = _runtime(ruby, launcher, root)
    records = []
    groups = {}
    for case in scenarios():
        if names and case["name"] not in names:
            continue
        if case["repeat"] and case["group"] not in groups:
            raise ValueError("warm case requires its cold case")
        base = root / case["group"]
        project = base / "project"
        if case["group"] not in groups:
            project.mkdir(parents=True, exist_ok=False)
            (base / "config.yml").write_text(case["config"], encoding="utf-8", newline="\n")
            for name, hex_value in case["files"].items():
                path = project / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(bytes.fromhex(hex_value))
                ns = case["mtime_ns"][name]
                os.utime(path, ns=(ns, ns))
            groups[case["group"]] = True
        before = _stamp(project, case["files"])
        if any(before[name]["mtime_ns"] != ns for name, ns in case["mtime_ns"].items()):
            raise ValueError("filesystem did not preserve the requested epoch")
        args = _arguments(case)
        stdin = bytes.fromhex(case["stdin_hex"]) if case["stdin_hex"] is not None else None
        cache_before = _cache(base / "cache")
        listing = _run([ruby, str(launcher), *args, "-L"], project, stdin)
        paths = validate_listing(listing.returncode, listing.stdout, listing.stderr, case)
        result = _run([ruby, str(launcher), *args, "--format", "json"], project, stdin)
        report, after_json = validate_report(
            result.returncode, result.stdout, result.stderr, case, paths
        )
        after = _stamp(project, case["files"])
        if before != after or (base / "config.yml").read_bytes() != case["config"].encode():
            raise ValueError("oracle changed input or metadata")
        report.pop("metadata")
        records.append(
            case
            | {
                "exit": result.returncode,
                "stdout_hex": result.stdout.hex(),
                "stderr_hex": result.stderr.hex(),
                "report": report,
                "after_json": after_json,
                "listing": listing.stdout.decode(),
                "paths": paths,
                "input_before": before,
                "input_after": after,
                "cache_before": cache_before,
                "cache_after": _cache(base / "cache"),
            }
        )
        print(case["name"], paths[0], flush=True)
    if not records or (names and {row["name"] for row in records} != set(names)):
        raise ValueError("requested case set is incomplete")
    sorts = _sort_records(ruby, launcher, root, runtime) if collect_sorts else None
    return {"runtime": runtime, "config": CONFIG, "cases": records}, sorts


def main():
    """依存する scratch や候補実行ファイルを要求せず公開記録を再生成する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument(
        "--output", type=pathlib.Path, default=pathlib.Path("tests/fixtures/fail_fast_order.json")
    )
    parser.add_argument(
        "--sort-output",
        type=pathlib.Path,
        default=pathlib.Path("tests/fixtures/fail_fast_sort.json"),
    )
    parser.add_argument("--names", nargs="+")
    parser.add_argument("--no-sort", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sonicop-fail-fast-oracle-") as temporary:
        document, sorts = collect(
            args.ruby, pathlib.Path(temporary), names=args.names, collect_sorts=not args.no_sort
        )
    _write(args.output.resolve(), document)
    if sorts is not None:
        _write(args.sort_output.resolve(), sorts)


if __name__ == "__main__":
    main()
