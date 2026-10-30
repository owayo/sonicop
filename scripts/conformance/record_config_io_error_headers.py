"""本家の設定I/O五API・十CLIを、実lockと全rawを保って再採取する。"""

import argparse
import ctypes
import hashlib
import json
import os
import stat
import subprocess
from contextlib import contextmanager
from pathlib import Path

from record_cache_parallel_preflight import _fresh, _inventory, _same, _write_json
from record_config_cop_namespaces import LOADER
from record_remote_config_origins import _cache_files, _environment, _prepare, _reparse
from record_verbatim_config_origins import _windows_name

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/config_io_error_headers.json"
DATA_SHA256 = "6a6dbce47beff44668d189810ae5be4fc44463897865437f2af240d43367e23a"
API = (
    LOADER
    + """
require 'json'
require 'digest'
spec = Gem.loaded_specs.fetch('rubocop')
rows = JSON.parse(ARGV.fetch(0)).map do |row|
  path = File.absolute_path(row.fetch('path'))
  begin
    value = RuboCop::ConfigLoader.send(:read_file, path)
    row.merge('kind' => 'value', 'value_hex' => value.b.unpack1('H*'))
  rescue StandardError => error
    row.merge('kind' => 'error', 'class' => error.class.name, 'message' => error.message)
  end
end
puts JSON.generate(runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM,
  gems: %w[parser prism rubocop-ast rubocop rainbow psych].to_h {|name|
    [name, Gem.loaded_specs.fetch(name).version.to_s]}},
  source_sha256: %w[config_loader.rb cli.rb options.rb].to_h {|name|
    [name, Digest::SHA256.file(File.join(spec.full_gem_path, 'lib/rubocop', name)).hexdigest]},
  gem_root: spec.full_gem_path.tr('\\\\', '/'), rows: rows)
"""
)


def template():
    """候補を起動せず、本家の全入力・raw・比較範囲を固定して読む。

    Returns:
        dict: 五API行と十CLI条件の実参照。

    """
    value = json.loads(FIXTURE.read_bytes())
    validate(value)
    return value


