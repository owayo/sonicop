"""実RuboCopからremote出典の15 CLI条件と12 API行を再採取する。"""

import argparse
import copy
import hashlib
import json
import os
import stat
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from record_cache_parallel_preflight import (
    _fresh,
    _inventory,
    _same,
    _write_json,
    child_environment,
)
from record_config_cop_namespaces import LOADER

FIXTURE = Path(__file__).resolve().parents[2] / "tests/fixtures/remote_config_origins.json"
DATA_SHA256 = "a00ea2fe62eed4cd1171fbbc7a4759f6647fdc500e6a39e5ab16a865069e7bef"
API = (
    LOADER
    + """
require 'json'
require 'digest'
spec=Gem.loaded_specs.fetch('rubocop')
rows=JSON.parse(ARGV.fetch(0)).map do |url|
  remote=RuboCop::RemoteConfig.new(url,'cache/rubocop_cache')
  {url:url, uri:remote.uri.to_s, path:remote.uri.path,
   name:remote.send(:cache_name_from_uri), cache_path:remote.send(:cache_path)}
end
puts JSON.generate(runtime:{ruby:RUBY_VERSION,platform:RUBY_PLATFORM,
 gems:%w[parser prism rubocop-ast rubocop rainbow].to_h {|n|
   [n,Gem.loaded_specs.fetch(n).version.to_s]}},
 source_sha256:%w[lib/rubocop/remote_config.rb lib/rubocop/cache_config.rb
   lib/rubocop/config_loader.rb lib/rubocop/cli.rb].to_h {|n|
   [n,Digest::SHA256.file(File.join(spec.full_gem_path,n)).hexdigest]},rows:rows)
"""
)


def template():
    """元12条件を保持し、追加3条件と昇格した3条件を保持した固定参照を読む。

    Returns:
        dict: 全型・全raw・出典を固定した独立参照。

    """
    value = json.loads(FIXTURE.read_bytes())
    validate(value)
    return value


def validate(value):
    """参照の入力・全raw・型・比較範囲の変更を拒否する。

    Raises:
        ValueError: 固定matrixとcanonical hashが一致しない場合。

    """
    digest = hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    ).hexdigest()
    if digest != DATA_SHA256:
        raise ValueError("固定参照の入力・型・raw・scopeが変わりました")
    _same(value["schema"], 1)
    _same(len(value["cases"]), 15)
    _same(len(value["blocked_cases"]), 0)
    _same(value["coverage"]["blocked_by_issue"], None)
    for row in value["cases"] + value["blocked_cases"]:
        _same(row["source_bytes_mtime_mode_preserved"], True)
        _same(row["exit"], 2 if row["id"] == "ambiguous" else 1)
        stdout = bytes.fromhex(row["stdout_hex"])
        if row["id"] == "ambiguous":
            _same(stdout, b"")
        else:
            report = json.loads(stdout)
            _same(report["metadata"]["ruby_engine"], "ruby")
            _same(report["metadata"]["rubocop_version"], "1.89.0")
            _same(
                report["summary"],
                {"offense_count": 1, "target_file_count": 1, "inspected_file_count": 1},
            )
            _same(len(report["files"]), 1)
            _same(report["files"][0]["path"], "a.rb")
            _same(len(report["files"][0]["offenses"]), 1)


def bind(text, bindings):
    """動的root・URL・MD5だけを再束縛する。

    Returns:
        str: その他の文字と改行を保持した実入力。

    """
    for key, value in bindings.items():
        text = text.replace(f"<{key}>", value)
    return text


def _environment(extra):
    result = child_environment({"TERM": "dumb", "CLICOLOR_FORCE": "0"})
    for key in list(result):
        if key.upper() == "RUBOCOP_CACHE_ROOT":
            result.pop(key)
    result.update(extra)
    return result


def _reparse(info):
    return stat.S_ISLNK(info.st_mode) or bool(getattr(info, "st_file_attributes", 0) & 0x400)


def _marker(root):
    path = root / "loaded_marker"
    if not os.path.lexists(path):
        return None
    info = path.lstat()
    if _reparse(info) or not stat.S_ISREG(info.st_mode):
        raise ValueError("markerは通常fileではありません")
    return _inventory(root, ["loaded_marker"])["loaded_marker"]


def _cache_files(root):
    result = {}
    pending = [root]
    while pending:
        directory = pending.pop()
        with os.scandir(directory) as entries:
            for entry in entries:
                info = entry.stat(follow_symlinks=False)
                if _reparse(info):
                    raise ValueError("採取中に未知のlink/reparse entryが作成されました")
                if stat.S_ISDIR(info.st_mode):
                    pending.append(Path(entry.path))
                elif entry.name.endswith(".yml") and "-" in entry.name:
                    name = str(Path(entry.path).relative_to(root))
                    result[name] = _inventory(root, [name])[name]
    return result


