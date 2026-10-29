"""RuboCop1.89の設定namespace解決を、候補を呼ばずに記録する。"""

import argparse
import copy
import hashlib
import json
import os
import subprocess
from pathlib import Path

VERSIONS = {
    "parser": "3.3.12.0",
    "prism": "1.8.1",
    "rubocop-ast": "1.50.0",
    "rubocop": "1.89.0",
    "rainbow": "3.1.1",
}
LOADER = "gem 'psych', '5.3.1'\n"
LOADER += "\n".join(f"gem '{name}', '{version}'" for name, version in VERSIONS.items())
LOADER += "\nrequire 'rubocop'\n"
LOADER += "\n".join(
    f"raise 'unexpected {name}' unless Gem.loaded_specs.fetch('{name}').version.to_s == '{version}'"
    for name, version in {"psych": "5.3.1", **VERSIONS}.items()
)
ENV_KEYS = {"RUBYOPT", "RUBYLIB", "RUBOCOP_OPTS", "RUBOCOP_TARGET_RUBY_VERSION"}


def reference_environment():
    """親のRuby注入変数だけを子copyから除き、GEM/PATH/OS環境を保つ。

    Returns:
        dict: 本家を起動する環境。親の環境は変更しない。

    """
    environment = dict(os.environ)
    for key in list(environment):
        if (key.upper() if os.name == "nt" else key) in ENV_KEYS:
            environment.pop(key)
    return environment


BASE = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n"
SPACE = "Layout/SpaceAroundOperators"
STYLE = "Style/StringLiterals"


def _section(name, value="  Enabled: true\n"):
    return name + ":\n" + value


FORMS = []


def _add(name, body, *, extra=None, source="x=1\n", args=None, env=None, cfg="config.yml"):
    FORMS.append(
        {
            "id": name,
            "files": {"Gemfile": "", "a.rb": source, cfg: BASE + body, **(extra or {})},
            "args": args or ["--config", cfg, "--cache", "false", "--format", "json", "a.rb"],
            "env": env or {},
        }
    )


