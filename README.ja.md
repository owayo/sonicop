<p align="center">
  <img src="docs/images/sonicop_logo_header.png" width="320" alt="sonicop">
</p>

<h1 align="center">sonicop</h1>

<p align="center">
  Rust で実装した高速なネイティブ RuboCop 互換 Ruby リンター／フォーマッター
</p>

<!-- standard:badges:start -->
<h3 align="center">対応プラットフォーム</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&amp;logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&amp;logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/Windows-0078D6" alt="Windows">
</p>

<p align="center">
  <a href="https://github.com/owayo/sonicop/actions/workflows/ci.yml"><img src="https://github.com/owayo/sonicop/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/sonicop/releases/latest"><img src="https://img.shields.io/github/v/release/owayo/sonicop" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/owayo/sonicop" alt="License"></a>
  <a href="https://rubygems.org/gems/sonicop"><img src="https://img.shields.io/gem/v/sonicop" alt="RubyGems"></a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>
<!-- standard:badges:end -->

---

Sonicop は、Ruby プロセスを起動せずに動作する高速な Rust 製 Ruby リンター／フォーマッターです。既存の `.rubocop.yml` をそのまま利用でき、サブディレクトリごとの設定、設定の継承、対象ファイルの Include／Exclude、severity、自動修正設定にも対応しています。

