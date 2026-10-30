"""ENVの二十六getterと二十六CLIを、実版・全raw・原本保持から再採取する。"""

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

from record_cache_parallel_preflight import _fresh, _inventory, _same, _write_json
from record_config_cop_namespaces import LOADER
from record_remote_config_origins import _cache_files, _environment, _prepare
from record_verbatim_config_origins import _windows_name

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/target_ruby_environment.json"
DATA_SHA256 = "a27bcd62f0bc4b1da5a7aaad72c421fcd621ad7e6d5c8dacd5b78dc92dc5ad77"
API = (
    LOADER
    + r"""
require 'json'; require 'digest'; require 'stringio'
roots = {'RUBOCOP_ROOT' => Gem.loaded_specs.fetch('rubocop').full_gem_path.tr('\\','/'), 'AST_ROOT' => Gem.loaded_specs.fetch('rubocop-ast').full_gem_path.tr('\\','/')}
sources = %w[target_ruby.rb config.rb cli.rb runner.rb].to_h {|p| ['rubocop/'+p, Digest::SHA256.file(File.join(roots.fetch('RUBOCOP_ROOT'),'lib/rubocop',p)).hexdigest]}
sources['rubocop-ast/processed_source.rb'] = Digest::SHA256.file(File.join(roots.fetch('AST_ROOT'),'lib/rubocop/ast/processed_source.rb')).hexdigest
runtime = {ruby: RUBY_VERSION, platform: RUBY_PLATFORM, gems: %w[psych parser prism rubocop-ast rubocop rainbow].to_h {|p| [p, Gem.loaded_specs.fetch(p).version.to_s]}}
rows=JSON.parse(File.read(ARGV.fetch(0))).map do |row|
  output, errors = StringIO.new, StringIO.new
  previous = ENV.fetch('RUBOCOP_TARGET_RUBY_VERSION', nil)
  begin
    ENV['RUBOCOP_TARGET_RUBY_VERSION'] = row.fetch('value')
    $stdout, $stderr = output, errors
    value = Dir.chdir(row.fetch('root')) do
      c = RuboCop::ConfigLoader.load_file(row.fetch('config'))
      {kind: 'value', target_class: c.target_ruby_version.class.name, target_inspect: c.target_ruby_version.inspect, parser_engine: c.parser_engine.to_s}
    end
  rescue StandardError => error
    value = {kind: 'error', error_class: error.class.name, message: error.message}
  ensure
    previous.nil? ? ENV.delete('RUBOCOP_TARGET_RUBY_VERSION') : ENV['RUBOCOP_TARGET_RUBY_VERSION'] = previous
    $stdout, $stderr = STDOUT, STDERR
  end
  {id: row.fetch('id'), **value, stdout_hex: output.string.unpack1('H*'), stderr_hex: errors.string.unpack1('H*')}
end
puts JSON.generate({runtime: runtime, source_sha256: sources, roots: roots, rows: rows})
"""
)


def template():
    """型・出典・API・全rawを固定し、候補exeを使わず参照を読む。

    Returns:
        dict: 実本家から採取した二十六条件。

    """
    value = json.loads(FIXTURE.read_bytes())
    validate(value)
    return value