_add("full", _section(SPACE))
_add("short", _section("SpaceAroundOperators"))
_add("short_false", _section("SpaceAroundOperators", "  Enabled: false\n"))
_add("wrong_namespace", _section("Wrong/SpaceAroundOperators"))
_add("short_snake", _section("space_around_operators"))
_add("qualified_snake", _section("layout/space_around_operators"))
_add("unknown", _section("Unknown/Cop"))
_add("unknown_bare", _section("NoSuchCop"))
_add("obsolete", _section("Metrics/LineLength"))
_add("ambiguous_multiple", _section("MultipleComparison"))
_add("ambiguous_wrong", _section("Wrong/MultipleComparison"))
_add("ambiguous_self", _section("SelfAssignment"))
_add("ambiguous_full", _section("Lint/MultipleComparison"))
_add("ruby_extension", _section("SpaceAroundOperators"), cfg="config.rb")
bad = "  Enabled: true\n  WrongKey: true\n"
_add("short_parameter", _section("SpaceAroundOperators", bad))
_add("wrong_parameter", _section("Wrong/SpaceAroundOperators", bad))
_add("initial_color", _section("SpaceAroundOperators", bad), env={"CLICOLOR_FORCE": "1"})
_add(
    "explicit_color",
    _section("SpaceAroundOperators", bad),
    args=["--color", "--config", "config.yml", "--cache", "false", "--format", "json", "a.rb"],
)
_add(
    "collision_short_first",
    _section("SpaceAroundOperators", "  Enabled: false\n") + _section(SPACE),
)
_add(
    "collision_full_first", _section(SPACE) + _section("SpaceAroundOperators", "  Enabled: false\n")
)
_add(
    "collision_short_no_enabled",
    _section(SPACE) + _section("SpaceAroundOperators", "  Severity: warning\n"),
)
_add(
    "collision_full_no_enabled",
    _section("SpaceAroundOperators") + _section(SPACE, "  Severity: warning\n"),
)
_add(
    "order_existing_target",
    _section("SpaceAroundOperators", "  Enabled: true\n  FirstWrong: true\n")
    + _section(STYLE, "  Enabled: false\n  MiddleWrong: true\n")
    + _section(SPACE, "  Enabled: false\n  LastWrong: true\n"),
)
_add(
    "order_new_target",
    _section("SpaceAroundOperators", "  Enabled: true\n  FirstWrong: true\n")
    + _section(STYLE, "  Enabled: false\n  MiddleWrong: true\n")
    + _section("Lint/Debugger", "  Enabled: false\n  LastWrong: true\n"),
)
_add(
    "two_short",
    _section("SpaceAroundOperators", bad)
    + _section("StringLiterals", "  Enabled: false\n  AnotherWrong: true\n"),
)
parent_full = BASE + _section(SPACE, "  Enabled: true\n  EnforcedStyleForExponentOperator: space\n")
parent_short = BASE + _section(
    "SpaceAroundOperators", "  Enabled: true\n  EnforcedStyleForExponentOperator: space\n"
)
_add(
    "parent_short_child_full",
    "inherit_from: base.yml\n" + _section(SPACE, "  Severity: warning\n"),
    extra={"base.yml": parent_short},
    source="x = 2**3\n",
)
_add(
    "parent_full_child_short",
    "inherit_from: base.yml\n" + _section("SpaceAroundOperators", "  Severity: warning\n"),
    extra={"base.yml": parent_full},
    source="x = 2**3\n",
)
_add(
    "parent_short_child_short",
    "inherit_from: base.yml\n" + _section("SpaceAroundOperators", "  Enabled: false\n"),
    extra={"base.yml": parent_short},
    source="x = 2**3\n",
)
_add(
    "parent_wrong_child_full",
    "inherit_from: base.yml\n" + _section(SPACE, "  Severity: warning\n"),
    extra={"base.yml": BASE + _section("Wrong/SpaceAroundOperators")},
)
_add(
    "diamond",
    "inherit_from: [left.yml, right.yml]\n",
    extra={
        "leaf.yml": BASE + _section("SpaceAroundOperators", bad),
        "left.yml": "inherit_from: leaf.yml\n",
        "right.yml": "inherit_from: leaf.yml\n",
    },
)
_add(
    "parent_parameter_child_short",
    "inherit_from: base.yml\n"
    + _section("SpaceAroundOperators", "  Enabled: true\n  ChildWrong: true\n"),
    extra={"base.yml": BASE + _section("SpaceAroundOperators", bad)},
)
_add(
    "short_safe",
    _section("SpaceAroundOperators"),
    args=["--config", "config.yml", "--cache", "false", "--format", "json", "-a", "a.rb"],
)
_add(
    "short_unsafe",
    _section("SpaceAroundOperators"),
    args=["--config", "config.yml", "--cache", "false", "--format", "json", "-A", "a.rb"],
)
_add(
    "full_safe",
    _section(SPACE),
    args=["--config", "config.yml", "--cache", "false", "--format", "json", "-a", "a.rb"],
)
_add(
    "short_stdin",
    _section("SpaceAroundOperators"),
    args=["--config", "config.yml", "--cache", "false", "--format", "json", "--stdin", "a.rb"],
)
for count in (1, 2):
    files = {
        "Gemfile": "",
        ".rubocop.yml": BASE,
        "a.rb": "x=1\n",
        "nested/.rubocop.yml": "inherit_from: ../.rubocop.yml\n" + _section("SpaceAroundOperators"),
        "nested/b.rb": "y=2\n",
    }
    paths = ["a.rb", "nested/b.rb"]
    if count == 2:
        files["nested/c.rb"] = "z=3\n"
        paths.append("nested/c.rb")
    FORMS.append(
        {
            "id": f"discovered_{count}",
            "files": files,
            "args": ["--cache", "false", "--format", "json", *paths],
            "env": {},
        }
    )


