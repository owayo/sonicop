#!/usr/bin/env python3
"""RuboCop 1.89.0 の cache と注釈の履歴を、入力・cache の実ファイルとともに記録する。"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from contextlib import ExitStack
from pathlib import Path

CONFIG = (
    "AllCops:\n  NewCops: disable\n  UseProjectIndex: false\n"
    "  TargetRubyVersion: 3.3\n  ParserEngine: parser_prism\n"
    "  DisplayCopNames: true\n  DefaultFormatter: json\n"
    "  StyleGuideBaseURL: https://example.test/style\n"
    "Layout/SpaceAroundOperators:\n  Details: details from fixture\n"
    "  StyleGuide: '#operators'\n  Reference: https://example.test/reference\n"
)
OUTPUT_PATHS = [
    None,
    "",
    "---",
    "unused-1.txt",
    "unused-2.txt",
    "unused_日本.txt",
    "unused_別名.txt",
    "fold_K.txt",
    "fold_K.txt",
    "fold_k.txt",
    "fold_ſ.txt",
    "fold_s.txt",
    "fold_日本.txt",
    "out\rname.txt",
    "out\nname.txt",
    "out\vname.txt",
    "out\fname.txt",
    "out\\name.txt",
    'out"name.txt',
]

INSPECT_VALUES = [chr(cp) for cp in range(0xA0)] + [
    "#{value}",
    "#@value",
    "#$value",
    "#plain",
    *[
        chr(cp)
        for cp in (
            0xAD,
            0x378,
            0x200B,
            0x200C,
            0x200D,
            0x2028,
            0x2029,
            0x2060,
            0x3000,
            0xE000,
            0xF8FF,
            0xFFFE,
            0x10000,
            0x1F600,
            0x1FFFF,
            0xE0001,
            0xF0000,
            0x10FFFF,
        )
    ],
]


def _ruby_environment():
    # 参照を変える4変数だけ子の copy から除き、親・GEM/PATH/OS環境は保持する。
    environment = dict(os.environ)
    for name in ("RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"):
        environment.pop(name, None)
    return environment


# 受理するオプションが増えたとき、投影範囲の判断も回帰で必須にする。
CLI_OPTION_POLICY = {
    "relevant_inspection": (
        "safe only except only_guide_cops disable_pending_cops "
        "enable_pending_cops enable_all_cops disable_all_cops "
        "ignore_disable_comments force_exclusion only_recognized_file_types "
        "ignore_parent_exclusion ignore_unrecognized_cops force_default_config "
        "editor_mode raise_cop_error cache_root config display_cop_names "
        "no_display_cop_names extra_details display_style_guide stderr "
        "display_only_failed display_only_fail_level_offenses "
        "display_only_correctable display_only_safe_correctable"
    ),
    "non_changing": (
        "fail_fast parallel no_parallel fail_level cache formats color no_color "
        "debug display_time"
    ),
    "only_array_mutation": ("lint fix_layout"),
    "legacy_output_path": ("out"),
    "native_noop_or_limited": (
        "server no_server restart_server start_server stop_server server_status "
        "no_detach plugin requires"
    ),
    "early_exit_or_listing": (
        "lsp mcp list_target_files list_enabled_cops_for show_cops "
        "show_docs_url init version verbose_version"
    ),
    "cache_disabled": (
        "stdin profile memory autocorrect autocorrect_all "
        "deprecated_auto_correct deprecated_safe_auto_correct "
        "deprecated_auto_correct_all disable_uncorrectable"
    ),
    "configuration_generation": (
        "auto_gen_config regenerate_todo exclude_limit no_exclude_limit "
        "offense_counts no_offense_counts auto_gen_only_exclude "
        "no_auto_gen_only_exclude auto_gen_timestamp no_auto_gen_timestamp "
        "auto_gen_enforced_style no_auto_gen_enforced_style"
    ),
    "native_selection_extension": ("display_suppressed"),
    "positional_paths": ("paths"),
}


def _option_cases():
    # 実装済みの通常検査オプションを列挙し、Hash の順序と明示値も本家から採る。
    boolean = [
        "safe",
        "only-guide-cops",
        "disable-pending-cops",
        "enable-pending-cops",
        "disable-all-cops",
        "enable-all-cops",
        "ignore-disable-comments",
        "force-exclusion",
        "only-recognized-file-types",
        "ignore-parent-exclusion",
        "ignore-unrecognized-cops",
        "force-default-config",
        "editor-mode",
        "raise-cop-error",
        "stderr",
        "display-only-fail-level-offenses",
        "display-only-correctable",
        "display-only-safe-correctable",
        "display-cop-names",
        "no-display-cop-names",
        "extra-details",
        "display-style-guide",
        "color",
        "no-color",
        "debug",
        "display-time",
        "fail-fast",
        "parallel",
        "no-parallel",
        "lint",
        "fix-layout",
        "server",
        "no-server",
        "restart-server",
        "start-server",
        "stop-server",
        "server-status",
        "no-detach",
    ]
    return [
        [],
        *[["--" + name] for name in boolean],
        ["--only", "Layout/SpaceAroundOperators"],
        ["--only=Layout/SpaceAroundOperators"],
        ["--only", "Lint/Syntax,Layout/SpaceAroundOperators"],
        ["--only", ""],
        ["--only", "Layout/SpaceAroundOperators,Layout/TrailingWhitespace,"],
        ["--except", "Layout/SpaceAroundOperators,Layout/TrailingWhitespace"],
        ["--lint", "--only", "Layout/SpaceAroundOperators"],
        ["--only", "Layout/SpaceAroundOperators", "--lint"],
        ["--fix-layout", "--only", "Layout/SpaceAroundOperators"],
        ["--only", "Layout/SpaceAroundOperators", "--fix-layout"],
        ["--cache", "true"],
        ["--cache-root", "cache-1"],
        ["--cache-root=cache-2"],
        ["--config", "config.yml"],
        ["--config=config-日本.yml"],
        ["--fail-level", "warning"],
        ["--format", "json"],
        ["-ES"],
        ["-SE"],
        ["-D", "-E", "--no-display-cop-names"],
        ["-E", "--no-display-cop-names", "-D"],
        ["--out", "before.txt", "--format", "json", "--out", "after.txt"],
        ["--format", "json", "--out", "after.txt"],
        ["--out", "a.txt", "--out", "b.txt", "--format", "json"],
        ["--safe", "--stderr"],
        ["--stderr", "--safe"],
        ["--format", "junit", "--display-only-failed"],
        ["--", "--safe"],
    ]


def _step(
    formats=("json",),
    flags=(),
    cache=True,
    paths=("case.rb",),
    action=None,
    leading_out=(),
    to_stdout=False,
):
    return {
        "formats": list(formats),
        "flags": list(flags),
        "cache": cache,
        "paths": list(paths),
        "action": action,
        "leading_out": list(leading_out),
        "to_stdout": to_stdout,
    }


def _sequences():
    return [
        (
            "leading-output-del",
            [
                _step(leading_out=("unused-1.txt",)),
                _step(("json", "simple"), leading_out=("unused_\x7f.txt",)),
            ],
        ),
        (
            "relevant-option-order",
            [
                _step(flags=("-ES",)),
                _step(("json", "simple"), ("-SE",)),
                _step(("json", "simple"), ("-ES",)),
                _step(flags=("--safe", "--stderr")),
                _step(("json", "simple"), ("--stderr", "--safe")),
                _step(("json", "simple"), ("--safe", "--stderr")),
            ],
        ),
        (
            "display-filter-identities",
            [
                _step(),
                _step(("json", "simple"), ("--display-only-correctable",)),
                _step(("json", "simple"), ("--display-only-safe-correctable",)),
                _step(("json", "simple"), ("--display-only-fail-level-offenses",)),
                _step(("json", "simple"), ("--stderr",)),
                _step(("json", "simple"), ("--fail-level", "warning")),
                _step(("json", "simple")),
            ],
        ),
        (
            "default-stdout-to-explicit",
            [
                _step((), to_stdout=True),
                _step(to_stdout=True),
                _step(cache=False, to_stdout=True),
            ],
        ),
        (
            "leading-output-casefold",
            [
                _step(leading_out=("fold_K.txt",)),
                _step(("json", "simple"), leading_out=("fold_K.txt",)),
                _step(leading_out=("fold_k.txt",)),
                _step(leading_out=("fold_ſ.txt",)),
                _step(("json", "simple"), leading_out=("fold_s.txt",)),
                _step(("json", "simple"), leading_out=("fold_日本.txt",)),
                _step(("json", "simple"), leading_out=("fold_K.txt",)),
                _step(("json", "simple"), leading_out=("fold_ſ.txt",)),
            ],
        ),
        (
            "leading-output-identity",
            [
                _step(leading_out=("unused-a.txt",)),
                _step(("json", "simple"), leading_out=("unused-a.txt",)),
                _step(("json", "simple"), leading_out=("unused-b.txt",)),
                _step(leading_out=("unused-a.txt", "unused-b.txt")),
                _step(("json", "simple")),
                _step(leading_out=("unused-a.txt",)),
            ],
        ),
        (
            "leading-output-normalization",
            [
                _step(leading_out=("unused-1.txt",)),
                _step(("json", "simple"), leading_out=("unused-2.txt",)),
                _step(leading_out=("unused_日本.txt",)),
                _step(("json", "simple"), leading_out=("unused_別名.txt",)),
            ],
        ),
        (
            "json-to-simple-alias",
            [
                _step(),
                _step(("json", "simple")),
                _step(("json", "simple"), cache=False),
                _step(("j",)),
                _step(("j",), cache=False),
            ],
        ),
        (
            "simple-to-json-order",
            [
                _step(("json", "simple")),
                _step(),
                _step(("simple", "json")),
                _step(cache=False),
            ],
        ),
        ("default-json-to-explicit", [_step(()), _step(), _step(cache=False)]),
        (
            "explicit-options",
            [
                _step(),
                _step(("json", "simple"), ("--no-display-cop-names",)),
                _step(flags=("-D",)),
                _step(("json", "simple")),
                _step(flags=("-D", "--no-display-cop-names")),
                _step(flags=("--no-display-cop-names", "-D")),
                _step(("json", "simple"), ("-E",)),
                _step(("json", "simple")),
                _step(("json", "simple"), ("-S",)),
                _step(("json", "simple"), ("-E", "-S")),
            ],
        ),
        (
            "debug-after-json",
            [
                _step(),
                _step(flags=("--debug",)),
                _step(flags=("--debug",), cache=False),
            ],
        ),
        ("json-after-debug", [_step(flags=("--debug",)), _step(), _step(cache=False)]),
        (
            "touch-and-body",
            [
                _step(),
                _step(("json", "simple"), action="touch"),
                _step(("json", "simple")),
                _step(("json", "simple"), action="body-same-stat"),
            ],
        ),
        (
            "mixed-histories",
            [
                _step(paths=("first.rb",)),
                _step(("json", "simple"), paths=("second.rb",)),
                _step(("json", "simple"), paths=("first.rb", "second.rb")),
            ],
        ),
    ]


def _snapshot(paths, base):
    result = {}
    for path in sorted(paths):
        stat = path.stat()
        result[path.relative_to(base).as_posix()] = {
            "size": stat.st_size,
            "mtime_ns": stat.st_mtime_ns,
            "device": stat.st_dev,
            "inode": stat.st_ino,
            "mode": stat.st_mode,
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }
    return result


def _cache_snapshot(root):
    return (
        _snapshot(
            [path for path in (root / "cache").rglob("*") if path.is_file()], root
        )
        if (root / "cache").exists()
        else {}
    )


def _cache_messages(root):
    messages = []
    for path in (root / "cache").rglob("*") if (root / "cache").exists() else []:
        if path.is_file():
            rows = json.loads(path.read_bytes())
            messages.extend(row["message"] for row in rows)
    return sorted(messages)


def _output_path_keys(ruby, root, environment):
    # 対象 CLI と同じ Options / ResultCache を使い、空値や制御文字はファイルへ書かない。
    loader = (
        'gem "parser", "3.3.12.0"; gem "prism", "1.8.1"; '
        'gem "rubocop-ast", "1.50.0"; gem "rubocop", "1.89.0"; '
        'require "rubocop"; require "json"; '
        "rows = JSON.parse(STDIN.binmode.read).map do |value|; "
        'args = value.nil? ? [] : ["--out", value]; args += ["--format", "json"]; '
        "options, = RuboCop::Options.new.parse(args); "
        "{value: value, present: options.key?(:output_path), "
        "output_path: options[:output_path], "
        "normalized: RuboCop::ResultCache.relevant_options_digest(options)}; end; "
        "STDOUT.binmode.write(JSON.generate(rows))"
    )
    output = subprocess.run(
        [ruby, "-e", loader],
        cwd=root,
        env=environment,
        input=json.dumps(OUTPUT_PATHS).encode(),
        capture_output=True,
        check=True,
        timeout=90,
    )
    return json.loads(output.stdout)


def _option_keys(ruby, root, environment):
    loader = (
        'gem "parser", "3.3.12.0"; gem "prism", "1.8.1"; '
        'gem "rubocop-ast", "1.50.0"; gem "rubocop", "1.89.0"; '
        'require "rubocop"; require "json"; '
        "rows = JSON.parse(STDIN.binmode.read).map do |args|; "
        "options, = RuboCop::Options.new.parse(args.dup); "
        "RuboCop::ResultCache.instance_variable_set(:@relevant_options_digest, {}); "
        "{arguments: args, inspected: options.inspect, "
        "normalized: RuboCop::ResultCache.relevant_options_digest(options)}; end; "
        "STDOUT.binmode.write(JSON.generate(rows))"
    )
    output = subprocess.run(
        [ruby, "-e", loader],
        cwd=root,
        env=environment,
        input=json.dumps(_option_cases()).encode(),
        capture_output=True,
        check=True,
        timeout=90,
    )
    return json.loads(output.stdout)


def _inspect_keys(ruby, root, environment):
    # 非 ASCII 全 scalar の単文字 String#inspect から print 分類の区間を採る。
    loader = r"""