def validate(value):
    """全入力・型・比較範囲・backtrace参照の欠落を拒否する。

    Raises:
        ValueError: 固定した実参照と異なる場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
    ).hexdigest()
    if digest != DATA_SHA256:
        raise ValueError("固定参照の入力・型・raw・scopeが変わりました")
    _same(len(value["cases"]), 26)


def bind(text, bindings):
    """cwd・二つの実Gem配置・launcher以外の文字を保持する。

    Returns:
        str: 指定した動的部分だけを束縛した文字列。

    """
    for key, actual in bindings.items():
        text = text.replace(f"<{key}>", actual)
    return text


def _capture(ruby, args, root, files, environment, stdin=b""):
    before = _inventory(root, files)
    report = root / "report.json"
    report_before = _inventory(root, ["report.json"]) if os.path.lexists(report) else {}
    try:
        done = subprocess.run(
            [str(ruby), *args],
            cwd=root,
            env=_environment(
                {
                    "XDG_CACHE_HOME": str(root / "xdg"),
                    "XDG_CONFIG_HOME": str(root / "empty-config"),
                    **environment,
                }
            ),
            input=stdin,
            capture_output=True,
            timeout=60,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        _write_json(
            root / "private-process.json",
            {
                "exit": None,
                "timed_out": True,
                "stdout_hex": (error.stdout or b"").hex(),
                "stderr_hex": (error.stderr or b"").hex(),
            },
        )
        raise ValueError("本家processがtimeoutしました") from error
    raw = {
        "exit": done.returncode,
        "stdout_hex": done.stdout.hex(),
        "stderr_hex": done.stderr.hex(),
    }
    _write_json(root / "private-process.json", raw)
    after = _inventory(root, files)
    _cache_files(root)
    report_after = _inventory(root, ["report.json"]) if os.path.lexists(report) else {}
    _write_json(
        root / "private-run0.json",
        {
            **raw,
            "before": before,
            "after": after,
            "report_before": report_before,
            "report_after": report_after,
        },
    )
    _same(before, after)
    return done, report_before, report_after


def collect(ruby, work, output):
    """二十七実processのAPI・全CLI raw・原本・出典を照合後だけ新規保存する。

    Returns:
        dict: 全件の再採取が一致した参照。

    Raises:
        ValueError: 出典、raw、原本、report、保存先の検査が失敗した場合。

    """
    _fresh(work, output)
    if os.name != "nt":
        raise ValueError("固定Windows参照専用。非Windows実走へ投影しません")
    value = template()
    work.mkdir(parents=True)
    work = work.resolve()
    root = work / "api-runtime"
    api_files = {"Gemfile": ""}
    _prepare(root, api_files)
    for row in value["cases"]:
        _prepare(root / row["id"], row["files"])
        api_files.update({row["id"] + "/" + name: body for name, body in row["files"].items()})
    root = Path(_windows_name(root))
    forms, expected = [], []
    for row in value["cases"]:
        cwd = _windows_name(root / row["id"])
        forms.append(
            {
                "id": row["id"],
                "root": cwd,
                "value": row["environment"]["RUBOCOP_TARGET_RUBY_VERSION"],
                "config": row["args"][1],
            }
        )
        bindings = {"CWD": cwd, "CWD_FWD": cwd.replace("\\", "/")}
        expected.append({key: bind(data, bindings) for key, data in row["api"].items()})
    program, inputs = root / "reference_api.rb", root / "forms.json"
    with program.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(API)
    _write_json(inputs, forms)
    done, report_before, report_after = _capture(
        ruby, [str(program), str(inputs)], root, api_files, {}
    )
    _same(done.returncode, 0)
    _same(done.stderr, b"")
    _same(report_before, {})
    _same(report_after, {})
    observed = json.loads(done.stdout)
    roots = observed["roots"]
    _same(set(roots), {"RUBOCOP_ROOT", "AST_ROOT"})
    _same(
        observed,
        {
            "runtime": value["runtime"],
            "source_sha256": value["source_sha256"],
            "roots": roots,
            "rows": expected,
        },
    )
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(
            LOADER
            + "\nabort 'wrong Ruby' unless RUBY_VERSION == '4.0.7'; "
            + "load Gem.bin_path('rubocop', 'rubocop', '1.89.0')\n"
        )
    for row in value["cases"]:
        root = work / row["id"]
        _prepare(root, row["files"])
        cwd = _windows_name(root)
        bindings = {
            "CWD": cwd,
            "CWD_FWD": cwd.replace("\\", "/"),
            "LAUNCHER": launcher.as_posix(),
            **roots,
        }
        originals = {name: body for name, body in row["files"].items() if name != "report.json"}
        done, before, after = _capture(
            ruby,
            [str(launcher), *row["args"]],
            Path(cwd),
            originals,
            row["environment"],
            bytes.fromhex(row["stdin_hex"]),
        )
        _same(done.returncode, row["exit"])
        _same(done.stdout, bytes.fromhex(row["stdout_hex"]))
        _same(done.stderr, bind(row["stderr_template"], bindings).encode())
        _same(after["report.json"]["bytes_hex"], row["report_hex"])
        if row["report_preserved"]:
            _same(before, after)
    validate(value)
    _write_json(output, value)
    return value


def main():
    """固定本家だけを新規workで実行し、全rawを含む参照を新規保存する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "out"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
