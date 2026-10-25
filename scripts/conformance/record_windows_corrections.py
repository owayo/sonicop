"""Windows の実補正サイクルと、保護する DATA/NUL の原本値を記録する。"""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path

BASE = (
    "AllCops:\n  DisabledByDefault: true\n  NewCops: disable\n  TargetRubyVersion: 2.7\n"
    "Layout/SpaceAroundOperators:\n  Enabled: true\nLayout/EndOfLine:\n"
)
SOURCES = {
    "changed.rb": b"x=1\n",
    "clean.rb": b"x = 1\n",
    "crlf.rb": b"x=1\r\n",
    "mixed.rb": b"x=1\r\np x\n",
    "literal.rb": b'x="a\nb"\np x.bytes\n',
    "heredoc.rb": b"x=<<~END\n  a\n  b\nEND\np x.bytes\n",
    "fake_data.rb": b'x="__END__\na\nb"\np x.bytes\n',
    "data.rb": b"x=1\np DATA.binmode.read.bytes\n__END__\na\nb\n",
    "nul.rb": b"x=1\n\x00a\nb\n",
    "unicode.rb": "あ=1\n".encode(),
    "bom.rb": b"\xef\xbb\xbfx=1\n",
}


def oracle_child_env():
    """親の実行環境を保ち、Oracleへ検査条件を注入する4keyだけを子から除く。

    Returns:
        dict: GEM/PATH/SystemRoot等を保持した独立の子ENV。

    """
    env = os.environ.copy()
    for key in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        env.pop(key, None)
    return env


def execute(command, folder, args, expected_paths, stdin=None):
    """プラットフォーム情報以外は本家の全 JSON を保持する。

    Returns:
        dict: 本家の終了値と全診断。
    """
    result = subprocess.run(
        command + ["--cache", "false", "-f", "json"] + args,
        cwd=folder,
        input=stdin,
        capture_output=True,
        timeout=180,
        env=oracle_child_env(),
    )
    if result.returncode not in (0, 1):
        raise ValueError(f"unexpected reference exit: {result.returncode}")
    if result.stderr:
        raise ValueError(f"unexpected reference stderr: {result.stderr!r}")
    expected_paths = list(expected_paths)
    if not expected_paths or len(set(expected_paths)) != len(expected_paths):
        raise ValueError("expected paths must be nonempty and unique")
    report = json.loads(result.stdout)
    if not isinstance(report, dict):
        raise ValueError("reference report must be an object")
    files = report.get("files")
    summary = report.get("summary")
    if not isinstance(files, list) or not isinstance(summary, dict):
        raise ValueError("reference report needs files and summary")
    paths = [file.get("path") for file in files if isinstance(file, dict)]
    if (
        len(paths) != len(files)
        or any(not isinstance(path, str) for path in paths)
        or sorted(paths) != sorted(expected_paths)
    ):
        raise ValueError(f"reference paths are incomplete or duplicated: {paths!r}")
    counts = (summary.get("target_file_count"), summary.get("inspected_file_count"))
    if any(type(count) is not int or count != len(expected_paths) for count in counts):
        raise ValueError(f"reference inspection did not finish: {counts!r}")
    offenses = [file.get("offenses") for file in files]
    if any(not isinstance(items, list) for items in offenses):
        raise ValueError("reference files need offense arrays")
    offense_count = summary.get("offense_count")
    if type(offense_count) is not int or offense_count != sum(map(len, offenses)):
        raise ValueError("reference offense summary disagrees with files")
    report.pop("metadata", None)
    return {"exit": result.returncode, "report": report}


def runtime(ruby, folder, data):
    """Windows の通常 DATA.read が隠す変化を binmode の実行値で比較する。

    Returns:
        dict: MRI の終了値と標準出力・標準エラーの hex。
    """
    (folder / "value.rb").write_bytes(data)
    result = subprocess.run(
        [ruby, "value.rb"], cwd=folder, capture_output=True, env=oracle_child_env()
    )
    return {"exit": result.returncode, "stdout": result.stdout.hex(), "stderr": result.stderr.hex()}


