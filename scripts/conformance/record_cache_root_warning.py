"""Windows実Rubyから固定19 CLI・10 join・16 realpath・11警告対を逐次再採取する。"""

import argparse
import copy
import ctypes
import hashlib
import json
import os
import subprocess
from pathlib import Path

from record_cache_parallel_preflight import (
    _fresh,
    _inventory,
    _same,
    _write_json,
    child_environment,
)
from record_config_cop_namespaces import LOADER

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/cache_root_warning.json"
DATA_SHA256 = "fbd2d05535f25ddb5a91cf159da0713cf7cac2d8d300751e5047ba22106cfc28"
API = (
    LOADER
    + """
require 'json'
require 'digest'
require 'stringio'
spec = Gem.loaded_specs.fetch('rubocop')
input = JSON.parse(ARGV.fetch(0))
paths = input.fetch('groups').map do |group|
  rows = group.fetch('paths').to_h do |name, path|
    [name, {path: path, exists: File.exist?(path), realpath: File.realpath(path)}]
  end
  pairs = group.fetch('pairs').map do |pair|
    stream = StringIO.new
    previous = $stderr
    begin
      $stderr = stream
      RuboCop::CLI.new.send(:warn_if_cache_root_changed,
        group['paths'].fetch(pair['early']), group['paths'].fetch(pair['desired']))
    ensure
      $stderr = previous
    end
    pair.merge('warning_string' => stream.string)
  end
  {paths: rows, pairs: pairs}
end
joins = input.fetch('join').map do |root|
  ENV.delete('RUBOCOP_CACHE_ROOT')
  configured = RuboCop::CacheConfig.root_dir { root }
  ENV['RUBOCOP_CACHE_ROOT'] = root
  environment = RuboCop::CacheConfig.root_dir { 'ignored' }
  ENV.delete('RUBOCOP_CACHE_ROOT')
  ENV['XDG_CACHE_HOME'] = root
  {input: root, configured: configured, environment: environment,
   xdg: RuboCop::CacheConfig.root_dir { nil }}
end
puts JSON.generate(runtime: {ruby: RUBY_VERSION,
  **%w[parser prism rubocop-ast rubocop rainbow].to_h {|n|
    [n, Gem.loaded_specs.fetch(n).version.to_s]}},
  source_sha256: %w[lib/rubocop/cli.rb lib/rubocop/cache_config.rb].to_h {|n|
    [n, Digest::SHA256.file(File.join(spec.full_gem_path, n)).hexdigest]},
  path_cases: joins, groups: paths)
"""
)


def template():
    """実本家に由来する入力・型・raw・scopeの固定版を読む。

    Returns:
        dict: 再採取対象の完全な参照。

    """
    value = json.loads(FIXTURE.read_bytes())
    validate(value)
    return value