最新の Ruby 構文へ追従する [owayo/tree-sitter-ruby](https://github.com/owayo/tree-sitter-ruby) を使い、ファイルを並列に検査して修正をアトミックに適用します。RuboCop 1.89 互換の CLI と JSON 出力により、既存のエディタや CI へ最小限の変更で導入できます。

## 機能

Bundler、Gemspec、Layout、Lint、Metrics、Migration、Naming、Security、Style の各デパートメントの Cop を実装しています。一覧はバイナリ自身が正本です。

```bash
# 認識済み Cop と実装状況の一覧
sonicop --show-cops
```

**RuboCop 1.89 の全 609 Cop を実装しています。** 本家のレジストリと名前まで一致し、プロジェクト索引を使う検出にも対応しています。`Enabled: pending` の 159 個と `Enabled: false` の 56 個も含みます。この 215 個は本家でも既定の実行では走らないので、`--only` で名指しするか設定で有効にしてください。本家に存在しない Cop 名を書いた場合だけエラーで止まります（`--ignore-unrecognized-cops` で続行できます）。

### Cop 別の一致状況

2026-10-16 に RuboCop 1.89.0 と比較しました。本家 spec の記録から **42,433 入力・1,543 設定群**、追加で**索引の 174 ケースとファイル名に依存する 2 ケース**を実行しました。設定群ごとの `TargetRubyVersion`、ファイル種別、Ruby の設定値の型を保っています。**完全一致**は開始・終端位置、length、メッセージ、重大度、修正可否、同じ記録の重複まで一致することを指します。異なる実行の差分は相殺しません。

別の６設定群・161 入力は、本家の spec helper が許す null の `EnforcedStyle` を CLI が拒否するため未測定です。表は測定できた入力の結果であり、すべての設定値を保証するものではありません。

<!-- conformance:start -->

| 部門 | Cop 数 | 発火 | 完全一致 | 差分 |
|---|---:|---:|---:|---:|
| Bundler | 7 | 7 | **7 ✓** | 0 |
| Gemspec | 10 | 10 | **10 ✓** | 0 |
| Layout | 100 | 100 | **100 ✓** | 0 |
| Lint | 157 | 157 | **157 ✓** | 0 |
| Metrics | 10 | 10 | **10 ✓** | 0 |
| Migration | 1 | 1 | **1 ✓** | 0 |
| Naming | 19 | 19 | **19 ✓** | 0 |
| Security | 7 | 7 | **7 ✓** | 0 |
| Style | 298 | 298 | **298 ✓** | 0 |
| **合計** | **609** | **609** | **609** | **0** |

<!-- conformance:end -->

**今回の掃引では全 609 Cop が発火し、完全一致しました。** `Lint/DeprecatedReference`、`Lint/NameTypo`、`Lint/UnusedPrivateMethod` は `AllCops: UseProjectIndex: true` で、ファイルをまたぐ参照も検査しています。この被覆の結果とは別に、下記の実コードには構文エラー復帰の差が残っています。

測定条件と残る不足は [docs/cop-conformance.ja.md](docs/cop-conformance.ja.md) に記録しています。

## インストール

<!-- standard:install:start -->
### Cargo

Rust 1.98 以上が必要です。

```bash
cargo install --git https://github.com/owayo/sonicop --locked
```

### RubyGems

```bash
gem install sonicop
```

### ソースから

[mise](https://mise.jdx.dev/) が必要です (Rust のツールチェーンは `mise.toml` で固定しています)。

```bash
git clone https://github.com/owayo/sonicop.git
cd sonicop
make install
```

`make install` は `/usr/local/bin` に入れます。場所を変えるときは `INSTALL_PATH` を指定します (例: `make install INSTALL_PATH="$HOME/.local/bin"`)。
<!-- standard:install:end -->

### プラットフォーム別の gem

[RubyGems](https://rubygems.org/gems/sonicop) で配っている Linux、macOS、Windows 向けの platform gem には、ネイティブ実行ファイルが含まれます。対応する platform gem がない環境では、source gem がインストール時に Cargo でビルドします。

## 使い方

```bash
# 現在のプロジェクトを検査
sonicop

# Cop／デパートメントを指定
sonicop --only Layout,Style/StringLiterals app spec

# 安全な自動修正／全自動修正
sonicop -a
sonicop -A

# RuboCop 互換形状の JSON
sonicop --format json

# エディタからの入力
printf '%s\n' 'value=10000' | sonicop --stdin example.rb --format json

# 認識済み Cop と実装状況
sonicop --show-cops
```

主な互換オプションは `-l`、`-x`、`--only`、`--except`、`-s/--stdin`、`-P/--parallel`、`-f/--format`、`-a/--autocorrect`、`-A/--autocorrect-all`、`-L/--list-target-files`、`-c/--config`、`-v/--version`、`-V/--verbose-version` です。

## 設定

検査するファイルごとに `.rubocop.yml` を解決するため、サブディレクトリ設定も cop に適用します。対象ファイルの探索には、指定したディレクトリの設定を配下全体に使います。入れ子の `Include`／`Exclude` を探索に反映するには、そのディレクトリを別に指定してください。この挙動は RuboCop と一致します。ローカル／HTTPS の `inherit_from`、`inherit_gem`、`inherit_mode`、`AllCops/DisabledByDefault`、`Include`／`Exclude`、Cop ごとの `Enabled`、`Exclude`、`Severity`、`Safe`、`SafeAutoCorrect` と設定値に対応します。宣言されたプラグイン由来の Cop は「認識済み・未実装」として受理し、Ruby プラグインコード自体は実行しません。リモート設定のリクエストには 30 秒のネットワークタイムアウトを設け、応答は 1 件あたり 5 MiB に制限します。

```yaml
inherit_from: .rubocop_todo.yml

AllCops:
  Exclude:
    - "vendor/**/*"

Layout/LineLength:
  Max: 100

Style/StringLiterals:
  EnforcedStyle: single_quotes
```

既存コマンドとの互換性を保つため、server/LSP/MCP、plugin 系の引数も受理します。サーバートランスポート、Ruby プラグイン実行、カスタム Cop、実装済み以外の Cop は実行しません。これらはその旨を出力します。`--server` / `--no-server` / `--lsp` / `--mcp` / `--plugin` は stderr に 1 行の注記を出します。

`AllCops/UseProjectIndex: true` を指定すると、`Lint/DeprecatedReference`、`Lint/NameTypo`、`Lint/UnusedPrivateMethod` を含むプロジェクト全体の解析が有効になります。プロジェクト内の索引作成には Ruby は不要です。`AllCops/ProjectIndexIncludesGems: true` の場合は、Bundler がロードされた Ruby gem ラッパー経由の実行で、bundle 内の gem も索引に含めます。他ファイルの変更も診断に影響するため、索引を使う実行では結果キャッシュを無効にします。

cache 系の引数は、受理するだけでなく実際に効きます。sonicop は独自の結果キャッシュを持ち、検査時からサイズ・更新時刻・パーミッションのいずれも動いていないファイルには、保存済みのレポートをそのまま返します。

- キャッシュは既定で有効です。`--cache false` で無効化できます。設定ファイルの `AllCops/MaxFilesInCache: 0` でも同じです。
- 置き場所は `--cache-root DIR` で指定します。省略時は `$XDG_CACHE_HOME/sonicop`、macOS では `~/Library/Caches/sonicop`、それ以外は `~/.cache/sonicop` です。`--cache-root` は `--cache false` とは併用できません。
- 保持するレポート数の上限は `AllCops/MaxFilesInCache` で、既定は本家と同じ 20,000 件です。
- autocorrect 実行、`--stdin`、`--profile`、`--memory` では読み書きしません。
- 本家のキャッシュとは共有しません。形式が別物であり、書いたときとまったく同じビルドの sonicop にしかエントリを返さないためです。

無言なのは Cop の設定値のほうです。sonicop が実装していない設定値は**警告なしに無視されます**。名前を綴り間違えた設定値も同様です。つまり **offense が 0 件であることは、その設定が効いた証拠にはなりません**。無視された設定値と、違反の無いファイルが、同じ出力になるためです。どの設定値まで検証済みかは [既定以外の設定値](docs/cop-conformance.ja.md#既定以外の設定値) を参照してください。

Cop の*名前*は検査されます。設定ファイルに未知の Cop 名があれば、実行はエラーで止まります。素通りするのは、既知の Cop の中の設定値です。

## 適合性

sonicop は仕様とする RuboCop 1.89.0 の Cop を、プロジェクト索引の動作を含めて **609 / 609** 実装しています。固定した 5 つの Ruby プロジェクト（対象 18,251 ファイル）で、対象パスの集合は両者で一致します。既定設定の JSON は RuboCop 自身（offense 5,766 件）、Rails（167,760 件）、Mastodon（15,286 件）で、位置と全項目まで一致しました。Homebrew の差は `Lint/Syntax` のエラー復帰後の位置だけで、構文エラーと判定する 569 ファイルの集合は一致します。Ruby にはパーサと Cop の差が残るため、完全一致には数えません。

autocorrect は両者が別々の複製を修正した結果をバイト単位で比較します。本家が有効な Ruby を構文エラーに変える、代入順によって値を失う、ローカル変数の参照を `NameError` になるメソッド呼び出しへ変える場合は、安全のため意図的に一致させません。既知差分は[マニフェスト](tests/conformance/known_divergences.yml)に修正後ファイルのハッシュで固定しています。本家が最後まで処理できる 4 コーパスでは、`-a`・`-A` とも新規差分は 0 件です。Ruby は本家が途中で停止するため、完全な修正後ツリーを比較できません。Homebrew の修正後ツリーは両モードともバイト単位で一致します。

Ruby コーパス全体の冪等性ゲートは未達です。本家と同様に、200 パスの上限で無限ループを報告して途中の補正を書き戻し、次の実行でさらに変更する場合があります。これを収束した成功とは扱いません。[測定した限界](CONFORMANCE.md#autocorrect-and-safety)も参照してください。

コーパスのコミット、版、offense の比較項目、実測値と限界は [CONFORMANCE.md](CONFORMANCE.md) に記録しています。[Cop の適合表](#cop-別の一致状況)は別の計測で、測定した入力では全 609 Cop が発火し、完全一致しました。すべての Ruby プログラムや設定値での一致を保証するものではありません。

## 性能

2026-10-15 に[専用の GitHub Actions 実行](https://github.com/owayo/sonicop/actions/runs/38747062010)で、Ruby 4.0.7（`RUBYOPT=--yjit`）、RuboCop 1.92.0、sonicop のリビジョン `6b3e850` を測定しました。測定機は AMD EPYC 9V74 80-Core Processor（論理 CPU 4 個）、x86_64-pc-linux、ランナーイメージ `20261004.327.1` です。[適合性](#適合性)の仕様は引き続き RuboCop 1.89.0 です。新版は 613 Cop、sonicop は 609 Cop を持ちます。既定有効は両者 394 個ですが、1 個ずつ名前が異なるため、本家の `Lint/CopDirectiveSyntax` と sonicop の `Style/DoubleCopDisableDirective` を除いた**共通 393 Cop** で測りました。

入力はプロジェクトの外に置き、両 CLI の実効 TargetRubyVersion が Rails 3.3（`RAILS_VERSION=8.2.0.alpha`）、Mastodon 4.0、残り 2.7 であることを検査しました。両者に `--force-default-config` を付け、**JSON フォーマッタ**を使いました。計測する全実行で、検査完了・対象パス集合・診断の全項目がそれぞれの基準出力と一致することを確認しています。Ruby では本家を停止させる 2 ファイルを両者の複製から除き、7,466 対象のうち 7,464 ファイルを測りました。秒数は、先に実行するツールを交替した 2 回の**最速値 / 中央値**です。並列は利用可能な論理 CPU 全体と `--cache true`、毎回新しい実パスのキャッシュルートを使います。単一プロセスは両者とも `--cache false`、sonicop は `--no-parallel` を指定しました。計測した全実行は offense を報告して終了 1 を返しています。キャッシュのファイル数も記録していますが、warm の測定値は含めていません。

| コーパス | リビジョン | ファイル | 本家の検出 | sonicop の検出 | 本家 並列 | sonicop 並列 | 本家 単一 | sonicop 単一 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| rails/rails | `62b5458` | 3,551 | 167,889 | 167,760 | 46.26 / 46.64 | 8.75 / 8.76 | 87.06 / 87.96 | 20.92 / 21.00 |
| rubocop/rubocop | `f009b33` | 1,765 | 5,501 | 5,766 | 24.08 / 24.15 | 2.84 / 2.95 | 40.41 / 42.11 | 7.33 / 7.34 |
| mastodon/mastodon | `fad3685` | 3,290 | 15,292 | 15,286 | 22.67 / 22.79 | 3.09 / 3.15 | 38.38 / 39.64 | 8.00 / 8.02 |
| Homebrew/brew | `38ee325` | 2,179 | 49,921 | 49,326 | 23.22 / 23.42 | 2.87 / 2.89 | 40.77 / 41.41 | 6.80 / 6.80 |
| ruby/ruby | `3349f41` | 7,464 | 763,132 | 761,186 | 108.14 / 110.48 | 19.90 / 19.93 | 230.07 / 235.02 | 45.83 / 46.42 |

検出数のとおり、新しい本家とは処理内容が異なります。同じ仕事をした場合の速度倍率や、以前の sonicop からの改善率を表す値ではありません。単一プロセスの CPU 時間も最速値 / 中央値で示します。

| コーパス | 本家 user CPU | sonicop user CPU | 本家 system CPU | sonicop system CPU |
| --- | ---: | ---: | ---: | ---: |
| rails/rails | 84.09 / 84.96 | 20.71 / 20.79 | 2.89 / 2.92 | 0.21 / 0.21 |
| rubocop/rubocop | 38.48 / 40.13 | 7.27 / 7.29 | 1.88 / 1.92 | 0.05 / 0.06 |
| mastodon/mastodon | 36.37 / 37.55 | 7.92 / 7.94 | 1.95 / 2.02 | 0.08 / 0.08 |
| Homebrew/brew | 38.34 / 38.95 | 6.71 / 6.71 | 2.36 / 2.39 | 0.08 / 0.09 |
| ruby/ruby | 225.00 / 229.71 | 45.05 / 45.63 | 4.93 / 5.17 | 0.77 / 0.78 |

計測対象の最初と最後の実行、および記録した最大の 1 分ロードアベレージです。

| コーパス | 最初 | 最後 | 最大サンプル |
| --- | ---: | ---: | ---: |
| rails/rails | 1.33 | 1.08 | 4.45 |
| rubocop/rubocop | 1.00 | 1.47 | 3.53 |
| mastodon/mastodon | 1.22 | 1.49 | 3.20 |
| Homebrew/brew | 1.63 | 1.47 | 3.33 |
| ruby/ruby | 1.69 | 1.01 | 4.74 |

[測定結果](docs/benchmarks/2026-10-15.json)には、cop ごとの検出数、診断の差、終了コード、CPU 時間、キャッシュのファイル数、実行環境の版を保存しています。再実行は `make benchmark`、詳しい条件は[開発メモ](docs/development.ja.md#コーパスの検証と速度測定)を参照してください。

## 開発

<!-- standard:dev:start -->
[mise](https://mise.jdx.dev/) が必要です。ツールの版は `mise.toml` で固定しています。

```bash
make setup   # ツールチェーン (mise) と依存を取得する
make ci      # CI と同じ検査 (書き換えない)
```

| コマンド | 説明 |
|---|---|
| `make setup` | ツールチェーン (mise) と依存を取得する |
| `make build` | デバッグ版をビルドする |
| `make release` | リリース版をビルドする |
| `make run` | デバッグ版を実行する (引数は ARGS="...") |
| `make test` | テストを実行する |
| `make lint` | clippy を警告ゼロで通す |
| `make fmt` | コードを整形する (書き換える) |
| `make fmt-check` | 整形済みかを確かめる (書き換えない) |
| `make check` | 整形と静的検査 (書き換えない) |
| `make ci` | CI と同じ検査 (書き換えない) |
| `make install` | リリース版を INSTALL_PATH (既定 /usr/local/bin) に入れる |
| `make uninstall` | INSTALL_PATH から取り除く |
| `make clean` | ビルド成果物を消す |

`make` でターゲットの一覧を表示します。リリースは GitHub Actions で行います (**Actions → Release → Run workflow**)。
<!-- standard:dev:end -->

Rakefile には gem の組み立てと版の処理だけがあり、`make gem`・`make gem-platform`・`make version-sync` がそれを呼びます。版の正本は `Cargo.toml` です。Cop の追加、上流から取り込むファイル、ほかのターゲットは [docs/development.ja.md](docs/development.ja.md) にまとめています。

## ライセンス

<!-- standard:license:start -->
[MIT](LICENSE)
<!-- standard:license:end -->

同梱する RuboCop 既定設定とパーサー依存の著作権表示は [NOTICE](NOTICE) および [`licenses/`](licenses/) に収録しています。
