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

**RuboCop 1.89 の全 609 Cop を実装しています。** 本家のレジストリと名前まで一致しています。`Enabled: pending` の 159 個と `Enabled: false` の 56 個も含みます。この 215 個は本家でも既定の実行では走らないので、`--only` で名指しするか設定で有効にしてください。本家に存在しない Cop 名を書いた場合だけエラーで止まります（`--ignore-unrecognized-cops` で続行できます）。

### Cop 別の一致状況

全 609 Cop を両者で有効にし、本家の spec が供給する 37,491 ケースを、それぞれの spec が指定した `TargetRubyVersion` で走らせて比較した結果です。**完全一致**とは、その Cop の offense が位置・メッセージ・重大度・修正可否まで 1 件残らず一致し、どちらにも余りがないことを指します。

以下は 2026-08-28 の最後の全件掃引です。その後の個別修正はこの数値に織り込んでいないため、現在すべての Cop が完全一致したという意味ではありません。

<!-- conformance:start -->
| デパートメント | Cop 数 | 検証済み | 完全一致 | 相違 |
|---|---:|---:|---:|---:|
| Bundler | 7 | 7 | **7 ✓** | 0 |
| Gemspec | 10 | 10 | **10 ✓** | 0 |
| Layout | 100 | 100 | 90 | 10 |
| Lint | 157 | 157 | 147 | 10 |
| Metrics | 10 | 10 | **10 ✓** | 0 |
| Migration | 1 | 1 | **1 ✓** | 0 |
| Naming | 19 | 19 | **19 ✓** | 0 |
| Security | 7 | 7 | **7 ✓** | 0 |
| Style | 298 | 298 | 288 | 10 |
| **合計** | **609** | **609** | **579** | **30** |
<!-- conformance:end -->

**先に読むべきは「検証済み」の列です。** ここで一度も発火しなかった Cop は、沈黙が一致と見分けられないため、訊かれないまま一致に数えられてしまいます。**609 個すべてがここで発火します**。それが「完全一致」の列を意味あるものにしています。

すべての Cop に届かせる方法、本家 spec から直接抽出したケースでの照合、既定以外の設定値の計測は [docs/cop-conformance.ja.md](docs/cop-conformance.ja.md) にまとめています。

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

cache 系の引数は、受理するだけでなく実際に効きます。sonicop は独自の結果キャッシュを持ち、検査時からサイズ・更新時刻・パーミッションのいずれも動いていないファイルには、保存済みのレポートをそのまま返します。

- キャッシュは既定で有効です。`--cache false` で無効化できます。設定ファイルの `AllCops/MaxFilesInCache: 0` でも同じです。
- 置き場所は `--cache-root DIR` で指定します。省略時は `$XDG_CACHE_HOME/sonicop`、macOS では `~/Library/Caches/sonicop`、それ以外は `~/.cache/sonicop` です。`--cache-root` は `--cache false` とは併用できません。
- 保持するレポート数の上限は `AllCops/MaxFilesInCache` で、既定は本家と同じ 20,000 件です。
- autocorrect 実行、`--stdin`、`--profile`、`--memory` では読み書きしません。
- 本家のキャッシュとは共有しません。形式が別物であり、書いたときとまったく同じビルドの sonicop にしかエントリを返さないためです。

