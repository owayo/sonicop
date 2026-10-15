#!/usr/bin/env bash
# 公開コーパスの補正互換性と安全性を、独立した複製で検査する。
set -euo pipefail

corpus="${1:?コーパス名が必要}"
source_root="${GATE_CORPUS_ROOT:?コーパスの親ディレクトリが必要}"
oracle_root="${GATE_ORACLE_ROOT:?仕様版を記録したディレクトリが必要}"
output_root="${OUT_DIR:?出力先が必要}"
binary="${SONICOP:?固定したバイナリが必要}"
reference="${RUBOCOP:-rubocop}"
script_root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
project_root=$(git rev-parse --show-toplevel)
source_tree="$source_root/$corpus"
manifest="$project_root/tests/conformance/known_divergences.yml"
mkdir -p "$output_root"

# 対象集合を比較してから補正する。検査件数が同じだけでは別の入力を見ていても分からない。
(cd "$source_tree" && "$reference" --force-default-config --cache false -L .) \
  > "$output_root/reference-paths.txt" 2> "$output_root/reference-paths.err"
(cd "$source_tree" && "$binary" --force-default-config --cache false -L .) \
  > "$output_root/candidate-paths.txt" 2> "$output_root/candidate-paths.err"
LC_ALL=C sort "$output_root/reference-paths.txt" > "$output_root/reference-paths-sorted.txt"
LC_ALL=C sort "$output_root/candidate-paths.txt" > "$output_root/candidate-paths-sorted.txt"
test -s "$output_root/reference-paths-sorted.txt"
cmp "$output_root/reference-paths-sorted.txt" "$output_root/candidate-paths-sorted.txt"

for mode in safe unsafe; do
  flag=-a
  [ "$mode" = safe ] || flag=-A
  out="$output_root/$mode"
  mkdir -p "$out"
  result=0
  CORPUS_ROOT="$oracle_root" OUT_DIR="$out" MODE="$flag" SONICOP="$binary" RUBOCOP="$reference" MANIFEST="$manifest" \
    bash "$script_root/fullfmt.sh" "$source_tree" > "$out/comparison.log" 2>&1 || result=$?
  echo "$result" > "$out/comparison.exit"
  cat "$out/comparison.log"
  [ "$result" -le 1 ] || exit 2
  test -f "$out/ff-diff-$corpus.txt"
  # ff_gate は内容の差を扱う。片側だけのファイルも、その手前で明示的に拒否する。
  if grep -q '^Only in ' "$out/ff-diff-$corpus.txt"; then
    echo '片側だけのファイルがある' >&2
    exit 1
  fi
  # 標準出力にはソース行も載るため、同じ文言を持つ正常な入力をエラーと誤認しない。
  if grep -Eq 'An error occurred while|[0-9]+ errors? occurred' "$out/ff-rc-$corpus.err"; then
    echo '本家 cop の内部エラーで検証が不完全' >&2
    exit 2
  fi
  python3 "$script_root/ff_gate.py" "$corpus" "$out" "$manifest" "$mode" \
    > "$out/known-differences.log" 2>&1
  cat "$out/known-differences.log"
done

# 元が有効な .rb だけを評価し、両者が同じ構文エラーを作る場合も失敗にする。
result=0
CORPUS_ROOT="$source_root" OUT_DIR="$output_root/unsafe" \
  python3 "$script_root/syntaxcheck.py" "$corpus" > "$output_root/syntax.log" 2>&1 || result=$?
echo "$result" > "$output_root/syntax.exit"
cat "$output_root/syntax.log"
[ "$result" -le 1 ] || exit 2
if [ "$result" = 1 ] && ! grep -q '本家だけ壊れた' "$output_root/syntax.log"; then
  echo '構文検査が正常な分類結果を返していない' >&2
  exit 2
fi
if grep -Eq '移植版だけ壊れた|両方壊れた' "$output_root/syntax.log"; then
  exit 1
fi

# 二度目の -A が同じツリーを保つことを、対象外のファイルも含むハッシュで確かめる。
ported="$output_root/unsafe/ff-port-$corpus"
hash_tree() {
  python3 - "$1" <<'PY'
import hashlib, json, os, pathlib, sys
root = pathlib.Path(sys.argv[1])
result = {}
for directory, dirs, files in os.walk(root, followlinks=False):
    dirs[:] = [name for name in dirs if name != '.git']
    for name in dirs + files:
        path = pathlib.Path(directory) / name
        relative = str(path.relative_to(root))
        if path.is_symlink():
            result[relative] = ['link', os.readlink(path)]
        elif path.is_file():
            result[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
print(json.dumps(result, sort_keys=True))
PY
}
hash_tree "$ported" > "$output_root/before-idempotence.json"
result=0
(cd "$ported" && "$binary" --force-default-config --cache false -A) \
  > "$output_root/idempotence.log" 2> "$output_root/idempotence.err" || result=$?
echo "$result" > "$output_root/idempotence.exit"
if [ "$result" -gt 1 ]; then
  [ "$result" = 2 ] || exit 2
  # 原本のまま保護されたファイルは二度目も同じ拒否を返す。終了2と理由を残して照合する。
  python3 "$script_root/rollbacks.py" "$output_root/idempotence.err" "$ported" "$manifest" "$corpus" unsafe \
    > "$output_root/idempotence-rollbacks.json" || exit 2
fi
hash_tree "$ported" > "$output_root/after-idempotence.json"
cmp "$output_root/before-idempotence.json" "$output_root/after-idempotence.json"
echo '安全性: 移植版だけの構文破壊 0 / 両者の構文破壊 0 / 二度目の変更 0'