def tail_cases():
    """既知の原本境界から安全な物理 bytes を構成し、候補出力への依存を避ける。

    Returns:
        dict: 原本と保存予定 bytes を名前へ対応付けた入力集合。
    """
    prefix = b"x=1\np DATA.binmode.read.bytes\n__END__\n"
    cases = {
        "lf.rb": (prefix + b"a\nb\n", b"__END__\n"),
        "crlf.rb": ((prefix + b"a\nb\n").replace(b"\n", b"\r\n"), b"__END__\r\n"),
        "mixed.rb": (prefix + b"a\r\nb\nc\r", b"__END__\n"),
        "barecr.rb": (prefix + b"a\rb\n", b"__END__\n"),
        "empty.rb": (prefix, b"__END__\n"),
        "unterminated.rb": (prefix + b"a\nb", b"__END__\n"),
        "nul.rb": (b"x=1\np x\n\0ignored\nnext\n", b"\0"),
        "nul_literal.rb": (b'x="a\0\nb"\np x.bytes\n', None),
        "nul_comment.rb": (b"x=1 # a\0b\np x\n", None),
        "marker_comment.rb": (b"x=1\n# __END__\np x\n", None),
        "marker_heredoc.rb": (b"x=<<~TEXT\n__END__\na\nTEXT\np x.bytes\n", None),
    }
    rows = {}
    for name, (source, marker) in cases.items():
        corrected = source.replace(b"x=", b"x = ")
        offset = (
            len(corrected)
            if marker is None
            else corrected.index(marker) + (0 if marker == b"\0" else len(marker))
        )
        code = corrected[:offset]
        if b"\r\n" not in source:
            code = code.replace(b"\n", b"\r\n")
        rows[name] = {"source": source.hex(), "after": (code + corrected[offset:]).hex()}
    return rows


def removal_cases(command, config):
    """複数 pass で空行が露出する二種類の削除を、実 CLI で固定する。

    Returns:
        list: 原本、設定、初回と再実行の全 JSON と物理 bytes。

    """
    groups = []
    for cop, source in [
        ("Lint/UselessAccessModifier", b"class A\n  foo\n\n  private\n\n  bar\nend\n"),
        (
            "Lint/UselessMethodDefinition",
            b"class A\n  foo\n\n  def binding\n    super\n  end\n\n  bar\nend\n",
        ),
    ]:
        for ending in ["lf", "crlf"]:
            source_bytes = source if ending == "lf" else source.replace(b"\n", b"\r\n")
            for mode in ["-a", "-A"]:
                with tempfile.TemporaryDirectory() as directory:
                    folder = Path(directory)
                    (folder / ".rubocop.yml").write_bytes(config.encode())
                    (folder / "input.rb").write_bytes(source_bytes)
                    only = cop + ",Layout/EmptyLines,Layout/TrailingWhitespace"
                    runs = []
                    for flag in [mode, "-A"]:
                        run = execute(
                            command, folder, [flag, "--only", only, "input.rb"], ["input.rb"]
                        )
                        run["after"] = (folder / "input.rb").read_bytes().hex()
                        runs.append(run)
                    groups.append(
                        {
                            "source": source_bytes.hex(),
                            "cop": cop,
                            "ending": ending,
                            "mode": mode,
                            "runs": runs,
                        }
                    )
    return groups