def validate(value):
    """参照と型・全raw・有限の比較範囲の改変を拒否する。

    Raises:
        ValueError: 固定した参照と一致しない場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
    ).hexdigest()
    if digest != DATA_SHA256:
        raise ValueError("固定参照の入力・型・raw・scopeが変わりました")
    _same(len(value["api"]), 5)
    _same(len(value["cases"]), 10)
    for row in value["cases"]:
        _same(row["source_bytes_mtime_mode_preserved"], True)
        _same(row["directory_mtime_mode_preserved"], True)
        _same(row["exit"], 0 if row["comparison"] == "report" else 2)
        _same(bytes.fromhex(row["stdout_hex"]), b"")


def bind(text, bindings):
    """cwd・Gem実体・launcherだけを束縛し、frameやCRLFを変えない。

    Returns:
        str: 明示binding以外を保持した文字列。

    """
    for key, actual in bindings.items():
        text = text.replace(f"<{key}>", actual)
    return text


@contextmanager
def _locked(path):
    if path is None:
        yield
        return
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    create, close = kernel.CreateFileW, kernel.CloseHandle
    create.argtypes = [
        ctypes.c_wchar_p,
        ctypes.c_uint,
        ctypes.c_uint,
        ctypes.c_void_p,
        ctypes.c_uint,
        ctypes.c_uint,
        ctypes.c_void_p,
    ]
    create.restype = ctypes.c_void_p
    close.argtypes = [ctypes.c_void_p]
    close.restype = ctypes.c_int
    handle = create(str(path), 0x80000000, 0, None, 3, 0x80, None)
    if handle in (None, ctypes.c_void_p(-1).value):
        raise OSError(ctypes.get_last_error(), "CreateFileW")
    try:
        yield
    finally:
        if not close(handle):
            raise OSError(ctypes.get_last_error(), "CloseHandle")


def _directories(root, names):
    result = {}
    for name in names:
        info = (root / name).lstat()
        if _reparse(info) or not stat.S_ISDIR(info.st_mode):
            raise ValueError("directoryは通常entryではありません")
        result[name] = {"mode": info.st_mode, "mtime_ns": info.st_mtime_ns}
    return result


def _capture(ruby, args, root, files, directories, environment, locked_file):
    before = _inventory(root, files)
    dirs_before = _directories(root, directories)
    try:
        with _locked(None if locked_file is None else root / locked_file):
            done = subprocess.run(
                [str(ruby), *args],
                cwd=root,
                env=_environment(
                    {
                        "XDG_CACHE_HOME": str(root / "xdg-cache"),
                        "XDG_CONFIG_HOME": str(root / "empty-config"),
                        **environment,
                    }
                ),
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
    dirs_after = _directories(root, directories)
    _cache_files(root)
    out = _inventory(root, ["out.json"]) if os.path.lexists(root / "out.json") else {}
    _write_json(
        root / "private-run0.json",
        {
            **raw,
            "before": before,
            "after": after,
            "directories_before": dirs_before,
            "directories_after": dirs_after,
            "out": out,
        },
    )
    _same(before, after)
    _same(dirs_before, dirs_after)
    return done, out


def collect(ruby, work, output):
    """十一実processと全原本・directory・reportを照合してから新規保存する。

    Returns:
        dict: 全rawと出典が一致した固定参照。

    Raises:
        ValueError: 出典・属性・全raw・lock又は保存先の検査が失敗した場合。

    """
    _fresh(work, output)
    if os.name != "nt":
        raise ValueError("固定Windows参照専用。非Windows実走へ投影しません")
    value = template()
    work.mkdir(parents=True)
    work = work.resolve()
    root = work / "api-runtime"
    _prepare(root, value["api_files"])
    (root / "configdir").mkdir()
    cwd = _windows_name(root)
    bindings = {"CWD": cwd, "CWD_FWD": cwd.replace("\\", "/")}
    forms = [{key: bind(row[key], bindings) for key in ("id", "path")} for row in value["api"]]
    done, out = _capture(
        ruby,
        ["-e", API, json.dumps(forms)],
        Path(cwd),
        value["api_files"],
        ["configdir"],
        {},
        "locked.yml",
    )
    _same(done.returncode, 0)
    _same(done.stderr, b"")
    _same(out, {})
    observed = json.loads(done.stdout)
    gem_root = observed["gem_root"]
    bindings["GEM_ROOT"] = gem_root
    expected = [{key: bind(data, bindings) for key, data in row.items()} for row in value["api"]]
    _same(
        observed,
        {
            "runtime": value["runtime"],
            "source_sha256": value["source_sha256"],
            "gem_root": bind(value["gem_root_template"], bindings),
            "rows": expected,
        },
    )
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(
            LOADER
            + "\nabort 'wrong Ruby' unless RUBY_VERSION == '4.0.7'\n"
            + "load Gem.bin_path('rubocop', 'rubocop', '1.89.0')\n"
        )
    for row in value["cases"]:
        root = work / row["id"]
        _prepare(root, row["files"])
        for name in row["directories"]:
            (root / name).mkdir()
        cwd = _windows_name(root)
        bindings = {
            "CWD": cwd,
            "CWD_FWD": cwd.replace("\\", "/"),
            "GEM_ROOT": gem_root,
            "LAUNCHER": launcher.as_posix(),
        }
        args = [bind(arg, bindings) for arg in row["args"]]
        done, out = _capture(
            ruby,
            [str(launcher), *args],
            Path(cwd),
            row["files"],
            row["directories"],
            row["environment"],
            row["locked_file"],
        )
        _same(done.returncode, row["exit"])
        _same(done.stdout, bytes.fromhex(row["stdout_hex"]))
        _same(done.stderr, bind(row["stderr_template"], bindings).encode())
        _same(out["out.json"]["bytes_hex"], row["out_hex"])
    validate(value)
    _write_json(output, value)
    return value


def main():
    """候補exeを使わず、全rawを含む本家参照を新規workで再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("ruby", "work", "out"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
