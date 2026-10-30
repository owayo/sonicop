"""実本家だけで49 glob API条件と14 CLI条件を再採取し、全件一致後に保存する。"""

import argparse
import contextlib
import hashlib
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from record_cache_parallel_preflight import _fresh, _same, _write_json
from record_config_cop_namespaces import LOADER
from record_target_ruby_environment import _capture, bind
from record_target_ruby_validation import prepare

FIXTURES = Path(__file__).resolve().parents[2] / "tests/fixtures"
HASHES = {
    "api": "6d38978f5172e9a042565d3b9cea66243f59e7d4534989db97a6366bdfbb1897",
    "cli": "48d4fd59efea13f77bb0a1f8946ff562477efb16e467246cf9a42a1486e8bd02",
}
API = (
    LOADER
    + r"""
require 'json'; require 'digest'
rows = JSON.parse(STDIN.read).map do |p|
  {pattern: p, glob: RuboCop::PathUtil.glob?(p),
   expanded: RuboCop::PathUtil.glob?(p) ? Dir.glob(p) : [p]}
end
root = Gem.loaded_specs.fetch('rubocop').full_gem_path
sources = %w[lib/rubocop/path_util.rb lib/rubocop/config_loader_resolver.rb].to_h do |p|
  [p, Digest::SHA256.file(File.join(root,p)).hexdigest]
end
print JSON.generate(runtime: {ruby: RUBY_VERSION, platform: RUBY_PLATFORM},
  source_sha256: sources, rows: rows)
"""
)
CLI = LOADER + "\nexit RuboCop::CLI.new.run(ARGV)\n"


def validate(value):
    """入力、出典、順序、全rawと比較範囲の削減を拒否する。

    Raises:
        ValueError: 実測した固定参照と異なる場合。

    """
    _same(set(value), set(HASHES))
    for key, digest in HASHES.items():
        actual = hashlib.sha256(
            json.dumps(
                value[key], sort_keys=True, ensure_ascii=False, separators=(",", ":")
            ).encode()
        ).hexdigest()
        if actual != digest:
            raise ValueError("固定参照の入力・型・raw・scopeが変わりました")


def template():
    """候補exeに依存しない二つの固定参照を読む。

    Returns:
        dict: APIとCLIの全入力及び実出力。

    """
    value = {
        key: json.loads((FIXTURES / f"inherit_glob_{key}.json").read_bytes()) for key in HASHES
    }
    validate(value)
    return value


@contextlib.contextmanager
def server(row):
    """一件のloopback HTTPを実行し、要求順を保持して必ず終了する。

    Yields:
        tuple: URLと観測中の要求list。

    """
    requests = []

    class Handler(BaseHTTPRequestHandler):
        """固定した公開設定だけを返す。"""

        def do_GET(self):
            """未定義routeを成功に数えない。"""
            requests.append(self.path)
            text = row["routes"].get(self.path)
            body = b"" if text is None else text.encode()
            self.send_response(404 if text is None else 200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            """HTTPの内部logを参照streamへ混ぜない。"""

    http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=http.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{http.server_port}", requests
    finally:
        http.shutdown()
        http.server_close()
        thread.join()


def bindings(root, url, row):
    """URLと起動位置及びremote出典のMD5だけを束縛する。

    Returns:
        dict: rawへ可逆変換できる動的値。

    """
    value = {"BASE": url, "CWD": str(root), "CWD_FWD": root.as_posix()}
    for i, path in enumerate(sorted(row["routes"])):
        value[f"HASH_{i}"] = hashlib.md5((url + path).encode()).hexdigest()
    return value


def collect(ruby, work, output):
    """17本の実Rubyを直列実行し、全件照合した後だけexclusive保存する。

    Returns:
        dict: 49 API条件と14 CLI条件を再検証した参照。

    Raises:
        ValueError: entry、OS、原本、要求、runtime又は全rawが一致しない場合。

    """
    _fresh(work, output)
    if os.name != "nt":
        raise ValueError("固定Windowsの物理stream専用です")
    value = template()
    work.mkdir(parents=True)
    work = work.resolve()
    api = value["api"]
    for i, group in enumerate(api["groups"]):
        root = work / f"api-{i}"
        prepare(root, group["files"])
        done, before, after = _capture(
            ruby,
            ["-e", API],
            root,
            group["files"],
            {},
            json.dumps([row["pattern"] for row in group["rows"]]).encode(),
        )
        _same(done.returncode, 0)
        _same(done.stderr, b"")
        _same(before, {})
        _same(after, {})
        _same(
            json.loads(done.stdout),
            {
                "runtime": api["runtime"],
                "source_sha256": api["source_sha256"],
                "rows": group["rows"],
            },
        )
    for row in value["cli"]["cases"]:
        root = work / row["id"]
        with server(row) as (url, requests):
            bound = bindings(root, url, row)
            files = {name: bind(body, bound) for name, body in row["files"].items()}
            prepare(root, files)
            done, before, after = _capture(
                ruby,
                ["-e", CLI, "--", *row["args"]],
                root,
                files,
                {"NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost"},
            )
        _same(before, {})
        _same(after, {})
        _same(done.returncode, row["exit"])
        _same(done.stdout, bytes.fromhex(row["stdout_hex"]))
        _same(done.stderr, bind(row["stderr_template"], bound).encode())
        _same(requests, row["requests"])
    validate(value)
    _write_json(output, value)
    return value


def main():
    """新規のprivate保存先だけを受け付ける。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ruby", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    collect(args.ruby, args.work, args.out)


if __name__ == "__main__":
    main()
