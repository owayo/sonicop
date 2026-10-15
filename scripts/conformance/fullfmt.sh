#!/usr/bin/env bash
# 全 cop の autocorrect の結果をバイト単位で突き合わせる (死守ラインの確認)。
#
#   fullfmt.sh <コーパス名 | コーパスのパス>
#
# lint の一致だけでは足りない。**同じソースを両者で修正した結果がバイト単位で一致すること**
# が置き換えの条件で、ここが割れると「移植版で直したコードが本家で通らない」状態になる。
#
# コーパスは共有資源なので必ず複製の上で回す。原本を修正すると、並行して走っている検出比較や
# 速度計測が修正後のツリーを見て、双方の結果が狂う。
#
# 環境変数:
#   SONICOP      移植版バイナリ     (既定: <git のトップ>/target/release/sonicop)
#   RUBOCOP      本家の起動コマンド (既定: rubocop)
#   CORPUS_ROOT  コーパスの置き場所 (既定: $HOME/tmp/conf-pinned = 固定した木)
#                動く木で測るときだけ $HOME/tmp/source を明示する
#   OUT_DIR      作業用の出力先     (既定: 移植版バイナリごとに分けた作業ディレクトリ)
#
# **出力先は同時に走る他の実行と分けること。** ここは JSON だけでなくコーパスの複製そのものを
# 置く。2 人が同じ OUT_DIR を使うと、片方が本家で修正したツリーをもう片方が自分の出力と
# 比べることになり、差分が消えて「バイト一致」と誤報告する。既定値は SONICOP のパスから
# 導くので、worktree を分けている限り自動的に別ディレクトリになる。
#   MODE         -a か -A          (既定: -A)
#
# exit code: 0=バイト一致 / 1=差分あり / 2=実行エラー
set -uo pipefail

NAME="${1:?コーパス名かパスを指定する}"
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# --- パス解決 (cd する前に済ませる) ---
if [ -n "${SONICOP:-}" ]; then
  SONICOP_BIN="$SONICOP"
else
  _root=$(git rev-parse --show-toplevel 2>/dev/null) || _root="$PWD"
  SONICOP_BIN="$_root/target/release/sonicop"
fi
case "$SONICOP_BIN" in /*) ;; *) SONICOP_BIN="$PWD/$SONICOP_BIN" ;; esac

RUBOCOP="${RUBOCOP:-rubocop}"
# **既定は固定した木**。`~/tmp/source` は追加取得で動くので、既定にすると
# 「死守ラインが動く入力を見張る」ことになり、バイト差分 0 が別の主張に化ける。
# 2026-08-17 まで両スクリプトとも既定が `source` で、pinned は
# **人が CORPUS_ROOT を思い出したときだけ**使われていた (= 規律頼み)。
CORPUS_ROOT="${CORPUS_ROOT:-$HOME/tmp/conf-pinned}"
# 既定の出力先はバイナリのパスから導く。worktree ごとにバイナリが違うので、
# 同時に走っても互いのコーパス複製を踏まない。
_slug=$(printf '%s' "$SONICOP_BIN" | tr -c 'A-Za-z0-9' '-' | sed 's/--*/-/g; s/^-//; s/-$//')
OUT_DIR="${OUT_DIR:-${TMPDIR:-/tmp}/rubocop-conformance/$_slug}"
MODE="${MODE:--A}"

# Homebrew の Ruby を優先する (システム ruby 2.6 では本家が起動しない)。
for _dir in /opt/homebrew/lib/ruby/gems/*/bin /opt/homebrew/opt/ruby/bin; do
  [ -d "$_dir" ] && PATH="$_dir:$PATH"
done
export PATH

# **裸の名前は必ず CORPUS_ROOT の下。パスは必ずパス。** 優先順位を決めるのではなく
# 曖昧さを消す — verify.sh と fullfmt.sh で優先順位が逆になっていて、カレントに
# 同名ディレクトリがあると 2 本が別の木を読んだ (2026-08-17 発見)。
case "$NAME" in
  */*) SRC=$(cd "$NAME" 2>/dev/null && pwd) || { echo "$NAME: そのパスが無い" >&2; exit 2; } ;;
  *)   SRC="$CORPUS_ROOT/$NAME" ;;
