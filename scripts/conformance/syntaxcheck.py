#!/usr/bin/env python3
r"""`-A` が生んだファイルを `ruby -c` に通し、どちら側が壊したかで仕分ける。

  syntaxcheck.py <コーパス名>
    fullfmt.sh が残した ff-up-<name> / ff-port-<name> を、原本と突き合わせて検査する

**バイト一致は「本家と同じ」の証拠であって「正しい」の証拠ではない。**
両者が同じように壊れる入力があると、`-A` のバイト比較という一番強い保証が素通りする。
実例 (2026-08-16 に 1 日で 2 件):

    action_mailbox.rb   本家だけ壊れる  Lint/AmbiguousRegexpLiteral が
                                        `routing (/re/ => :sym)` を出す (Ruby に無い形)
    行継続 2 連          両方壊れる      本家の final_pos がループでなく 1 回きりなので、
                                        忠実に写すと壊れ方まで写る

**どちらも偶然見つかった。** 前者は実装して 13 通り一致まで作り込んでから、後者は網羅の
つもりで書いた 4 通り目で。系統的に見る手が無かったので、これを置く。

仕分け:

    移植版だけ構文エラー  こちらのバグ。最優先
    本家だけ構文エラー    カテゴリ ③ (合わせるべきでない差) の候補
    両方 構文エラー       本家のバグを忠実に写している。判断が要る
    両方 OK               現状どおり

**入力そのものが壊れているファイルは除く。** 原本が構文エラーなら、出力が構文エラーでも
`-A` のせいではない。

**これは構文の検査であって意味の検査ではない。** 「両方 OK」でも意味は変わりうる
(`# coding: ISO-8859-15` のファイルで `€` を `\\xa4` へ潰した件は、両方とも `ruby -c` を通る)。

環境変数:
  CORPUS_ROOT  コーパスの置き場   (既定: $HOME/tmp/source)
  OUT_DIR      fullfmt.sh の出力先 (既定: バイナリのパスから導いた既定値を探す)
"""

import os
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

ENV = dict(os.environ)
for _dir in ("/opt/homebrew/lib/ruby/gems/4.0.0/bin", "/opt/homebrew/opt/ruby/bin"):
    if os.path.isdir(_dir):
        ENV["PATH"] = _dir + ":" + ENV["PATH"]

CORPUS_ROOT = os.environ.get("CORPUS_ROOT", os.path.expanduser("~/tmp/source"))


def say(message):
    """進捗と結果を stderr へ。"""
    print(message, file=sys.stderr, flush=True)


def out_dir():
    """`fullfmt.sh` が使った出力先を、**同じ導き方で**求める。

    `fullfmt.sh` は `SONICOP` のパスを slug にして出力先を分けている。同じ規則で導くこと。
    **「いちばん新しいディレクトリ」で探してはいけない** — 他の担当が同時に測っていると
    その人の出力先を掴み、**自分の変更を他人のバイナリの出力と比べる**ことになる。
    (最初にそう書いて、実際に別の担当のディレクトリを掴んだ)

    Returns:
        出力先のパス。`SONICOP` が無ければ終了する。

    """
    if os.environ.get("OUT_DIR"):
        return os.environ["OUT_DIR"]
    binary = os.environ.get("SONICOP")
    if not binary:
        say("SONICOP か OUT_DIR を指定すること (fullfmt.sh と同じ出力先を導くため)。")
        raise SystemExit(2)
    slug = re.sub(r"-+", "-", re.sub(r"[^A-Za-z0-9]", "-", os.path.abspath(binary)))
    return os.path.join(os.environ.get("TMPDIR", "/tmp"), "rubocop-conformance", slug.strip("-"))


def parses(path):
    """`ruby -c` が通るか。

    Returns:
        通れば True。

    """
    done = subprocess.run(
        ["ruby", "-c", path], capture_output=True, text=True, env=ENV, check=False
    )
    return done.returncode == 0


def classify(source, upstream, ported, relative):
    """1 ファイルを 4 つのどれかに仕分ける。原本が壊れていれば None。

    Returns:
        仕分け名。検査の対象外なら None。

    """
    origin = os.path.join(source, relative)
    if not parses(origin):
        return None
    up, port = (
        parses(os.path.join(upstream, relative)),
        parses(os.path.join(ported, relative)),
    )
    if up and port:
        return None
    if up and not port:
        return "移植版だけ壊れた"
    if port and not up:
        return "本家だけ壊れた"
    return "両方壊れた"


def changed(source, ported, upstream):
    """`-A` で中身が変わった `.rb` の相対パス。どちらか一方でも変わっていれば対象。

    Returns:
        相対パスのリスト。

    """
    found = []
    for dirpath, dirnames, filenames in os.walk(source):
        dirnames[:] = [name for name in dirnames if name != ".git"]
        for name in filenames:
            if not name.endswith(".rb"):
                continue
            relative = os.path.relpath(os.path.join(dirpath, name), source)
            try:
                with open(os.path.join(source, relative), "rb") as handle:
                    origin = handle.read()
                with open(os.path.join(ported, relative), "rb") as handle:
                    port = handle.read()
                with open(os.path.join(upstream, relative), "rb") as handle:
                    up = handle.read()
            except OSError as error:
                # 消えた出力を検査対象から落とすと、安全な補正に見えてしまう。
                raise RuntimeError(f"構文検査の入力が読めない: {relative}") from error
            if origin != port or origin != up:
                found.append(relative)
    return found


def main():
    """コーパス名を受け取り、`-A` の出力を仕分けて報告する。"""
    name = sys.argv[1]
    source = os.path.join(CORPUS_ROOT, name)
    base = out_dir()
    upstream = os.path.join(base, f"ff-up-{name}")
    ported = os.path.join(base, f"ff-port-{name}")
    for path in (source, upstream, ported):
        if not os.path.isdir(path):
            say(f"{path} が無い。先に fullfmt.sh を回すこと。")
            raise SystemExit(2)

    targets = changed(source, ported, upstream)
    say(f"{name}: -A で変わった .rb は {len(targets)} 件。ruby -c にかける")

    buckets = {}
    with ThreadPoolExecutor(max_workers=8) as pool:
        verdicts = pool.map(
            lambda relative: (relative, classify(source, upstream, ported, relative)),
            targets,
        )
        for relative, verdict in verdicts:
            if verdict:
                buckets.setdefault(verdict, []).append(relative)

    if not buckets:
        say("  すべて ruby -c を通る (原本が壊れているものは対象外)")
        return
    for verdict in ("移植版だけ壊れた", "本家だけ壊れた", "両方壊れた"):
        items = buckets.get(verdict, [])
        if not items:
            continue
        say(f"  ★ {verdict}: {len(items)} 件")
        for relative in items[:20]:
            say(f"      {relative}")
    raise SystemExit(1)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as error:
        say(str(error))
        raise SystemExit(2) from error