_add(
    "department_disabled_short_enabled",
    _section("Layout", "  Enabled: false\n") + _section("SpaceAroundOperators"),
)
_add(
    "department_disabled_short_inherited",
    "inherit_from: base.yml\n"
    + _section("Layout", "  Enabled: false\n")
    + _section("SpaceAroundOperators"),
    extra={"base.yml": BASE + _section(SPACE, "  Enabled: true\n  ParentWrong: true\n")},
)
_add(
    "parent_department_disabled_child_short",
    "inherit_from: base.yml\n"
    + _section("SpaceAroundOperators", "  Enabled: true\n  ChildWrong: true\n"),
    extra={
        "base.yml": BASE
        + _section("Layout", "  Enabled: false\n")
        + _section(SPACE, "  Enabled: true\n  ParentWrong: true\n")
    },
)
_add(
    "inherited_namespace_prefix_before_ambiguity",
    "inherit_from: base.yml\n"
    + _section("StringLiterals", "  Enabled: false\n")
    + _section("MultipleComparison"),
    extra={
        "base.yml": BASE
        + _section("SpaceAroundOperators", "  Enabled: true\n  ParentWrong: true\n")
    },
)
FORMS.append(
    {
        "id": "parent_exclude_namespace_reload",
        "files": {
            "Gemfile": "",
            ".rubocop.yml": BASE + _section("SpaceAroundOperators"),
            "nested/.rubocop.yml": "inherit_from: ../.rubocop.yml\n"
            + _section("StringLiterals", "  Enabled: false\n"),
            "nested/b.rb": "y=2\n",
        },
        "args": ["--cache", "false", "--format", "json", "nested/b.rb"],
        "env": {},
    }
)


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def case_inputs():
    """値解決・継承・slot順と失敗prefixを42条件へ固定する。

    Returns:
        list: 比較範囲と対象pathを明示した全入力のcopy。

    """
    forms = copy.deepcopy(FORMS)
    for form in forms:
        form["env"] = {"CLICOLOR_FORCE": "0", **form["env"]}
        name = form["id"]
        form["coverage"] = (
            "known_residual"
            if name == "unknown_bare"
            else "ambiguity_bytes"
            if name
            in {
                "ambiguous_multiple",
                "ambiguous_wrong",
                "ambiguous_self",
                "inherited_namespace_prefix_before_ambiguity",
            }
            else "rejection_envelope"
            if name in {"qualified_snake", "unknown", "obsolete"}
            else "json"
        )
        form["targets"] = (
            [name for name in form["files"] if name.endswith(".rb")]
            if name.startswith("discovered_") or name == "parent_exclude_namespace_reload"
            else ["a.rb"]
        )
    return forms


API = (
    LOADER
    + """
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
puts JSON.generate({
  runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
    gems: %w[parser prism rubocop-ast rubocop rainbow].to_h { |n| [n, Gem.loaded_specs.fetch(n).version.to_s] }},
  registry: RuboCop::Cop::Registry.global.names,
  legacy: RuboCop::ConfigObsoletion.new({}).legacy_cop_names,
  obsoletion_sha256: Digest::SHA256.file(File.join(spec.full_gem_path, 'config/obsoletion.yml')).hexdigest
})
"""
)


def _registry(ruby, work):
    done = subprocess.run(
        [ruby, "-e", API],
        cwd=work,
        env=reference_environment(),
        capture_output=True,
        check=False,
        timeout=60,
    )
    _require(done.returncode == 0 and not done.stderr, "実Registry照会が失敗")
    value = json.loads(done.stdout)
    _require(
        value["runtime"]["ruby"] == "4.0.7"
        and value["runtime"]["gems"] == VERSIONS
        and "mingw" in value["runtime"]["platform"],
        "実runtimeが不一致",
    )
    _require(len(value["registry"]) == len(set(value["registry"])) == 609, "Registryが不完全")
    _require(len(value["legacy"]) == len(set(value["legacy"])) == 67, "obsolete集合が不完全")
    return value


def _report(raw, targets):
    _require(bool(targets) and len(targets) == len(set(targets)), "対象pathが空か重複")
    value = json.loads(raw)
    metadata = value.pop("metadata")
    _require(
        metadata["rubocop_version"] == "1.89.0"
        and metadata["ruby_engine"] == "ruby"
        and metadata["ruby_version"] == "4.0.7",
        "本家JSONのidentityが不一致",
    )
    _require([file["path"] for file in value["files"]] == targets, "対象path/順が不一致")
    summary = value["summary"]
    _require(
        summary["target_file_count"] == summary["inspected_file_count"] == len(targets),
        "検査完了数が不一致",
    )
    _require(
        summary["offense_count"] == sum(len(file["offenses"]) for file in value["files"]),
        "診断件数が不一致",
    )
    return value