esac
LABEL=$(basename "$SRC")
if [ ! -d "$SRC" ]; then
  echo "$NAME: コーパスが無い ($SRC)" >&2
  echo "  固定した木に足すには:" >&2
  echo "    git -C \$HOME/tmp/source/$NAME worktree add --detach $SRC <sha>" >&2
  echo "  ★ 動く木で測りたいなら CORPUS_ROOT=\$HOME/tmp/source と明示すること" >&2
  exit 2
fi
[ -x "$SONICOP_BIN" ] || { echo "移植版バイナリが無い ($SONICOP_BIN)" >&2; exit 2; }

mkdir -p "$OUT_DIR"
WORK="$OUT_DIR/work-$LABEL"
UP="$OUT_DIR/ff-up-$LABEL"
PORT="$OUT_DIR/ff-port-$LABEL"
DIFF="$OUT_DIR/ff-diff-$LABEL.txt"

# 原本には触らない。複製の上で修正させる。
#
# **番人: 複製する前にコーパスが記録の状態かを確かめる。**
# 汚れたツリーを複製すると、両者とも「直すものが無い」で終わってバイト比較は一致する。
# そこで言えるのは「補正済みを壊さない」だけで、「クリーンから同じ結果に至る」は測れない。
# 数字は綺麗に出るので気づく手がかりが無い。理由は guards.py の docstring (5 件目)。
if ! python3 -c "
import sys
sys.path.insert(0, sys.argv[1])
import guards
guards.assert_corpus_is_clean(sys.argv[2])
" "$SCRIPT_DIR" "$SRC"; then
  exit 2
fi

git clone -q --shared --no-hardlinks "$SRC" "$WORK" || exit 2
git -C "$WORK" checkout -q --detach "$(git -C "$SRC" rev-parse HEAD)" || exit 2
rsync -a --delete --exclude='.git' "$SRC/" "$WORK/" || exit 2
snapshot() { rsync -a --delete --exclude='.git/' "$WORK/" "$1/"; }

# **番人: 走らせる本家が、移植元として読んでいるソースと同じ版かを確かめる。**
# 本家の版は PATH で暗黙に決まる。gem 環境が更新されれば誰にも知らされずに別の版が
# 神託になり、出力からは分からない (差は移植版の側に見える)。2026-08-17 に同じ形で
# tree-sitter の生成器の版がずれ、死守ラインが 5 件割れた事故がある。
if ! python3 -c "
import sys
sys.path.insert(0, sys.argv[1])
import guards
guards.assert_upstream_matches_source(sys.argv[2].split())
" "$SCRIPT_DIR" "$RUBOCOP"; then
  exit 2
fi

