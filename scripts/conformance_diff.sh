#!/usr/bin/env bash
# 本家 RuboCop と Sonicop に同じターゲットを検査させ、offense の差分を出す。
#
# 「移植版が本家と同じ指摘を、同じ位置・同じ severity・同じ文言で出すか」が
# 唯一の受け入れ基準なので、それを機械的に測るためのハーネス。
# 両者の `--format json` を jq で正規化し、
#   - 共有構文エラーファイル内の Lint/Syntax = パーサ診断位置の差
#   - それ以外で移植版だけが出した offense = 誤検出 (false positive)
#   - それ以外で本家だけが出した offense   = 検出漏れ (false negative)
#   - 位置は一致するがメッセージが違う = 文言差分
# に分類する。構文エラーの受理判定は offense 位置とは別にファイル集合で比較する。
set -euo pipefail

# このリポジトリに Gemfile は無いので、既定は bundler 越しではなく素の rubocop。
# bundler 配下で比べたいときは -r "bundle exec rubocop" を明示する。
DEFAULT_REFERENCE_CMD="rubocop"
DEFAULT_CANDIDATE_CMD="./target/release/sonicop"

REFERENCE_CMD="$DEFAULT_REFERENCE_CMD"
CANDIDATE_CMD="$DEFAULT_CANDIDATE_CMD"
REFERENCE_GIVEN=0
CANDIDATE_GIVEN=0
OUT_DIR=""
ONLY_COP=""
QUIET=0
FORCE_DEFAULT_CONFIG=0
EXCLUDE_UNPARSABLE=0
TARGETS=()

die() {
  printf '%s\n' "conformance_diff: $1" >&2
  exit 2
}

usage() {
  cat <<USAGE
使い方: conformance_diff.sh [options] [--] <検査対象パス...>

  -r, --reference CMD       本家の起動コマンド (既定: ${DEFAULT_REFERENCE_CMD})
  -c, --candidate CMD       移植版の起動コマンド (既定: ${DEFAULT_CANDIDATE_CMD})
  -o, --out DIR             成果物の出力先 (既定: mktemp -d)
      --cop NAME            指定 cop / department だけを比較 (--only を両者に付与)
      --force-default-config
                            両者でプロジェクト設定を無視して既定設定を使う
      --exclude-unparsable  どちらかが Lint/Syntax fatal を出したファイルを比較から除外する
                            (どちらかが Lint/Syntax fatal を出したファイルを offense
                             比較から除外する。構文エラーファイル集合の差は別に報告する)
      --quiet               サマリのみ表示
  -h, --help                このヘルプを表示

exit code: 0=完全一致 / 1=差分あり / 2=実行エラー
USAGE
}

# コマンド文字列は "bundle exec rubocop" のように複数語を意図的に分割するが、
# クォートを外すと単語分割と一緒に glob 展開まで効いてしまうので set -f で止める。
split_command() {
  set -f
  # shellcheck disable=SC2086
  set -- $1
  set +f
  printf '%s\n' "$@"
}

require_command() {
  local label="$1" flag="$2" cmd="$3" first=""
  first="$(split_command "$cmd" | head -1)"
  [ -n "$first" ] || die "${label}のコマンドが空です (${flag} で指定してください)"
  command -v "$first" >/dev/null 2>&1 ||
    die "${label}のコマンドが見つかりません: ${first} (${flag} で指定してください)"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    -r | --reference)
      REFERENCE_CMD="${2:-}"
      REFERENCE_GIVEN=1
      shift 2
      ;;
    -c | --candidate)
      CANDIDATE_CMD="${2:-}"
      CANDIDATE_GIVEN=1
      shift 2
      ;;
    -o | --out)
      OUT_DIR="${2:-}"
      shift 2
      ;;
    --cop)
      ONLY_COP="${2:-}"
      shift 2
      ;;
    --quiet)
      QUIET=1
      shift
      ;;
    --force-default-config)
      FORCE_DEFAULT_CONFIG=1
      shift
      ;;
    --exclude-unparsable)
      EXCLUDE_UNPARSABLE=1
      shift
      ;;
    --)
      shift
      while [ "$#" -gt 0 ]; do
        TARGETS+=("$1")
        shift
      done
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      TARGETS+=("$1")
      shift
      ;;
  esac