def validate(value):
    """部分集合やboolとintの混同を、canonical hashを含めて拒否する。

    Raises:
        ValueError: 固定19条件、全raw、型又は出典が変わった場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != DATA_SHA256:
        raise ValueError("固定参照の入力・全raw・型・scopeが変わりました")
    _same(value["schema"], 2)
    _same(len(value["cases"]), 6)
    _same(len(value["additional_cases"]), 13)
    for row in value["cases"]:
        _same(row["exit"], 0)
        report = row["report"]
        _same(report["files"], [{"path": "input.rb", "offenses": []}])
        _same(
            report["summary"],
            {
                "offense_count": 0,
                "target_file_count": 1,
                "inspected_file_count": 1,
            },
        )
    for row in value["additional_cases"]:
        _same(row["exit"], 0)
        _same(row["source_bytes_mtime_mode_preserved"], True)


def bind(value, bindings):
    """明示した動的rootだけを再束縛する。

    Returns:
        object: root以外のbytes・型を変更しない独立copy。

    """
    if isinstance(value, str):
        for key, replacement in bindings.items():
            value = value.replace(f"<{key}>", replacement)
        return value
    if isinstance(value, dict):
        return {key: bind(item, bindings) for key, item in value.items()}
    if isinstance(value, list):
        return [bind(item, bindings) for item in value]
    return copy.deepcopy(value)


def _windows_name(path, short):
    output = ctypes.create_unicode_buffer(32768)
    api = (
        ctypes.windll.kernel32.GetShortPathNameW
        if short
        else ctypes.windll.kernel32.GetLongPathNameW
    )
    count = api(str(path), output, len(output))
    if not 0 < count < len(output):
        raise ValueError("実Windows名を取得できません")
    return output.value


def _junction(work, name, target):
    link = work / name
    if (
        not link.resolve(strict=False).is_relative_to(work.resolve())
        or not target.resolve().is_relative_to(work.resolve())
        or os.path.lexists(link)
        or any(char in str(link) + str(target) for char in '%"\r\n')
    ):
        raise ValueError("専用work外又は既存entryへjunctionを作成できません")
    done = subprocess.run(
        f'cmd.exe /d /v:off /c mklink /J "{link}" "{target}"',
        capture_output=True,
        check=False,
        timeout=30,
    )
    if done.returncode or not os.path.lexists(link) or not link.is_dir():
        raise ValueError("実junctionを作成できません")


def setup(work):
    """専用root内へ本家測定時と同じalias構造を作る。

    Returns:
        dict: 実長名・短縮名と明示したcase/drive/forward表記。

    Raises:
        ValueError: 実8.3名が得られず、測定条件を満たさない場合。

    """
    long = work / "a deliberately long real cache directory name"
    (long / "0/rubocop_cache").mkdir(parents=True)
    (work / "nested realpath parent/child").mkdir(parents=True)
    (work / "nested realpath parent/rubocop_cache").mkdir()
    (work / "rubocop_cache").mkdir()
    _junction(work, "junction alias", long)
    _junction(work, "nested junction", work / "nested realpath parent/child")
    long = _windows_name(long, False)
    short = _windows_name(long, True)
    if long == short:
        raise ValueError("実8.3名なし。未測定を成功に数えません")
    result = {
        "WORK": _windows_name(work, False),
        "LONG": long,
        "SHORT": short,
        "UPPER": long.upper(),
        "LOWER_LONG": long[0].lower() + long[1:],
    }
    result.update({key + "_FWD": value.replace("\\", "/") for key, value in list(result.items())})
    return result


def root(kind, bindings):
    """CLI用rootの有限集合を、本家の入力と同じ綴りで作る。

    Returns:
        str: 正規化していないroot文字列。

    """
    return {
        "long": bindings["LONG"],
        "short": bindings["SHORT"],
        "upper": bindings["UPPER"],
        "junction": bindings["WORK"] + "\\junction alias",
        "dot": bindings["LONG"] + "\\..\\a deliberately long real cache directory name",
        "tmp": "tmp",
    }[kind]


def _environment(extra):
    child = child_environment({"CLICOLOR_FORCE": "0"})
    for key in list(child):
        if key.upper() == "RUBOCOP_CACHE_ROOT":
            child.pop(key)
    child.update(extra)
    return child


def _capture(ruby, arguments, cwd, files, environment):
    before = _inventory(cwd, files)
    done = subprocess.run(
        [str(ruby), *arguments],
        cwd=cwd,
        env=_environment(environment),
        capture_output=True,
        check=False,
        timeout=60,
    )
    raw = {
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
    }
    # 失敗時にも、採れた実rawを属性検査や完成判定より先に保全する。
    _write_json(cwd / "private-process.json", raw)
    after = _inventory(cwd, files)
    _write_json(cwd / "private-run0.json", {**raw, "before": before, "after": after})
    _same(before, after)
    _same(done.returncode, 0)
    return done


def _cli_case(ruby, launcher, work, row, bindings, additional):
    cwd = work / row["id"]
    if os.path.lexists(cwd):
        raise ValueError("case directoryが既に存在します")
    cwd.mkdir()
    xdg = str(cwd / "cache")
    envroot = str(cwd / "environment-cache")
    environment = {"XDG_CACHE_HOME": xdg, "XDG_CONFIG_HOME": str(cwd / "empty-config")}
    if additional:
        configured = root(row["configured_kind"], bindings) + ("/0" if row["append_uid"] else "")
        xdg = root(row["desired_kind"], bindings)
        environment.update(row["env"])
        environment["XDG_CACHE_HOME"] = xdg
        config = row["config_template"].replace("<CONFIG_ROOT>", configured.replace("'", "''"))
        expected_stderr = row["stderr_template"].replace("<EARLY>", configured + "/rubocop_cache")
        expected_stderr = expected_stderr.replace("<DESIRED>", xdg + "/0/rubocop_cache")
    else:
        config = row["config"].replace("<XDG>", xdg)
        expected_stderr = row["stderr"].replace("<XDG>", xdg)
        if row["environment_root"] is not None:
            environment["RUBOCOP_CACHE_ROOT"] = envroot
    files = {
        ".rubocop.yml": config.encode(),
        "Gemfile": b"",
        "input.rb": bytes.fromhex(row["input_hex"]),
    }
    for name, data in files.items():
        (cwd / name).write_bytes(data)
    done = _capture(ruby, [str(launcher), *row["args"]], cwd, files, environment)
    _same(done.stderr, expected_stderr.encode())
    if additional:
        _same(done.stdout, bytes.fromhex(row["stdout_hex"]))
    else:
        _same(json.loads(done.stdout), row["report"])
        for path in (Path(xdg), Path(envroot), cwd / "tmp"):
            if os.path.lexists(path):
                raise ValueError("cache false実行がcache entryを作成しました")


def collect(ruby, work, output):
    """全APIと19 CLIの成功後だけ完成版をexclusive保存する。

    Returns:
        dict: 実測を照合した固定参照。

    Raises:
        ValueError: 保存先、実測runtime・全raw・属性又は完了条件が異なる場合。

    """
    _fresh(work, output)
    if os.name != "nt":
        raise ValueError("Windows実測専用。Unixへの投影は行いません")
    value = template()
    work.mkdir(parents=True)
    aliases = work / "aliases"
    aliases.mkdir()
    bindings = setup(aliases)
    groups = [bind(value["realpath_api"][key], bindings) for key in ("first", "additional")]
    arguments = {
        "join": [row["input"] for row in value["path_cases"]],
        "groups": [
            {
                "paths": {name: row["path"] for name, row in group["paths"].items()},
                "pairs": [
                    {key: pair[key] for key in ("early", "desired")} for pair in group["pairs"]
                ],
            }
            for group in groups
        ],
    }
    done = _capture(ruby, ["-e", API, json.dumps(arguments)], aliases, {}, {})
    _same(done.stderr, b"")
    observed = json.loads(done.stdout)
    _same(
        observed,
        {
            "runtime": value["runtime"],
            "source_sha256": value["source_sha256"],
            "path_cases": value["path_cases"],
            "groups": [{key: group[key] for key in ("paths", "pairs")} for group in groups],
        },
    )
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n')
    for row in value["cases"]:
        _cli_case(ruby, launcher.absolute(), work, row, bindings, False)
    for row in value["additional_cases"]:
        _cli_case(ruby, launcher.absolute(), work, row, bindings, True)
    validate(value)
    _write_json(output, value)
    return value


def main():
    """候補exeを使わず、固定Windows参照だけを再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