echo "== $LABEL ($MODE) =="
python3 -c "
import sys
sys.path.insert(0, sys.argv[1])
import guards
print('  移植版バイナリ:', guards.fingerprint(sys.argv[2]))
" "$SCRIPT_DIR" "$SONICOP_BIN"
# **入力そのものを名乗らせる。** 同じ名前のコーパスが別の根に別リビジョンで置いてある
# (`~/tmp/source` と `~/tmp/conf-pinned` は Homebrew が 2,183 対 2,179 ファイル)。
# 根を書かずに前後比較すると、1 cop の分岐しか触っていない変更で数百件動いて見える
# (2026-08-17 に実際に起きた)。**「バイト差分 0」も、入力が違えば別の主張になる。**
echo "  コーパス     : $SRC @ $(git -C "$SRC" rev-parse --short HEAD 2>/dev/null || echo '(git 管理外)')"
# **指紋は「どのバイナリか」に答えるが、「誰の修正が入っているか」には答えない。**
# 2026-08-17 に、この 2 つを別の問いだと気づかないまま 60 分の門を 5 回、古い木で回した。
# 受け取った側が 1 回打てば交差を確かめられるよう、打つべきコマンドを出力に置く。
_built=$(python3 -c "
import sys
sys.path.insert(0, sys.argv[1])
import guards
print(guards.build_commit(sys.argv[2]) or '')
" "$SCRIPT_DIR" "$SONICOP_BIN")
if [ -n "$_built" ]; then
  echo "  作った commit: $_built"
  echo "                 確かめ方: git merge-base --is-ancestor <直したコミット> ${_built%% *}"
else
  # **原因は 3 つある。** 3 つ目は罠 18 の対処 (測定中にビルドが走らないよう固定コピーを作る)
  # と引き換えに起きるので、いちばん踏みやすい。
  echo "  作った commit: (不明。原因は 3 つ)" >&2
  echo "    1  build.rs より前のバイナリ (枝が古い)" >&2
  echo "    2  tarball ビルド (リポジトリが無い)" >&2
  echo "    3  ★ 固定コピーを作るときに .sonicop-build-sha を連れて来なかった" >&2
  echo "       → cp <元の target/release>/.sonicop-build-sha $(dirname "$SONICOP_BIN")/" >&2
fi

# **番人: 両者が同じ Ruby バージョンとしてソースを読むことを確かめる。**
# `--force-default-config` は設定ファイルを無視するが、**TargetRubyVersion の推定は
# 止めない**。本家は cwd から `*.gemspec` / `Gemfile.lock` / `.ruby-version` を探す。
# 版が違えば文法が違い、木が違い、**そこから先は「差分 0」も同じだけ無意味になる。**
# `verify.sh` は以前からこれを通っていたが、`fullfmt.sh` は通っていなかった (2026-08-17 追加)。
if ! python3 -c "
import sys
sys.path.insert(0, sys.argv[1])
import guards
guards.assert_same_ruby_version(
    [*sys.argv[3].split(), '--force-default-config', '--cache', 'false'],
    [sys.argv[2], '--force-default-config'],
    sys.argv[4],
)
" "$SCRIPT_DIR" "$SONICOP_BIN" "$RUBOCOP" "$WORK"; then
  exit 2
fi

# `git reset --hard` は必ず -C で名指しする。裸で打つと cwd 依存で移植版のリポジトリを巻き戻す。
git -C "$WORK" reset --hard -q 2>/dev/null || exit 2
( cd "$WORK" && $RUBOCOP --force-default-config --cache false --no-color "$MODE" ) \
  > "$OUT_DIR/ff-rc-$LABEL.log" 2> "$OUT_DIR/ff-rc-$LABEL.err"
rc_status=$?
echo "$rc_status" > "$OUT_DIR/ff-rc-$LABEL.exit"
echo "  rubocop: $(grep -E 'files inspected' "$OUT_DIR/ff-rc-$LABEL.log" | tail -1)"
snapshot "$UP" || exit 2

git -C "$WORK" reset --hard -q 2>/dev/null || exit 2
( cd "$WORK" && "$SONICOP_BIN" --force-default-config "$MODE" ) \
  > "$OUT_DIR/ff-sc-$LABEL.log" 2> "$OUT_DIR/ff-sc-$LABEL.err"
sc_status=$?
echo "$sc_status" > "$OUT_DIR/ff-sc-$LABEL.exit"
echo "  移植版 : $(grep -E 'files inspected' "$OUT_DIR/ff-sc-$LABEL.log" | tail -1)"
snapshot "$PORT" || exit 2

git -C "$WORK" reset --hard -q 2>/dev/null || exit 2

if [ "$rc_status" -gt 1 ]; then
  echo "  未完了: rubocop exit=$rc_status / sonicop exit=$sc_status" >&2
  exit 2
fi
if [ "$sc_status" -gt 1 ]; then
  [ "$sc_status" = 2 ] || exit 2
  _mode=unsafe
  [ "$MODE" != -a ] || _mode=safe
  _manifest="${MANIFEST:-$(git rev-parse --show-toplevel)/tests/conformance/known_divergences.yml}"
  # 終了2は残す。登録済みパスの保護動作だけを認め、出力ハッシュは ff_gate でも照合する。
  python3 "$SCRIPT_DIR/rollbacks.py" "$OUT_DIR/ff-sc-$LABEL.err" "$WORK" "$_manifest" "$LABEL" "$_mode" \
    > "$OUT_DIR/recorded-rollbacks.json" || exit 2
  echo "  記録済みの書き戻し中止 (exit=$sc_status): $(cat "$OUT_DIR/recorded-rollbacks.json")"
fi

# **番人: どちらかが 1 ファイルも検査していないなら、比較ではなく未測定。**
# 検査 0 なら両者の複製は原本そのままなので `diff -r -q` は差分 0 を返し、
# 「完全一致 (バイト差分 0)」で exit 0 になる。**壊れ方が成功と同じ顔をする。**
# 実際に起きた例 (2026-08-16): fullfmt.sh の実行ビットが落ちていて `permission denied`
# → 0 秒 → 呼び出し側は exit 0 を受け取り「死守ライン 3 本一致」と報告された。
# 検査数を**報告に書く**だけでは足りない (0 でも書けば守れてしまう)。ここで止める。
inspected() { grep -Eo '[0-9]+ files? inspected' "$1" | tail -1 | grep -Eo '^[0-9]+'; }
rc_n=$(inspected "$OUT_DIR/ff-rc-$LABEL.log")
sc_n=$(inspected "$OUT_DIR/ff-sc-$LABEL.log")
for _side in "rubocop:$rc_n:$OUT_DIR/ff-rc-$LABEL.log" "移植版:$sc_n:$OUT_DIR/ff-sc-$LABEL.log"; do
  _name=${_side%%:*}; _rest=${_side#*:}; _n=${_rest%%:*}; _log=${_rest#*:}
  if [ -z "$_n" ] || [ "$_n" -eq 0 ]; then
    echo "  未測定: $_name が 1 ファイルも検査していない (検査数: ${_n:-取得できず})" >&2
    echo "  ログ: $_log" >&2
    exit 2
  fi
done
# 検査数が食い違うなら、両者は別の集合を見ている。差分 0 でも「一致」ではない。
if [ "$rc_n" -ne "$sc_n" ]; then
  echo "  未測定: 検査したファイル数が違う (rubocop $rc_n / 移植版 $sc_n)" >&2
  echo "  同じ集合を見ていないので、バイト比較の結果は意味を持たない" >&2
  exit 2
fi

diff -r -q "$UP" "$PORT" > "$DIFF" 2>&1
# `Directory loop detected` は symlink をたどった diff の警告で、差分ではない
# (Homebrew はディレクトリ symlink を持つので必ず出る)。数から除く。
differing=$(grep -c '^Files .* differ$' "$DIFF")
onlyone=$(grep -c '^Only in ' "$DIFF")
# ★ 数に入れなかった行を黙って捨てない。判定は正しくても、**diff ファイルに中身が
#   あるのに「完全一致」と出る**のは読み手を迷わせる (2026-08-17 に指摘を受けた。
#   mastodon の `public/sw.js` で `diff: ...: No such file or directory` が 3 行入っていた)。
#   捨てた行数を出せば、読み手は「見落としではなく除外」と分かる。
noise=$(grep -vcE '^(Files .* differ$|Only in )' "$DIFF")
if [ "$differing" -eq 0 ] && [ "$onlyone" -eq 0 ]; then
  if [ "$noise" -gt 0 ]; then
    echo "  完全一致 (バイト差分 0)  ※ 差分ではない行 $noise 件を除外: $DIFF"
    echo "     (symlink をたどった警告・片側にしか無い非対象ファイルのエラー等)"
  else
    echo "  完全一致 (バイト差分 0)"
  fi
  exit 0
fi
echo "  差分ファイル数: $differing  (片側のみ存在: $onlyone)"
grep -E '^(Files .* differ|Only in )' "$DIFF" | sed -E "s|$UP/||; s| and .* differ$||" | head -20
echo "  完全なリスト: $DIFF"
echo "  中身を見る: diff -u $UP/<path> $PORT/<path>"
exit 1