無言なのは Cop の設定値のほうです。sonicop が実装していない設定値は**警告なしに無視されます**。名前を綴り間違えた設定値も同様です。つまり **offense が 0 件であることは、その設定が効いた証拠にはなりません**。無視された設定値と、違反の無いファイルが、同じ出力になるためです。どの設定値まで検証済みかは [既定以外の設定値](docs/cop-conformance.ja.md#既定以外の設定値) を参照してください。

Cop の*名前*は検査されます。設定ファイルに未知の Cop 名があれば、実行はエラーで止まります。素通りするのは、既知の Cop の中の設定値です。

## 適合性

sonicop は仕様とする RuboCop 1.89.0 の Cop を **609 / 609** 実装しています。固定した 5 つの Ruby プロジェクト（対象 18,251 ファイル）で、対象パスの集合は両者で一致します。既定設定の JSON は RuboCop 自身（offense 5,766 件）、Rails（167,760 件）、Mastodon（15,286 件）で、位置と全項目まで一致しました。Homebrew の差は `Lint/Syntax` のエラー復帰後の位置だけで、構文エラーと判定する 569 ファイルの集合は一致します。Ruby にはパーサと Cop の差が残るため、完全一致には数えません。

autocorrect は両者が別々の複製を修正した結果をバイト単位で比較します。本家が有効な Ruby を構文エラーに変える、代入順によって値を失う、ローカル変数の参照を `NameError` になるメソッド呼び出しへ変える場合は、安全のため意図的に一致させません。既知差分は[マニフェスト](tests/conformance/known_divergences.yml)に修正後ファイルのハッシュで固定しています。本家が最後まで処理できる 4 コーパスでは、`-a`・`-A` とも新規差分は 0 件です。Ruby は本家が途中で停止するため、完全な修正後ツリーを比較できません。Homebrew の修正後ツリーは両モードともバイト単位で一致します。

コーパスのコミット、版、offense の比較項目、実測値と限界は [CONFORMANCE.md](CONFORMANCE.md) に記録しています。[Cop の適合表](#cop-conformance)は本家 spec の入力で 609 Cop すべてを発火させる別の計測で、完全一致は 579 Cop、残る 30 Cop は未達です。

## 性能

2026-10-04 に Apple M2（8 コア）上で、最新安定版 Ruby 4.0.7 と RuboCop 1.91.0 を使って測定しました。これは新しい gem との速度比較です。[適合性](#適合性)の仕様は RuboCop 1.89.0 のままです。RuboCop 1.91.0 の Cop は 613 個、sonicop が実装する 1.89.0 の Cop は 609 個です。既定有効は両者 394 個ですが、互いに 1 個ずつ名前が異なります。速度計測では RuboCop から `Lint/CopDirectiveSyntax`、sonicop から `Style/DoubleCopDisableDirective` を除き、**共通の既定有効 393 Cop** に揃えました。

両者に `--force-default-config` を付け、対象パス集合を先に照合しています。Ruby では本家を途中で停止させる 2 ファイルを**両者から**除いた複製を使い、7,466 対象のうち 7,464 ファイルを測りました。各条件はキャッシュを空にして 2 回走らせた最速値です。並列の RuboCop は `--parallel --cache true` と毎回新しい実パスのキャッシュ、並列の sonicop も毎回新しいキャッシュを使います。単一プロセスでは両者 `--cache false`、sonicop に `--no-parallel` を指定しました。JSON 出力の検査数が対象数に届いたことを確認し、速度計測の各実行で終了コードを記録しました。

| コーパス | リビジョン | ファイル | 本家 offense | sonicop offense | 本家 並列 | sonicop 並列 | 本家 単一 | sonicop 単一 |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| rails/rails | `62b5458` | 3,551 | 167,899 | 167,760 | 36.70 s | 7.12 s | 93.50 s | 23.31 s |
| rubocop/rubocop | `f009b33` | 1,765 | 5,501 | 5,766 | 20.98 s | 2.34 s | 53.55 s | 7.91 s |
| mastodon/mastodon | `fad3685` | 3,290 | 15,292 | 15,286 | 16.42 s | 2.39 s | 38.33 s | 7.74 s |
| Homebrew/brew | `38ee325` | 2,179 | 49,926 | 49,326 | 18.50 s | 2.58 s | 42.70 s | 7.62 s |
| ruby/ruby | `3349f41` | 7,464 | 763,186 | 761,186 | 152.90 s | 18.07 s | 297.97 s | 54.32 s |

offense の列も、この速度計測と同じ条件で実測した値です。新しい RuboCop では 1.89.0 から Cop の挙動が変わり得るため、同じ名前の Cop を走らせても仕事量が同じとは限りません。検出数と時間を並べて読み、エンジンの比較には単一プロセスの列を優先してください。並列の実時間は OS のスケジューリングにも左右されます。各コーパスの測定前後のロードアベレージを下に記録します。

| コーパス | 開始時 | 終了時 | 記録した最大値 |
|---|---:|---:|---:|
| rails/rails | 7.50 | 19.08 | 116.81 |
| rubocop/rubocop | 13.41 | 27.13 | 51.63 |
| mastodon/mastodon | 26.29 | 29.53 | 77.93 |
| Homebrew/brew | 31.96 | 25.14 | 48.35 |
| ruby/ruby | 21.18 | 11.64 | 248.83 |

このマシンは測定中も負荷が高く、OS の別処理と競合していました。秒数はこの条件での実測値であり、静かな環境での下限値ではありません。

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