done

command -v jq >/dev/null 2>&1 || die "jq が必要です"
if [ "${#TARGETS[@]}" -eq 0 ]; then
  usage >&2
  die "検査対象パスを指定してください"
fi

# 既定値のまま動かせない環境では、実行してから謎の exit 2 になるより先に理由を出す。
if [ "$REFERENCE_GIVEN" -eq 0 ] && ! command -v "$DEFAULT_REFERENCE_CMD" >/dev/null 2>&1; then
  die "本家 ${DEFAULT_REFERENCE_CMD} が見つかりません。'gem install rubocop' するか、-r で起動コマンドを指定してください (例: -r 'bundle exec rubocop')"
fi
if [ "$CANDIDATE_GIVEN" -eq 0 ] && [ ! -x "$DEFAULT_CANDIDATE_CMD" ]; then
  die "既定の候補コマンド ${DEFAULT_CANDIDATE_CMD} がありません。'make release' でビルドするか -c で指定してください"
fi
require_command 参照 '-r/--reference' "$REFERENCE_CMD"
require_command 候補 '-c/--candidate' "$CANDIDATE_CMD"

if [ -z "$OUT_DIR" ]; then
  OUT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rubocop-conformance.XXXXXX")"
fi
mkdir -p "$OUT_DIR"

# 共通フラグ: キャッシュと色は差分のノイズになるので必ず切る。
COMMON_FLAGS=(--format json --no-color --cache false)
if [ "$FORCE_DEFAULT_CONFIG" -eq 1 ]; then
  COMMON_FLAGS+=(--force-default-config)
fi
if [ -n "$ONLY_COP" ]; then
  COMMON_FLAGS+=(--only "$ONLY_COP")
fi

# offense があると exit 1 になるのは正常系。2 以上だけを異常として扱う。
run_linter() {
  local label="$1" cmd="$2" dest="$3" status=0
  local words=()
  while IFS= read -r word; do
    words+=("$word")
  done <<<"$(split_command "$cmd")"

  "${words[@]}" "${COMMON_FLAGS[@]}" "${TARGETS[@]}" >"$dest" 2>"$dest.stderr" || status=$?
  if [ "$status" -ge 2 ]; then
    printf '%s の実行が失敗しました (exit %s)\n' "$label" "$status" >&2
    tail -20 "$dest.stderr" >&2 || true
    exit 2
  fi
  if ! jq -e . "$dest" >/dev/null 2>&1; then
    printf '%s の出力が JSON として読めません: %s\n' "$label" "$dest" >&2
    exit 2
  fi
}

run_linter reference "$REFERENCE_CMD" "$OUT_DIR/reference.json"
run_linter candidate "$CANDIDATE_CMD" "$OUT_DIR/candidate.json"