def main():
    """本家の初回と、保存予定の安全な入力に対する再実行を別々に記録する。"""
    parser = argparse.ArgumentParser()
    parser.add_argument("--ruby", required=True)
    parser.add_argument("--reference", required=True)
    parser.add_argument("--output", type=Path, required=True)
    options = parser.parse_args()
    command = [options.ruby, options.reference]
    result = {
        "rubocop": "1.89.0",
        "parser": "3.3.12.0",
        "platform": "windows",
        "sources": {name: data.hex() for name, data in SOURCES.items()},
        "groups": [],
    }
    result["tail_cases"] = tail_cases()
    result["tail_groups"] = []
    for target in ["2.7", "3.4"]:
        config = (BASE + "  Enabled: false\n").replace(
            "TargetRubyVersion: 2.7", "TargetRubyVersion: " + target
        )
        for mode in ["-a", "-A"]:
            with tempfile.TemporaryDirectory() as directory:
                folder = Path(directory)
                (folder / ".rubocop.yml").write_bytes(config.encode())
                for name, case in result["tail_cases"].items():
                    source = bytes.fromhex(case["source"])
                    (folder / name).write_bytes(source)
                    case["before_value"] = runtime(options.ruby, folder, source)
                first = execute(
                    command, folder, [mode] + list(result["tail_cases"]), result["tail_cases"]
                )
                first["raw_after"] = {
                    name: (folder / name).read_bytes().hex() for name in result["tail_cases"]
                }
                first["raw_values"] = {
                    name: runtime(options.ruby, folder, (folder / name).read_bytes())
                    for name in result["tail_cases"]
                }
                for name, case in result["tail_cases"].items():
                    safe = bytes.fromhex(case["after"])
                    (folder / name).write_bytes(safe)
                    case["safe_value"] = runtime(options.ruby, folder, safe)
                rerun = execute(
                    command, folder, ["-A"] + list(result["tail_cases"]), result["tail_cases"]
                )
                result["tail_groups"].append(
                    {"target": target, "mode": mode, "config": config, "runs": [first, rerun]}
                )
                print("tail", target, mode, flush=True)
    for style in ["disabled", "native", "lf", "crlf"]:
        config = BASE + (
            "  Enabled: false\n"
            if style == "disabled"
            else "  Enabled: true\n  EnforcedStyle: " + style + "\n"
        )
        for mode in ["-a", "-A"]:
            with tempfile.TemporaryDirectory() as directory:
                folder = Path(directory)
                (folder / ".rubocop.yml").write_bytes(config.encode())
                for name, data in SOURCES.items():
                    (folder / name).write_bytes(data)
                runs = []
                for flag in [mode, "-A"]:
                    run = execute(command, folder, [flag] + list(SOURCES), SOURCES)
                    run["raw_after"] = {
                        name: (folder / name).read_bytes().hex() for name in SOURCES
                    }
                    expected = dict(run["raw_after"])
                    # 確定済み mixed 保存と、DATA/NUL tail 保存の独立した byte 期待。
                    expected["mixed.rb"] = SOURCES["mixed.rb"].replace(b"x=1", b"x = 1").hex()
                    for name, marker in [("data.rb", b"__END__\n"), ("nul.rb", b"\0")]:
                        original = SOURCES[name]
                        offset = original.index(marker) + (len(marker) if name == "data.rb" else 0)
                        code = original[:offset].replace(b"x=1", b"x = 1").replace(b"\n", b"\r\n")
                        expected[name] = (code + original[offset:]).hex()
                    run["after"] = expected
                    runs.append(run)
                    # 再実行の診断は、本家の破壊的な tail ではなく保存する物理入力で記録する。
                    for name, hexed in expected.items():
                        (folder / name).write_bytes(bytes.fromhex(hexed))
                result["groups"].append(
                    {"style": style, "mode": mode, "config": config, "runs": runs}
                )
                print(style, mode, flush=True)
    with tempfile.TemporaryDirectory() as directory:
        folder = Path(directory)
        config = BASE + "  Enabled: true\n  EnforcedStyle: native\n"
        (folder / ".rubocop.yml").write_bytes(config.encode())
        result["stdin_config"] = config
        result["stdin"] = []
        for name in ["changed.rb", "clean.rb", "crlf.rb"]:
            run = execute(command, folder, ["-a", "--stdin", name], [name], SOURCES[name])
            run["name"] = name
            result["stdin"].append(run)
    config = "AllCops:\n  DisabledByDefault: true\n  NewCops: disable\n  TargetRubyVersion: 2.7\nLayout/EndOfLine:\n  Enabled: false\n"
    result["empty_config"] = config
    result["removals"] = removal_cases(command, config)
    result["continuations"] = []
    for cop, source in [
        ("Layout/LineContinuationSpacing", b"x=1\nvalue = 'a'\\\n  'b'\n"),
        ("Layout/LineContinuationLeadingSpace", b"x=1\nvalue = 'a' \\\n  ' b'\n"),
    ]:
        for ending in ["lf", "crlf"]:
            physical = source if ending == "lf" else source.replace(b"\n", b"\r\n")
            for mode in ["inspect", "-A"]:
                with tempfile.TemporaryDirectory() as directory:
                    folder = Path(directory)
                    (folder / ".rubocop.yml").write_bytes(config.encode())
                    (folder / "input.rb").write_bytes(physical)
                    args = (
                        ["--only", cop + ",Layout/SpaceAroundOperators"]
                        + ([] if mode == "inspect" else [mode])
                        + ["input.rb"]
                    )
                    first = execute(command, folder, args, ["input.rb"])
                    first["after"] = (folder / "input.rb").read_bytes().hex()
                    result["continuations"].append(
                        {
                            "cop": cop,
                            "ending": ending,
                            "mode": mode,
                            "source": physical.hex(),
                            "run": first,
                        }
                    )
    composite = b"class A\n  foo\n\n  private\n\n  bar\nend\n__END__\na\n\n\nb\n"
    result["composite"] = {"source": composite.hex(), "config": config}
    with tempfile.TemporaryDirectory() as directory:
        folder = Path(directory)
        (folder / ".rubocop.yml").write_bytes(config.encode())
        (folder / "input.rb").write_bytes(composite)
        args = [
            "-A",
            "--only",
            "Lint/UselessAccessModifier,Layout/EmptyLines,Layout/TrailingWhitespace",
        ]
        reference = execute(command, folder, args + ["input.rb"], ["input.rb"])
        reference["after"] = (folder / "input.rb").read_bytes().hex()
        expected = execute(command, folder, args + ["--stdin", "input.rb"], ["input.rb"], composite)
        safe = b"class A\r\n  foo\r\n\r\n  bar\r\nend\r\n__END__\r\na\n\n\nb\n"
        expected["after"] = safe.hex()
        (folder / "input.rb").write_bytes(safe)
        rerun = execute(command, folder, args + ["input.rb"], ["input.rb"])
        rerun["after"] = (folder / "input.rb").read_bytes().hex()
        result["composite"].update({"reference": reference, "runs": [expected, rerun]})
    options.output.write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n"
    )


if __name__ == "__main__":
    main()
