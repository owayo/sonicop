#!/usr/bin/env bash
# 上流 RuboCop の config/default.yml を取り込み直す。
#
# 同梱設定は「どの cop を認識するか」の唯一の根拠なので、どの版から持ってきたかが
# ファイル自身に残っていないと再取得が属人化する。取得と同時に由来を先頭へ書き込み、
# cop 数が変わったかどうかもその場で見えるようにする。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DESTINATION="${REPO_ROOT}/config/default.yml"
RUST_CONFIG="${REPO_ROOT}/src/config/mod.rs"
UPSTREAM="https://raw.githubusercontent.com/rubocop/rubocop"
VERSION=""
DRY_RUN=0

die() {
  printf '%s\n' "sync_default_yml: $1" >&2
  exit 2
}

usage() {
  cat <<USAGE
使い方: sync_default_yml.sh [options] <rubocop-version>

  <rubocop-version>   取り込む RuboCop のバージョン (例: 1.89.0 / v1.89.0)

  -o, --out PATH      書き込み先 (既定: config/default.yml)
  -n, --dry-run       差分の要約だけ出して書き込まない
  -h, --help          このヘルプを表示

取り込み後は cop 数が変わっていないかを cargo test が検証する。
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    -o | --out)
      DESTINATION="${2:-}"
      shift 2
      ;;
    -n | --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      usage >&2
      die "不明なオプション: $1"
      ;;
    *)
      [ -z "$VERSION" ] || die "バージョンは 1 つだけ指定してください"
      VERSION="$1"
      shift
      ;;
  esac
done

if [ -z "$VERSION" ]; then
  usage >&2
  die "RuboCop のバージョンを指定してください"
fi
command -v curl >/dev/null 2>&1 || die "curl が必要です"

# `1.89.0` でも `v1.89.0` でも受ける。タグは常に v 付き。
VERSION="${VERSION#v}"
case "$VERSION" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) die "バージョンの形式が正しくありません: ${VERSION} (例: 1.89.0)" ;;
esac

TAG="v${VERSION}"
SOURCE_URL="${UPSTREAM}/${TAG}/config/default.yml"

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sonicop-default-yml.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT

DOWNLOAD="${WORK_DIR}/default.yml"
printf '%s\n' "取得中: ${SOURCE_URL}"
curl -fsSL --proto '=https' --tlsv1.2 -o "$DOWNLOAD" "$SOURCE_URL" ||
  die "取得に失敗しました。タグ ${TAG} が存在するか確認してください"

[ -s "$DOWNLOAD" ] || die "取得したファイルが空です"
grep -q '^AllCops:' "$DOWNLOAD" || die "取得したファイルに AllCops が見つかりません"

# トップレベルの `Department/CopName:` 見出しが cop 1 件に対応する。
count_cops() {
  grep -cE '^[A-Z][A-Za-z]*/[A-Za-z0-9_]+:[[:space:]]*$' "$1" || true
}

new_count="$(count_cops "$DOWNLOAD")"
old_count=0
[ -f "$DESTINATION" ] && old_count="$(count_cops "$DESTINATION")"

[ "$new_count" -gt 100 ] || die "cop 見出しが ${new_count} 件しかありません。取得内容を確認してください"

OUTPUT="${WORK_DIR}/vendored.yml"
{
  printf '%s\n' "# vendored from rubocop ${TAG}"
  printf '%s\n' "# source: ${SOURCE_URL}"
  printf '%s\n' "# Regenerate with: scripts/sync_default_yml.sh ${VERSION}"
  printf '%s\n' '# Do not edit by hand.'
  # 取り込んだ本文と、こちらが足したヘッダの境目を目で追えるようにする。
  printf '\n'
  cat "$DOWNLOAD"
} >"$OUTPUT"

printf '%s\n' "rubocop ${TAG}: cop ${old_count} 件 -> ${new_count} 件"

if [ "$DRY_RUN" -eq 1 ]; then
  if [ -f "$DESTINATION" ]; then
    diff -u "$DESTINATION" "$OUTPUT" | head -40 || true
  fi
  printf '%s\n' "dry-run のため書き込みませんでした"
  exit 0
fi

cp "$OUTPUT" "$DESTINATION"
printf '%s\n' "更新しました: ${DESTINATION#"${REPO_ROOT}/"}"

# 認識 cop 数は Rust 側のテストが固定値で持っている。ここでは食い違いを知らせるだけにし、
# 実際の検証は cargo test に任せる (版が上がれば増減するのが正常なため)。
expected=""
if [ -f "$RUST_CONFIG" ]; then
  expected="$(sed -n 's/.*known_cop_names().count(), *\([0-9][0-9]*\).*/\1/p' "$RUST_CONFIG" | head -1)"
fi
if [ -n "$expected" ] && [ "$expected" != "$new_count" ]; then
  printf '%s\n' ""
  printf '%s\n' "要対応: src/config/mod.rs の期待値 ${expected} と取り込んだ ${new_count} 件が食い違っています。"
  printf '%s\n' "        テストの期待値を ${new_count} に更新してください。"
fi
printf '%s\n' "確認: make test"