gem "parser", "3.3.12.0"
gem "prism", "1.8.1"
gem "rubocop-ast", "1.50.0"
gem "rubocop", "1.89.0"
require "rubocop"
require "json"
require "rbconfig"
rows = JSON.parse(STDIN.binmode.read).map do |value|
  options, = RuboCop::Options.new.parse(["--out", value, "--format", "json"])
  {value: value, inspected: value.inspect,
   normalized: RuboCop::ResultCache.relevant_options_digest(options)}
end
ranges = []
first = nil
last = nil
(128..0x10FFFF).each do |cp|
  next if (0xD800..0xDFFF).cover?(cp)
  value = cp.chr(Encoding::UTF_8)
  escaped = value.inspect != '"' + value + '"'
  if escaped
    if first && cp != last + 1
      ranges << [first, last]
      first = nil
    end
    first ||= cp
    last = cp
  elsif first
    ranges << [first, last]
    first = nil
  end
end
ranges << [first, last] if first
STDOUT.binmode.write(JSON.generate({cases: rows, nonprintable_ranges: ranges,
  ruby_version: RUBY_VERSION, unicode_version: RbConfig::CONFIG["UNICODE_VERSION"],
  default_external: Encoding.default_external.name}))
"""
    output = subprocess.run(
        [ruby, "-e", loader],
        cwd=root,
        env=environment,
        input=json.dumps(INSPECT_VALUES).encode(),
        capture_output=True,
        check=True,
        timeout=90,
    )
    return json.loads(output.stdout)


def _printable_table(inspect):
    """本家から得た非 ASCII 全 scalar の分類を、CLI 専用の固定区間へ出す。"""
    assert inspect["ruby_version"] == "4.0.7"
    assert inspect["unicode_version"] == "17.0.0"
    assert inspect["default_external"] == "UTF-8"
    lines = [
        "// Ruby 4.0.7 / Unicode 17.0.0 / UTF-8 の String#inspect 実測から生成。",
        (
            "// 再生成: python scripts/record_cached_message_annotations.py --ruby ruby"
            " --output new-cached-message-annotations.json"
            " --table-output new-cache-string-printable-table.rs"
        ),
        "// 手で編集しない。",
        "// 非 ASCII の全 1,111,936 scalar を調べた、非表示文字の 737 区間。",
        "pub(super) const NONPRINTABLE_RANGES: &[(u32, u32)] = &[",
    ]
    lines.extend(
        f"    (0x{start:04X}, 0x{end:04X}),"
        for start, end in inspect["nonprintable_ranges"]
    )
    lines.append("];")
    return ("\n".join(lines) + "\n").encode("utf-8")


def _collect(options, ruby):
    """公開先の全予約後にだけ、既存の観測本文を実行する。

    Returns:
        tuple: 元と同じfixture bytesと、指定時だけ生成するtable bytes。

    Raises:
        RuntimeError: 本家が診断終了0/1以外を返した場合。

    """
    loader = (
        'gem "parser", "3.3.12.0"; gem "prism", "1.8.1"; '
        'gem "rubocop-ast", "1.50.0"; gem "rubocop", "1.89.0"; '
        'require "rubocop"; require "json"; '
        'File.binwrite("versions.json", JSON.generate({ruby: RUBY_VERSION, '
        "rubocop: RuboCop::Version::STRING, "
        'parser: Gem.loaded_specs["parser"].version.to_s, '
        'prism: Gem.loaded_specs["prism"].version.to_s, '
        'rubocop_ast: Gem.loaded_specs["rubocop-ast"].version.to_s})); '
        'load Gem.bin_path("rubocop", "rubocop", "1.89.0")'
    )
    environment = _ruby_environment()
    sequences = []
    versions = None
    with tempfile.TemporaryDirectory(prefix="sonicop-cache-annotations-") as temporary:
        for name, steps in _sequences():
            if options.sequence and name not in options.sequence:
                continue
            root = Path(temporary) / name
            root.mkdir()
            (root / "config.yml").write_bytes(CONFIG.encode())
            names = (
                ("first.rb", "second.rb") if name == "mixed-histories" else ("case.rb",)
            )
            for source in names:
                (root / source).write_bytes(b"x=1\n")
            records = []
            for step_index, step in enumerate(steps):
                if step["action"]:
                    path = root / "case.rb"
                    stat = path.stat()
                    if step["action"] == "touch":
                        os.utime(
                            path,
                            ns=(stat.st_atime_ns, stat.st_mtime_ns + 2_000_000_000),
                        )
                    elif step["action"] == "body-same-stat":
                        path.write_bytes(b"y=2\n")
                        os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns))
                arguments = [
                    "--config",
                    "config.yml",
                    "--only",
                    "Layout/SpaceAroundOperators",
                    "--no-color",
                    "--cache",
                    str(step["cache"]).lower(),
                ]
                if step["cache"]:
                    arguments += ["--cache-root", "cache"]
                arguments += step["flags"]
                for leading_out in step["leading_out"]:
                    arguments += ["--out", leading_out]
                outputs = []
                formats = step["formats"] or ["json"]
                for index, format_name in enumerate(formats):
                    path = root / f"report-{index}.out"
                    path.unlink(missing_ok=True)
                    if step["formats"]:
                        arguments += ["--format", format_name]
                    if not step["to_stdout"]:
                        arguments += ["--out", path.name]
                    outputs.append((format_name, None if step["to_stdout"] else path))
                arguments += step["paths"]
                input_before = _snapshot(
                    [root / source for source in names] + [root / "config.yml"], root
                )
                cache_before = _cache_snapshot(root)
                output = subprocess.run(
                    [ruby, "-e", loader, "--", *arguments],
                    cwd=root,
                    env=environment,
                    capture_output=True,
                    check=False,
                    timeout=90,
                )
                if output.returncode not in (0, 1):
                    raise RuntimeError(
                        f"{name}/{step_index}: exit {output.returncode}; "
                        f"stdout={output.stdout!r}; stderr={output.stderr!r}"
                    )
                loaded_versions = json.loads((root / "versions.json").read_bytes())
                assert (
                    loaded_versions["ruby"] == "4.0.7"
                    and loaded_versions["rubocop"] == "1.89.0"
                )
                versions = loaded_versions
                rendered = []
                for format_name, path in outputs:
                    body = output.stdout if path is None else path.read_bytes()
                    if format_name in ["json", "j"]:
                        document = json.loads(body)
                        rendered.append(
                            {
                                "format": format_name,
                                "channel": "stdout" if path is None else "file",
                                "report": {
                                    key: document[key] for key in ["files", "summary"]
                                },
                            }
                        )
                    else:
                        rendered.append(
                            {
                                "format": format_name,
                                "channel": "stdout" if path is None else "file",
                                "text_hex": body.hex(),
                            }
                        )
                cache_after = _cache_snapshot(root)
                record = {
                    **step,
                    "arguments": arguments,
                    "exit": output.returncode,
                    "stderr": output.stderr.decode(),
                    "stdout_nonempty": bool(output.stdout),
                    "outputs": rendered,
                    "input_before": input_before,
                    "input_after": _snapshot(
                        [root / source for source in names] + [root / "config.yml"],
                        root,
                    ),
                    "cache_before": cache_before,
                    "cache_after": cache_after,
                    "cache_unchanged": cache_before == cache_after,
                    "cached_messages": _cache_messages(root),
                }
                assert record["input_before"] == record["input_after"]
                records.append(record)
            sequences.append({"name": name, "steps": records})
        output_path_keys = _output_path_keys(ruby, Path(temporary), environment)
        option_keys = _option_keys(ruby, Path(temporary), environment)
        inspect_keys = _inspect_keys(ruby, Path(temporary), environment)
    fixture = {
        "reference": versions,
        "platform": sys.platform,
        "configuration_hex": CONFIG.encode().hex(),
        "initial_source_hex": "783d310a",
        "output_path_keys": output_path_keys,
        "option_keys": option_keys,
        "inspect_keys": inspect_keys,
        "inspect_table_sha256": hashlib.sha256(
            _printable_table(inspect_keys)
        ).hexdigest(),
        "cli_option_policy": {
            key: value.split() for key, value in CLI_OPTION_POLICY.items()
        },
        "sequences": sequences,
    }
    encoded = (json.dumps(fixture, indent=2) + "\n").encode("ascii")
    return encoded, _printable_table(inspect_keys) if options.table_output else None


def main():
    """固定した本家を履歴ごとに実行し、保存前後の事実を JSON へ出す。

    採取・予約・保存の失敗は伝播し、新規0B/部分fileを証拠として残す。

    Raises:
        ValueError: 公開先が重複または占有されている場合。

    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", default="ruby")
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--table-output", type=Path, help="本家実測の CLI 専用 Unicode 分類表を保存する"
    )
    parser.add_argument(
        "--sequence", action="append", help="指定した履歴だけを独立して再記録する"
    )
    options = parser.parse_args()
    ruby = shutil.which(options.ruby)
    if not ruby:
        parser.error("Ruby executable not found")
    paths = {
        name: path.absolute()
        for name, path in (("fixture", options.output), ("table", options.table_output))
        if path is not None
    }
    keys = [os.path.normcase(os.path.normpath(path)) for path in paths.values()]
    if len(set(keys)) != len(keys):
        raise ValueError("outputとtable-outputに同じ公開先を指定しない")
    if any(path.exists() or path.is_symlink() for path in paths.values()):
        raise ValueError("既存の公開先を上書きしない。新しいpathを指定する")
    # 全leafの事前検査後に順に予約する。競合や失敗時の0B/部分fileは証拠として残す。
    with ExitStack() as publication:
        destinations = {}
        for name, path in paths.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            destinations[name] = publication.enter_context(path.open("xb"))
        encoded, table = _collect(options, ruby)
        if options.table_output:
            destinations["table"].write(table)
        if options.output:
            destinations["fixture"].write(encoded)
    if not options.output:
        sys.stdout.buffer.write(encoded)


if __name__ == "__main__":
    main()
