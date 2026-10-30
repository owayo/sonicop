"""固定Windowsの本家から、origin展開10行とCLI10条件を再採取する。"""

import argparse
import copy
import ctypes
import hashlib
import json
import os
import subprocess
from pathlib import Path

from record_cache_parallel_preflight import _fresh, _inventory, _same, _write_json
from record_config_cop_namespaces import LOADER
from record_remote_config_origins import _cache_files, _environment, _marker, _prepare

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/verbatim_config_origins.json"
DATA_SHA256 = "c3d22792107c7a7eb291ae351699f37405ab06a79064d364c73305bb737e4aa6"
API = (
    LOADER
    + """
require 'digest'
require 'json'
spec = Gem.loaded_specs.fetch('rubocop')
rows = JSON.parse(ARGV.fetch(0)).map do |row|
  expanded = if row.key?('base')
               File.absolute_path(row.fetch('input'), row.fetch('base'))
             else
               File.absolute_path(row.fetch('input'))
             end
  row.merge('absolute_path' => expanded)
end
sources = %w[lib/rubocop/config_loader.rb lib/rubocop/path_util.rb
             lib/rubocop/cop/registry.rb]
puts JSON.generate(runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow psych].to_h {|name|
    [name, Gem.loaded_specs.fetch(name).version.to_s]}},
  source_sha256: sources.to_h {|name|
    [name, Digest::SHA256.file(File.join(spec.full_gem_path, name)).hexdigest]},
  rows: rows)
"""
)


def template():
    """期待値を候補から作らず、固定した本家の全入力・rawを読む。

    Returns:
        dict: 型と比較範囲を保持した参照。

    """
    value = json.loads(FIXTURE.read_bytes())
    validate(value)
    return value


def validate(value):
    """API・CLIの空集合、型の変化、rawと比較範囲の改変を拒否する。

    Raises:
        ValueError: 固定した参照と一致しない場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
    ).hexdigest()
    if digest != DATA_SHA256:
        raise ValueError("固定参照の入力・型・raw・scopeが変わりました")
    _same(len(value["api"]), 10)
    _same(len(value["cases"]), 10)
    for row in value["cases"]:
        _same(row["source_bytes_mtime_mode_preserved"], True)
        stdout = bytes.fromhex(row["stdout_hex"])
        fatal = "ambiguous" in row["id"]
        _same(row["exit"], 2 if fatal else 1)
        if fatal:
            _same(stdout, b"")
        else:
            report = json.loads(stdout)
            _same(report["metadata"]["rubocop_version"], "1.89.0")
            _same(report["metadata"]["ruby_engine"], "ruby")
            _same(
                report["summary"],
                {
                    "offense_count": 1,
                    "target_file_count": 1,
                    "inspected_file_count": 1,
                },
            )
            _same(len(report["files"]), 1)
            _same(report["files"][0]["path"], "a.rb")
            _same(len(report["files"][0]["offenses"]), 1)


def bind(text, bindings):
    """明示したcwdと実8.3 basenameだけを束縛する。

    Returns:
        str: その他のbytesの表記を変えない入力。

    """
    for key, actual in bindings.items():
        text = text.replace(f"<{key}>", actual)
    return text


def _windows_name(path, short=False):
    name = "GetShortPathNameW" if short else "GetLongPathNameW"
    function = getattr(ctypes.WinDLL("kernel32", use_last_error=True), name)
    function.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint]
    function.restype = ctypes.c_uint
    output = ctypes.create_unicode_buffer(32_768)
    length = function(str(path), output, len(output))
    if not 0 < length < len(output):
        raise OSError(ctypes.get_last_error(), name)
    return output.value


def _capture(ruby, args, root, files, expected_exit):
    before = _inventory(root, files)
    if _marker(root) is not None:
        raise ValueError("marker entryは既に存在します")
    environment = _environment(
        {
            "XDG_CACHE_HOME": str(root / "xdg-cache"),
            "XDG_CONFIG_HOME": str(root / "empty-config"),
        }
    )
    try:
        done = subprocess.run(
            [str(ruby), *args],
            cwd=root,
            env=environment,
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
    marker = _marker(root)
    # API用のlong filenameも通常原本なので、返り値をcache artifactとは呼ばない。
    _cache_files(root)
    _write_json(
        root / "private-run0.json", {**raw, "before": before, "after": after, "marker": marker}
    )
    _same(before, after)
    _same(done.returncode, expected_exit)
    return done, marker


def collect(ruby, work, output):
    """実API一件と実CLI十件を完了した後だけ、全参照をexclusive保存する。

    Returns:
        dict: 固定Ruby/gemと実rawに一致した全参照。

    Raises:
        ValueError: 出典・原本・raw・保存先又は実8.3名が異なる場合。

    """
    _fresh(work, output)
    if os.name != "nt":
        raise ValueError("固定Windows参照専用。非Windows実走へ投影しません")
    value = template()
    work.mkdir(parents=True)
    work = work.resolve()
    root = work / "api-runtime"
    _prepare(root, value["api_files"])
    cwd = _windows_name(root)
    short_file = Path(_windows_name(root / "long-configuration-filename.yml", True)).name
    if short_file == "long-configuration-filename.yml":
        raise ValueError("実8.3名が必要。未計測をAPI十行の成功にしません")
    bindings = {"CWD": cwd, "CWD_FWD": cwd.replace("\\", "/"), "SHORT_FILE": short_file}
    forms = [
        {key: bind(data, bindings) for key, data in row.items() if key != "absolute_path"}
        for row in value["api"]
    ]
    done, marker = _capture(ruby, ["-e", API, json.dumps(forms)], Path(cwd), value["api_files"], 0)
    _same(done.stderr, b"")
    _same(marker, None)
    expected = [{key: bind(data, bindings) for key, data in row.items()} for row in value["api"]]
    _same(
        json.loads(done.stdout),
        {
            "runtime": value["runtime"],
            "source_sha256": value["source_sha256"],
            "rows": expected,
        },
    )
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n')
    for row in value["cases"]:
        root = work / row["id"]
        _prepare(root, row["files"])
        cwd = _windows_name(root)
        bindings = {"CWD": cwd, "CWD_FWD": cwd.replace("\\", "/")}
        args = [bind(arg, bindings) for arg in row["args"]]
        done, marker = _capture(ruby, [str(launcher), *args], Path(cwd), row["files"], row["exit"])
        _same(done.stdout, bytes.fromhex(row["stdout_hex"]))
        _same(done.stderr, bind(row["stderr_template"], bindings).encode())
        _same(None if marker is None else marker["bytes_hex"], row["marker_hex"])
    validate(value)
    _write_json(output, value)
    return copy.deepcopy(value)


def main():
    """候補exeを受け取らず、元の11実processを新規workへ再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "out"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
