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

Bundler、Gemspec、Layout、Lint、Metrics、Migration、Naming、Security、Style の各部門の Cop を実装しています。
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

実装済み Cop は、RuboCop 自身・Rails・Ruby・Homebrew・Mastodon の 5 プロジェクト
計 18,244 ファイルに対して、両者とも本家既定設定で検証しています。offense は Cop 名・パス・
行・桁・終端行・終端桁・長さ・メッセージ・重大度・修正可否のすべてで突き合わせます。

5 つのうち 3 つが**完全一致**です。RuboCop 自身のツリー（4,142 件）、Rails（117,541 件）、
Mastodon（7,610 件）で、過剰も不足もメタデータ差もありません。対象ファイル一覧は 5 つすべてで
一致します。残る差分は `Lint/Syntax` に集中しており、本家の LALR パーサが構文エラーから回復して
出す追加診断を tree-sitter では再現できないことによるものです。autocorrect は計測したすべての
コーパスでバイト単位に一致します。

コマンド、コーパスごとの数値、この種の計測が誤った結論を導く 2 つの罠は
[CONFORMANCE.md](CONFORMANCE.md) にまとめています。

### 性能

適合性検証に使う 5 コーパスすべてで測定しました。両者とも同梱の既定設定
（`--force-default-config`）で走らせているためプロジェクト側の `.rubocop.yml` は読まず、
どのコーパスでも**対象ファイル一覧は完全に一致**しています。

RuboCop は Sonicop の実装済み 28 Cop に絞っています。これが唯一の対等な比較です。
各値は暖機後 2 回の最速値です。

| コーパス | ファイル | RuboCop 並列 | Sonicop 並列 | RuboCop 単一 | Sonicop 単一 |
|---|---:|---:|---:|---:|---:|
| rubocop/rubocop | 1,765 | 5.30 秒 | **1.41 秒** | 20.88 秒 | **6.22 秒** |
| mastodon/mastodon | 3,289 | 3.77 秒 | **1.54 秒** | 11.18 秒 | **5.67 秒** |
| Homebrew/brew | 2,175 | 8.52 秒 | **1.47 秒** | 19.26 秒 | **5.20 秒** |
| rails/rails | 3,550 | 10.62 秒 | **5.07 秒** | 32.89 秒 | **15.58 秒** |
| ruby/ruby | 7,465 | 27.31 秒 | **11.19 秒** | 96.19 秒 | **34.85 秒** |

差は並列で 2.1〜5.8 倍、単一プロセスで 2.0〜3.7 倍と幅があり、1 コーパスでは代表できません。
単一プロセスの列のほうが安定しています。エンジンそのものを測っており、
各ツールの並列化が対象ツリーにたまたま噛み合ったかどうかに左右されないためです。

素で叩くと両者は違う量の仕事をします。RuboCop は既定で 394 Cop、Sonicop は 28 Cop なので、
**対等な比較ではありません**。参考までに RuboCop の既定・並列は、自身のツリーで 10.46 秒、
Mastodon で 9.91 秒、Homebrew で 10.83 秒、Rails で 20.73 秒、Ruby で 75.05 秒です。

この 28 Cop について、RuboCop 自身のツリー・Rails・Mastodon では**すべての offense が一致**
しているため、仕事を省いて速いわけではありません。

再現時に注意が必要な点が 2 つあります。RuboCop は **`--cache false` と併用すると
`--parallel` を黙って無効化します**。そのためここでの並列実行はキャッシュを有効にしたうえで
実行ごとにキャッシュディレクトリを消しており、`--cache false --parallel` で計測すると
単一プロセスを測ることになり差が過大に出ます。また RuboCop の既定は単一プロセス、
Sonicop は `--no-parallel` を渡さない限り並列です。

```bash
# RuboCop（並列・キャッシュは毎回空・Sonicop の実装済み Cop に限定）
rubocop --force-default-config --cache true --cache-root "$(mktemp -d)" \
        --no-color --parallel --only "$COPS" -f quiet

# Sonicop
sonicop --force-default-config --format quiet
```

測定機は Apple M2（8 コア）、Ruby 4.0.6（YJIT 利用可）、RubyGems 導入の RuboCop 1.89.0。
**他に負荷のない状態で測ってください。** コアを奪い合うものが動いていると両者とも不均等に
膨らみます。実際、競合状態で測ったときは RuboCop の Rails が 10.6 秒ではなく 21 秒と出ました。

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
