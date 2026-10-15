#!/usr/bin/env python3
r"""死守ラインの差分を `known_divergences.yml` と突き合わせ、「既知 N / 新規 M」を出す。

`fullfmt.sh` は割れたファイル数しか出さない。**数字だけを見た人は「割れている」と読む。**
意図して割れている事例があるので、そのまま報告すると毎回説明が要る。

**除外はしない。**除外すると、そのファイルが**別の理由で**変わったときに静かになる。
両者の出力のハッシュを manifest に固定しておき、**差分そのものを期待値として監視する**。

    既知 N / 新規 0                 通過
    新規 1 以上                     ★ 退行
    既知だがハッシュが動いた         ★ 同じファイルが別の理由で割れた。これも止める

使い方 (`fullfmt.sh` の後に):

    python3 ff_gate.py <コーパス名> <OUT_DIR> [manifest] [safe|unsafe]

`safe` は -a、`unsafe` は -A の出力を調べる。省略時は `unsafe`。

`OUT_DIR` は `fullfmt.sh` が `ff-up-<corpus>/` と `ff-port-<corpus>/` を置いた場所。
manifest の既定は `<git のトップ>/tests/conformance/known_divergences.yml`。
"""

import hashlib
import pathlib
import subprocess
import sys

DIGEST_LENGTH = 16


def say(message):
    print(message, file=sys.stderr, flush=True)


def digest(path):
    """manifest に載せるのと同じ長さの短縮 sha256。"""
    return hashlib.sha256(path.read_bytes()).hexdigest()[:DIGEST_LENGTH]


def manifest_entries(manifest, corpus, mode):
    """`autocorrect_divergences` のうち、このコーパスの分。

    **YAML の parser を持ち込まない。**このファイルは他の担当も編集するので、依存を
    増やすと編集できる人が減る。項目は `- id:` で始まる平坦な写像だけなので、
    その形に限って読む。**読めない形が来たら黙って飛ばさず、そう言って止める。**
    """
    lines = manifest.read_text(encoding="utf-8").splitlines()
    try:
        start = lines.index("autocorrect_divergences:")
    except ValueError:
        return []
    entries, current = [], None
    for line in lines[start + 1 :]:
        if line and not line.startswith((" ", "-", "#")):
            break
        stripped = line.strip()
        if stripped.startswith("- id:"):
            current = {"id": stripped.split(":", 1)[1].strip()}
            entries.append(current)
        elif current is not None and ":" in stripped and not stripped.startswith("#"):
            key, value = stripped.split(":", 1)
            current.setdefault(key.strip(), value.strip())
    for entry in entries:
        missing = {"corpus", "path", "rubocop_sha256", "sonicop_sha256"} - set(entry)
        if missing:
            say(f"!! manifest の項目 {entry.get('id')} に {sorted(missing)} がありません")
            raise SystemExit(2)
    return [
        entry
        for entry in entries
        if entry["corpus"] == corpus and entry.get("mode", "unsafe") == mode
    ]


def differing(out_dir, corpus):
    """`ff-diff-<corpus>.txt` に載っている相対パス。"""
    listing = out_dir / f"ff-diff-{corpus}.txt"
    if not listing.is_file():
        return []
    # **絶対パスの接頭辞で照合しない。**`pathlib` は `T//rubocop` を `T/rubocop` に
    # 正規化するが、`diff` の出力は渡された字面 (`T//rubocop`) をそのまま書く。
    # 一致しなくなり、**差分 3 件が「0 件」に化けた。**セグメントで拾う。
    marker = f"/ff-up-{corpus}/"
    paths = []
    for line in listing.read_text(encoding="utf-8", errors="replace").splitlines():
        # `diff -r` は片側にしか無いファイルを `diff: <path>: No such file` と書く。
        # 中身が違うのは `Files <a> and <b> differ` の行だけ。
        if not line.endswith(" differ"):
            continue
        for token in line.split():
            index = token.find(marker)
            if index != -1:
                paths.append(token[index + len(marker) :])
                break
    return paths


def main(argv):
    if len(argv) < 2:
        say(__doc__)
        return 2
    corpus, out_dir = argv[0], pathlib.Path(argv[1])
    if len(argv) > 2:
        manifest = pathlib.Path(argv[2])
    else:
        top = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.strip()
        manifest = pathlib.Path(top) / "tests/conformance/known_divergences.yml"
    mode = argv[3] if len(argv) > 3 else "unsafe"
    if mode not in ("safe", "unsafe"):
        say(f"!! 不明な補正モード: {mode}")
        return 2
    if not manifest.is_file():
        say(f"!! manifest がありません: {manifest}")
        return 2

    known = {entry["path"]: entry for entry in manifest_entries(manifest, corpus, mode)}
    seen = differing(out_dir, corpus)

    fresh, moved, matched = [], [], []
    for path in seen:
        entry = known.get(path)
        if entry is None:
            fresh.append(path)
            continue
        up = out_dir / f"ff-up-{corpus}" / path
        port = out_dir / f"ff-port-{corpus}" / path
        if not up.is_file() or not port.is_file():
            say(f"!! 出力が見つかりません: {path}")
            return 2
        if digest(up) != entry["rubocop_sha256"] or digest(port) != entry["sonicop_sha256"]:
            moved.append((path, entry["id"], digest(up), digest(port)))
        else:
            matched.append(path)

    # **登録されているのに割れなかったものも出す。**直ったなら manifest から消す番。
    healed = [path for path in known if path not in seen]

    print(f"  既知 {len(matched)} / 新規 {len(fresh)}", end="")
    if moved:
        print(f" / ★ 既知だが中身が変わった {len(moved)}", end="")
    if healed:
        print(f" / 割れなくなった {len(healed)}", end="")
    print()

    for path in fresh:
        print(f"  ★ 新規: {path}")
    for path, name, up, port in moved:
        print(f"  ★ 中身が変わった: {path} ({name})")
        print(f"      manifest 本家={known[path]['rubocop_sha256']} 実測={up}")
        print(f"      manifest 移植={known[path]['sonicop_sha256']} 実測={port}")
    for path in healed:
        print(f"  割れなくなった (manifest から消すこと): {path}")

    return 1 if fresh or moved or healed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