def _capture(ruby, args, root, files, environment, expected_exit):
    before = _inventory(root, files)
    if _marker(root) is not None:
        raise ValueError("marker entryは既に存在します")
    try:
        done = subprocess.run(
            [str(ruby), *args],
            cwd=root,
            env=_environment(environment),
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
    cache = _cache_files(root)
    _write_json(
        root / "private-run0.json",
        {
            **raw,
            "before": before,
            "after": after,
            "marker": marker,
            "cache_files": cache,
        },
    )
    _same(before, after)
    _same(done.returncode, expected_exit)
    return done, marker, cache


class _Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        """固定routeと実request順を記録し、未定義の取得を404へ分ける。"""
        self.server.observed.append(self.path)
        body = self.server.routes.get(self.path)
        encoded = b"" if body is None else body.encode()
        self.send_response(404 if body is None else 200)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, *args):
        """protocolの記録はobservedへ集約し、stderrへ混ぜない。"""


def _prepare(root, files):
    if os.path.lexists(root):
        raise ValueError("case entryは既に存在します")
    root.mkdir()
    for name, data in files.items():
        with (root / name).open("xb") as stream:
            stream.write(data.encode())


def _record_case(ruby, launcher, work, row):
    root = work / row["id"]
    if os.path.lexists(root):
        raise ValueError("case entryは既に存在します")
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
    server.daemon_threads = True
    base = f"http://127.0.0.1:{server.server_port}"
    bindings = {"BASE": base, "CWD": str(root), "CWD_FWD": root.as_posix()}
    for index, path in enumerate(sorted(row["routes"])):
        bindings[f"HASH_{index}"] = hashlib.md5((base + path).encode()).hexdigest()
    files = {key: bind(value, bindings) for key, value in row["files"].items()}
    server.routes = {key: bind(value, bindings) for key, value in row["routes"].items()}
    server.observed = []
    try:
        _prepare(root, files)
    except Exception:
        server.server_close()
        raise
    environment = {key: bind(value, bindings) for key, value in row["env"].items()}
    environment.update(
        {
            "XDG_CONFIG_HOME": str(root / "empty-config"),
            "NO_PROXY": "127.0.0.1,localhost",
            "no_proxy": "127.0.0.1,localhost",
        }
    )
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        done, marker, cache = _capture(
            ruby, [str(launcher), *row["args"]], root, files, environment, row["exit"]
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    _same(server.observed, row["requests"])
    _same(done.stdout, bytes.fromhex(row["stdout_hex"]))
    _same(done.stderr, bind(row["stderr_template"], bindings).encode())
    _same(None if marker is None else marker["bytes_hex"], row["marker_hex"])
    expected_cache = {
        bind(entry["path_template"], bindings): bind(entry["content_template"], bindings)
        .encode()
        .hex()
        for entry in row["reference_cache_files"]
    }
    _same({key: value["bytes_hex"] for key, value in cache.items()}, expected_cache)


def collect(ruby, work, output):
    """全APIと15実CLIの完了後だけ固定参照をexclusive保存する。

    Returns:
        dict: 元12行と追加3行を保持した全参照。

    Raises:
        ValueError: 版・出典・実raw・原本属性・取得順・artifact・保存先が異なる場合。

    """
    _fresh(work, output)
    if os.name != "nt":
        raise ValueError("固定Windows参照専用。非Windows実走へ投影しません")
    value = template()
    work.mkdir(parents=True)
    work = work.resolve()
    root = work / "api-runtime"
    _prepare(root, {})
    done, marker, cache = _capture(
        ruby, ["-e", API, json.dumps([row["url"] for row in value["api"]])], root, {}, {}, 0
    )
    _same(done.stderr, b"")
    _same(marker, None)
    _same(cache, {})
    observed = json.loads(done.stdout)
    bindings = {"CWD_FWD": root.as_posix(), "DRIVE": root.drive}
    expected = []
    for row in value["api"]:
        item = {
            key: copy.deepcopy(data) for key, data in row.items() if key != "cache_path_template"
        }
        item["cache_path"] = bind(row["cache_path_template"], bindings)
        expected.append(item)
    _same(
        observed,
        {"runtime": value["runtime"], "source_sha256": value["source_sha256"], "rows": expected},
    )
    launcher = work / "reference.rb"
    with launcher.open("x", encoding="utf8", newline="\n") as stream:
        stream.write(LOADER + '\nload Gem.bin_path("rubocop", "rubocop", "1.89.0")\n')
    for row in value["cases"] + value["blocked_cases"]:
        _record_case(ruby, launcher, work, row)
    validate(value)
    _write_json(output, value)
    return value


def main():
    """候補exeを受け取らず、全15行を再採取する。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
