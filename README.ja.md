<h1 align="center">
  <img src="docs/images/sonicop_logo_header.png" width="600" alt="Sonicop">
</h1>

<p align="center">
  <strong>Rust で実装した高速なネイティブ RuboCop 互換 Ruby リンター／フォーマッター。</strong>
</p>

<p align="center">
  <a href="https://github.com/owayo/sonicop/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/owayo/sonicop/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://rubygems.org/gems/sonicop"><img alt="Gem Version" src="https://img.shields.io/gem/v/sonicop"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/owayo/sonicop"></a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>

---

## 概要

Sonicop は、Ruby プロセスを起動せずに動作する高速な Rust 製 Ruby リンター／フォーマッターです。
既存の `.rubocop.yml` をそのまま利用でき、サブディレクトリごとの設定、設定の継承、対象ファイルの
Include／Exclude、severity、自動修正設定にも対応しています。

最新の Ruby 構文へ追従する
[owayo/tree-sitter-ruby](https://github.com/owayo/tree-sitter-ruby) を使い、ファイルを並列に検査して
修正をアトミックに適用します。RuboCop 1.89 互換の CLI と JSON 出力により、既存のエディタや
CI へ最小限の変更で導入できます。

## 主な機能

Layout、Lint、Metrics、Naming、Security、Style の各部門の Cop を実装しています。
実装済みの Cop はリリースごとに増えるため、一覧はバイナリ自身が正本です。

```bash
# 認識済み Cop と実装状況の一覧
sonicop --show-cops
```

RuboCop 1.89 の全 609 Cop を同梱設定として認識します。実装済み Cop は検査を実行し、
認識済みで未実装の Cop は既存設定を壊さず `--debug` で一覧表示します。本家にも存在しない
Cop 名だけをエラーにし、必要なら `--ignore-unrecognized-cops` で続行できます。

## インストール

```bash
gem install sonicop
```

Linux、macOS、Windows 向けの platform gem にはネイティブ実行ファイルが含まれます。
対応する platform gem がない環境では、source gem がインストール時に Cargo でビルドします。

最新版のソースから直接インストールすることもできます。

```bash
cargo install --git https://github.com/owayo/sonicop
```

## 使い方

```bash
# 現在のプロジェクトを検査
sonicop

# Cop／部門を選択
sonicop --only Layout,Style/StringLiterals app spec

# 安全な自動修正／全自動修正
sonicop -a
sonicop -A

# RuboCop 互換形状の JSON
sonicop --format json

# 認識済み Cop と実装状況
sonicop --show-cops
```

主な互換オプションは `-l`、`-x`、`--only`、`--except`、`-s/--stdin`、
`-P/--parallel`、`-f/--format`、`-a/--autocorrect`、`-A/--autocorrect-all`、
`-L/--list-target-files`、`-c/--config`、`-v/--version`、`-V/--verbose-version` です。

### 設定

対象ファイルごとに `.rubocop.yml` を解決するため、1 回の実行でもサブディレクトリ設定が
適用されます。ローカル／HTTPS の `inherit_from`、`inherit_gem`、`inherit_mode`、
`AllCops/DisabledByDefault`、`Include`／`Exclude`、Cop ごとの `Enabled`、`Exclude`、
`Severity`、`Safe`、`SafeAutoCorrect` と設定値に対応します。宣言されたプラグイン由来の
Cop は「認識済み・未実装」として受理し、Ruby プラグインコード自体は実行しません。

```yaml
inherit_from: .rubocop_todo.yml

Layout/LineLength:
  Max: 100

Style/StringLiterals:
  EnforcedStyle: single_quotes
```

既存コマンドとの互換性を保つため、server/LSP/MCP、plugin、cache 系の引数も受理します。
未対応の機能を要求する引数は互換 no-op として stderr に明示します。現在、サーバートランスポート、
Ruby プラグイン実行、キャッシュ再利用、カスタム Cop、実装済み以外の Cop は実行しません。

### 適合性

実装済み Cop は、正規化した JSON offense を使って RuboCop 1.89 との一致を検証済みです。
本家の全 1,759 ファイルを本家既定設定で検査した現在のスナップショットは、参照 4,052 件中
4,052 件すべての位置が一致（**recall 100%**）し、**誤検出とメタデータ差もともに 0 件**です。コマンド、比較範囲、測定結果は
[CONFORMANCE.md](CONFORMANCE.md) にまとめています。Rails では Ruby 対象 3,453 ファイルと
ルート／`guides/` の有効 Cop 一覧が完全一致し、全体の誤検出は 0 件です。Active Record
402 ファイルの無キャッシュ実行は本家比で約 12.5 倍高速でした。

## 開発

入口は `make` に一本化しています。`make help` で全ターゲットを確認できます。gem 配布の
タスクは Rakefile 側にあり、`make` から呼び出します。

```bash
make build   # デバッグビルド
make check   # fmt、clippy、Rust テスト、Ruby ラッパーテスト、バージョン整合
make gem     # source gem
```

### Cop の追加

Cop は `src/rules/<部門>/<cop>.rs` の 1 ファイルで、公開するのは `check(context, offenses)` の
1 関数だけです。登録は部門の `mod.rs` に 1 行を足します。

```rust
department_rules! {
    "Layout";
    line_length => ("LineLength", Convention),
}
```

Cop 名と既定 severity を書くのはこの 1 行だけです。Cop 本体では名前は暗黙で、
`context.setting("Max")` が `Layout/LineLength: Max` を読み、`context.offense(message, range)` が
その Cop の名前と設定済み severity で報告します。Cop が自分の名前を 2 度書ける設計では、
レジストリと食い違っても型検査では捕まりません。

全ノード走査より `context.nodes_of("kind")` を優先してください。Cop は全ファイルに対して走るため、
Cop ごとの全走査はファイル規模ではなく Cop 数に比例して重くなります。

バージョンの正本は `Cargo.toml` です。`lib/sonicop/version.rb` は `make version-sync` で
生成してコミットします（gemspec がパッケージ時に読むため）。両者が食い違うと CI が落ちます。

`config/default.yml` は上流 RuboCop から取り込んだものです。再取得は
`scripts/sync_default_yml.sh <rubocop-version>` で行い、由来のバージョンがファイル先頭に
記録されます。

依存更新には `depup --install` を使います。Ruby grammar は再現可能性のため `Cargo.toml` で
fork のコミットを固定しています。

## ライセンス

[MIT](LICENSE)。同梱する RuboCop 既定設定とパーサー依存の著作権表示は
[NOTICE](NOTICE) および [`licenses/`](licenses/) に収録しています。