# 1 offense = 1 行の TSV に正規化する。path は末尾一致で比較できるよう相対のまま扱う。
normalize() {
  jq -r '
    [ .files[]? as $f
      | ($f.offenses // [])[]
      | { path: $f.path,
          cop: .cop_name,
          line: .location.line,
          column: .location.column,
          length: (.location.length // 0),
          severity: .severity,
          correctable: (.correctable // false),
          message: .message }
    ]
    | sort_by(.path, .line, .column, .cop)
    | .[]
    | [.path, .cop, (.line|tostring), (.column|tostring), (.length|tostring), .severity, (.correctable|tostring), .message]
    | @tsv
  ' "$1"
}

normalize "$OUT_DIR/reference.json" >"$OUT_DIR/reference.tsv"
normalize "$OUT_DIR/candidate.json" >"$OUT_DIR/candidate.tsv"

# `Lint/Syntax` の位置差と「どちらがそのファイルを構文エラー扱いしたか」は別の量である。
# 同じファイルを両者とも拒否していても、回復後の 2 件目以降はパーサ固有の位置に分かれる。
# その位置を一律に false positive / false negative と呼ぶと、候補だけが有効なファイルを
# 拒否したように読めるため、まず fatal の**ファイル集合**を独立に比較する。
fatal_syntax_paths() {
  jq -r '
    .files[]?
    | select(any(.offenses[]?; .cop_name == "Lint/Syntax" and .severity == "fatal"))
    | .path
  ' "$1" | LC_ALL=C sort -u
}

fatal_syntax_paths "$OUT_DIR/reference.json" >"$OUT_DIR/reference-unparsable.paths"
fatal_syntax_paths "$OUT_DIR/candidate.json" >"$OUT_DIR/candidate-unparsable.paths"
comm -12 "$OUT_DIR/reference-unparsable.paths" "$OUT_DIR/candidate-unparsable.paths" \
  >"$OUT_DIR/shared-unparsable.paths"
comm -23 "$OUT_DIR/reference-unparsable.paths" "$OUT_DIR/candidate-unparsable.paths" \
  >"$OUT_DIR/reference-only-unparsable.paths"
comm -13 "$OUT_DIR/reference-unparsable.paths" "$OUT_DIR/candidate-unparsable.paths" \
  >"$OUT_DIR/candidate-only-unparsable.paths"
cat "$OUT_DIR/reference-unparsable.paths" "$OUT_DIR/candidate-unparsable.paths" \
  | LC_ALL=C sort -u >"$OUT_DIR/unparsable.paths"

reference_unparsable_count=$(wc -l <"$OUT_DIR/reference-unparsable.paths" | tr -d ' ')
candidate_unparsable_count=$(wc -l <"$OUT_DIR/candidate-unparsable.paths" | tr -d ' ')
shared_unparsable_count=$(wc -l <"$OUT_DIR/shared-unparsable.paths" | tr -d ' ')
reference_only_unparsable_count=$(wc -l <"$OUT_DIR/reference-only-unparsable.paths" | tr -d ' ')
candidate_only_unparsable_count=$(wc -l <"$OUT_DIR/candidate-only-unparsable.paths" | tr -d ' ')
unparsable_count=$(wc -l <"$OUT_DIR/unparsable.paths" | tr -d ' ')

# 空の第 1 引数だと awk の NR==FNR が第 2 ファイルにも当たって全行落ちるため、
# 除外対象があるときだけ通す。
if [ "$EXCLUDE_UNPARSABLE" -eq 1 ] && [ "$unparsable_count" -gt 0 ]; then
  for name in reference candidate; do
    awk -F'\t' 'NR == FNR { skip[$0] = 1; next } !($1 in skip)' \
      "$OUT_DIR/unparsable.paths" "$OUT_DIR/$name.tsv" >"$OUT_DIR/$name.kept"
    mv "$OUT_DIR/$name.kept" "$OUT_DIR/$name.tsv"
  done
fi

# 位置キー (path,cop,line,column) だけを取り出した集合。メッセージ差分と分けて数える。
cut -f1-4 "$OUT_DIR/reference.tsv" | sort -u >"$OUT_DIR/reference.keys"
cut -f1-4 "$OUT_DIR/candidate.tsv" | sort -u >"$OUT_DIR/candidate.keys"

comm -13 "$OUT_DIR/reference.keys" "$OUT_DIR/candidate.keys" >"$OUT_DIR/candidate_only.tsv"
comm -23 "$OUT_DIR/reference.keys" "$OUT_DIR/candidate.keys" >"$OUT_DIR/reference_only.tsv"
comm -12 "$OUT_DIR/reference.keys" "$OUT_DIR/candidate.keys" >"$OUT_DIR/matched.keys"

# 共有の構文エラーファイル内にある Lint/Syntax の位置差は、ファイル判定の
# false positive / false negative ではなく、パーサ診断位置の差として分ける。
# FILENAME で入力を判別するため、共有集合が空でも awk の NR==FNR 空入力罠を踏まない。
: >"$OUT_DIR/false_positives.tsv"
: >"$OUT_DIR/syntax_recovery_candidate_only.tsv"
awk -F'\t' \
  -v actionable="$OUT_DIR/false_positives.tsv" \
  -v recovery="$OUT_DIR/syntax_recovery_candidate_only.tsv" '
  FILENAME == ARGV[1] { shared[$0] = 1; next }
  ($1 in shared) && $2 == "Lint/Syntax" { print > recovery; next }
  { print > actionable }
' "$OUT_DIR/shared-unparsable.paths" "$OUT_DIR/candidate_only.tsv"

: >"$OUT_DIR/false_negatives.tsv"
: >"$OUT_DIR/syntax_recovery_reference_only.tsv"
awk -F'\t' \
  -v actionable="$OUT_DIR/false_negatives.tsv" \
  -v recovery="$OUT_DIR/syntax_recovery_reference_only.tsv" '
  FILENAME == ARGV[1] { shared[$0] = 1; next }
  ($1 in shared) && $2 == "Lint/Syntax" { print > recovery; next }
  { print > actionable }
' "$OUT_DIR/shared-unparsable.paths" "$OUT_DIR/reference_only.tsv"

# Sonicop-only の位置が共通診断より後ろなら、「有効なファイルを候補だけが拒否した」のでは
# なく、両者がエラーを認識した後の診断分岐だと機械的に確認できる。README に載せる
# Homebrew の 263 件は、この分類で全件が after-shared、without-earlier-shared が 0 になる。
: >"$OUT_DIR/syntax_recovery_candidate_after_shared.tsv"
: >"$OUT_DIR/syntax_recovery_candidate_without_earlier_shared.tsv"
awk -F'\t' \
  -v after="$OUT_DIR/syntax_recovery_candidate_after_shared.tsv" \
  -v unexplained="$OUT_DIR/syntax_recovery_candidate_without_earlier_shared.tsv" '
  FILENAME == ARGV[1] {
    if ($2 != "Lint/Syntax") next
    if (!($1 in line) || $3 + 0 < line[$1] || ($3 + 0 == line[$1] && $4 + 0 < column[$1])) {
      line[$1] = $3 + 0
      column[$1] = $4 + 0
    }
    next
  }
  ($1 in line) && ($3 + 0 > line[$1] || ($3 + 0 == line[$1] && $4 + 0 > column[$1])) {
    print > after
    next
  }
  { print > unexplained }
' "$OUT_DIR/matched.keys" "$OUT_DIR/syntax_recovery_candidate_only.tsv"

# 位置が一致したものだけを対象に、severity / correctable / message の食い違いを拾う。
# join は複合キーにタブを含められず直積になるため、awk の連想配列で突き合わせる。
sort -u "$OUT_DIR/reference.tsv" >"$OUT_DIR/reference.sorted"
sort -u "$OUT_DIR/candidate.tsv" >"$OUT_DIR/candidate.sorted"
awk -F'\t' '
  NR == FNR {
    key = $1 SUBSEP $2 SUBSEP $3 SUBSEP $4
    seen[key] = 1; severity[key] = $6; correctable[key] = $7; message[key] = $8
    next
  }
  {
    key = $1 SUBSEP $2 SUBSEP $3 SUBSEP $4
    if (key in seen && (severity[key] != $6 || correctable[key] != $7 || message[key] != $8)) {
      printf "%s\t%s\t%s\t%s\tref=[%s|%s] %s\tcand=[%s|%s] %s\n",
        $1, $2, $3, $4, severity[key], correctable[key], message[key], $6, $7, $8
    }
  }
' "$OUT_DIR/reference.sorted" "$OUT_DIR/candidate.sorted" >"$OUT_DIR/message_diff.tsv"

ref_count=$(wc -l <"$OUT_DIR/reference.keys" | tr -d ' ')
cand_count=$(wc -l <"$OUT_DIR/candidate.keys" | tr -d ' ')
fp_count=$(wc -l <"$OUT_DIR/false_positives.tsv" | tr -d ' ')
fn_count=$(wc -l <"$OUT_DIR/false_negatives.tsv" | tr -d ' ')
candidate_only_count=$(wc -l <"$OUT_DIR/candidate_only.tsv" | tr -d ' ')
reference_only_count=$(wc -l <"$OUT_DIR/reference_only.tsv" | tr -d ' ')
candidate_recovery_count=$(wc -l <"$OUT_DIR/syntax_recovery_candidate_only.tsv" | tr -d ' ')
reference_recovery_count=$(wc -l <"$OUT_DIR/syntax_recovery_reference_only.tsv" | tr -d ' ')
candidate_after_shared_count=$(wc -l <"$OUT_DIR/syntax_recovery_candidate_after_shared.tsv" | tr -d ' ')
candidate_without_earlier_shared_count=$(wc -l <"$OUT_DIR/syntax_recovery_candidate_without_earlier_shared.tsv" | tr -d ' ')
match_count=$(wc -l <"$OUT_DIR/matched.keys" | tr -d ' ')
msg_count=$(wc -l <"$OUT_DIR/message_diff.tsv" | tr -d ' ')

printf '\n=== conformance summary ===\n'
printf 'syntax-error files : reference=%s candidate=%s shared=%s\n' \
  "$reference_unparsable_count" "$candidate_unparsable_count" "$shared_unparsable_count"
printf 'syntax file diff   : candidate-only=%s reference-only=%s\n' \
  "$candidate_only_unparsable_count" "$reference_only_unparsable_count"
printf 'reference offenses : %s\n' "$ref_count"
printf 'candidate offenses : %s\n' "$cand_count"
printf 'matched (位置一致) : %s\n' "$match_count"
printf 'candidate-only pos : %s  (actionable=%s / shared syntax files=%s)\n' \
  "$candidate_only_count" "$fp_count" "$candidate_recovery_count"
printf 'reference-only pos : %s  (actionable=%s / shared syntax files=%s)\n' \
  "$reference_only_count" "$fn_count" "$reference_recovery_count"
printf 'candidate recovery : after-shared=%s / without-earlier-shared=%s\n' \
  "$candidate_after_shared_count" "$candidate_without_earlier_shared_count"
printf 'message/severity 差: %s\n' "$msg_count"
if [ "$ref_count" -gt 0 ]; then
  printf 'recall             : %s%%\n' "$((match_count * 100 / ref_count))"
fi
printf 'artifacts          : %s\n' "$OUT_DIR"

if [ "$QUIET" -eq 0 ]; then
  if [ "$fn_count" -gt 0 ]; then
    printf '\n--- 検出漏れが多い cop (上位 15) ---\n'
    cut -f2 "$OUT_DIR/false_negatives.tsv" | sort | uniq -c | sort -rn | head -15
  fi
  if [ "$fp_count" -gt 0 ]; then
    printf '\n--- 誤検出が多い cop (上位 15) ---\n'
    cut -f2 "$OUT_DIR/false_positives.tsv" | sort | uniq -c | sort -rn | head -15
  fi
fi

if [ "$candidate_only_count" -eq 0 ] && [ "$reference_only_count" -eq 0 ] \
  && [ "$msg_count" -eq 0 ] && [ "$candidate_only_unparsable_count" -eq 0 ] \
  && [ "$reference_only_unparsable_count" -eq 0 ]; then
  printf '\n完全一致\n'
  exit 0
fi
exit 1