def _public_stderr(raw, directory, coverage):
    text = raw.decode()
    if coverage == "ambiguity_bytes":
        _require(raw.endswith(b"\r\n"), "曖昧名の最終native改行が不一致")
        origin = (directory / "config.yml").as_posix()
        needle = f" used in {origin} needs department qualifier."
        _require(text.count(needle) == 1, "動的originの位置が不一致")
        text = text.replace(needle, " used in <cwd>/config.yml needs department qualifier.")
    return text


def _record(index, form, ruby, launcher, work):
    directory = work / f"{index:02}-{form['id']}"
    directory.mkdir()
    for name, source in form["files"].items():
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(source.encode())
    environment = reference_environment()
    environment.update(form["env"])
    done = subprocess.run(
        [ruby, str(launcher), *form["args"]],
        cwd=directory,
        env=environment,
        input=form["files"]["a.rb"].encode() if "--stdin" in form["args"] else None,
        capture_output=True,
        check=False,
        timeout=60,
    )
    after = {name: (directory / name).read_bytes().hex() for name in form["files"]}
    raw = {
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
        "after_hex": after,
    }
    (directory / "raw.json").write_text(json.dumps(raw, indent=2), encoding="utf-8", newline="\n")
    report = None
    if form["coverage"] == "json":
        _require(done.returncode in {0, 1}, f"{form['id']}: 通常検査が失敗")
        report = _report(done.stdout, form["targets"])
    else:
        _require(done.returncode == 2 and not done.stdout and done.stderr, "拒否境界が不一致")
    for name, source in form["files"].items():
        if name.endswith(".rb") and ("-a" in form["args"] or "-A" in form["args"]):
            continue
        _require(after[name] == source.encode().hex(), "非補正原本が変わった")
    stderr = _public_stderr(done.stderr, directory, form["coverage"])
    return {
        **form,
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": stderr.encode().hex(),
        "stderr_lf": stderr.replace("\r\n", "\n"),
        "raw_stdout_sha256": hashlib.sha256(done.stdout).hexdigest(),
        "raw_stderr_sha256": hashlib.sha256(done.stderr).hexdigest(),
        "after_hex": after,
        "report": report,
    }


def _validate_cases(cases):
    forms = case_inputs()
    _require(
        len(cases) == 42 and [case["id"] for case in cases] == [form["id"] for form in forms],
        "有限行列が不完全",
    )
    counts = {
        name: sum(case["coverage"] == name for case in cases)
        for name in ("json", "ambiguity_bytes", "rejection_envelope", "known_residual")
    }
    _require(
        counts == {"json": 34, "ambiguity_bytes": 4, "rejection_envelope": 3, "known_residual": 1},
        "比較範囲が不一致",
    )


def main():
    """本家42CLIと実Registryを照合し、全件完了した期待だけを新規保存する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "out", "legacy-out"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    destinations = [Path(value).absolute() for value in (args.work, args.out, args.legacy_out)]
    _require(len(set(destinations)) == 3, "work/outは異なるpathが必要")
    _require(not any(os.path.lexists(path) for path in destinations), "既存work/outを上書きしない")
    work = destinations[0]
    work.mkdir(parents=True, exist_ok=False)
    api = _registry(args.ruby, work)
    launcher = work / "reference.rb"
    launcher.write_text(
        LOADER + "\nload Gem.bin_path('rubocop', 'rubocop', '1.89.0')\n",
        encoding="utf-8",
        newline="\n",
    )
    cases = [
        _record(index, form, args.ruby, launcher, work) for index, form in enumerate(case_inputs())
    ]
    _validate_cases(cases)
    for destination, value in (
        (destinations[1], {"schema": 1, "api": api, "cases": cases}),
        (destinations[2], api["legacy"]),
    ):
        # x mode は採取中に現れたentryやdangling symlinkも上書きしない。
        with destination.open("x", encoding="utf-8", newline="\n") as stream:
            stream.write(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    print(
        f"recorded {len(cases)} CLI; 34 JSON / 4 ambiguity body / 3 rejection / 1 excluded residual"
    )


if __name__ == "__main__":
    main()
