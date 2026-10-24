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

最新の Ruby 構文へ追従する [owayo/tree-sitter-ruby](https://github.com/owayo/tree-sitter-ruby) を使い、ファイルを並列に検査し、ハードリンクを保持して修正します。RuboCop 1.89 互換の CLI と JSON 出力により、既存のエディタや CI へ最小限の変更で導入できます。

## 機能

Bundler、Gemspec、Layout、Lint、Metrics、Migration、Naming、Security、Style の各デパートメントの Cop を実装しています。一覧はバイナリ自身が正本です。

```bash
# 認識済み Cop と実装状況の一覧
sonicop --show-cops
```

**RuboCop 1.89 の全 609 Cop を実装しています。** 本家のレジストリと名前まで一致し、プロジェクト索引を使う検出にも対応しています。`Enabled: pending` の 159 個と `Enabled: false` の 56 個も含みます。この 215 個は本家でも既定の実行では走らないので、`--only` で名指しするか設定で有効にしてください。本家に存在しない Cop 名を書いた場合だけエラーで止まります（`--ignore-unrecognized-cops` で続行できます）。

### Cop 別の一致状況

2026-10-22 に RuboCop 1.89.0 と比較しました。本家 spec の記録から **42,433 入力・1,543 設定群**、追加で**索引の 174 ケースとファイル名に依存する 2 ケース**を実行しました。設定群ごとの `TargetRubyVersion`、ファイル種別、Ruby の設定値の型を保っています。**完全一致**は開始・終端位置、length、メッセージ、重大度、修正可否、同じ記録の重複まで一致することを指します。異なる実行の差分は相殺しません。

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

sonicop は本文の digest・サイズ・更新時刻・パーミッションに加え、ビルド・設定・選択した Cop も一致するときに保存済みレポートを再利用します。

- キャッシュは既定で有効です。`--cache false` で無効化できます。設定ファイルの `AllCops/MaxFilesInCache: 0` でも同じです。
- 置き場所は `--cache-root DIR` で指定します。省略時は `$XDG_CACHE_HOME/sonicop`、macOS では `~/Library/Caches/sonicop`、それ以外は `~/.cache/sonicop` です。`--cache-root` は `--cache false` とは併用できません。
- 保持するレポート数の上限は `AllCops/MaxFilesInCache` で、既定は本家と同じ 20,000 件です。
- autocorrect 実行、`--stdin`、`--profile`、`--memory` では読み書きしません。
- 本家のキャッシュとは共有しません。形式が別物であり、書いたときとまったく同じビルドの sonicop にしかエントリを返さないためです。

無言なのは Cop の設定値のほうです。sonicop が実装していない設定値は**警告なしに無視されます**。名前を綴り間違えた設定値も同様です。つまり **offense が 0 件であることは、その設定が効いた証拠にはなりません**。無視された設定値と、違反の無いファイルが、同じ出力になるためです。どの設定値まで検証済みかは [既定以外の設定値](docs/cop-conformance.ja.md#既定以外の設定値) を参照してください。

Cop の*名前*は検査されます。設定ファイルに未知の Cop 名があれば、実行はエラーで止まります。素通りするのは、既知の Cop の中の設定値です。

## 適合性

2026-10-24 に Windows で取得済み公開5コーパス、計 **18,670 対象**を調べました。仕様版は **RuboCop 1.89.0**、Ruby 4.0.7 / Parser 3.3.12.0 です。登録名609個と有効区分394 / pending 159 / 無効56は一致します。既定設定の全診断項目・重複・ファイル順を比較した結果は次のとおりです。

| コーパス | 比較対象 | RuboCop 1.89.0 | Sonicop | 欠落 | 追加 | 同位置の項目差 |
| --- | --- | --- | --- | --- | --- | --- |
| rails/rails | 3,588 | 176,909 | 176,909 | 0 | 0 | 0 |
| rubocop/rubocop | 1,807 | 7,957 | 7,957 | 0 | 0 | 0 |
| ruby/ruby | 7,522 / 7,524 | 776,604 | 776,679 | 3 | 78 | 3 |
| Homebrew/brew | 2,397 | 60,799 | 60,799 | 0 | 0 | 0 |
| mastodon/mastodon | 3,354 | 19,145 | 19,145 | 0 | 0 | 0 |

Ruby の本家は全体検査を終了2で中断します。個別エラーを確認した2ファイルを除き、完了した分割参照を照合しました。4コーパスは診断が全項目一致し、Ruby の残る差は7ファイルにあります。登録数・実コード上の一致・[2026-10-22 の Cop 別掃引](#cop-別の一致状況)は別の測定です。

最新 RuboCop 1.92.1 との診断差・補正差も別に測りました。新版の追加 JSON 項目 `correction` も差として残し、旧版から共通の診断項目の差と分けています。独立した複製での補正結果は次のとおりです。バイト差は本家と異なるファイル数で、最新版との互換性Pに合格したことを示しません。

| コーパス | -a バイト差 | -A バイト差 | 候補の終了 -a / -A | 2度目 -A の変更 |
| --- | --- | --- | --- | --- |
| rails/rails | 3,212 | 3,301 | 2 / 2 | 0 |
| rubocop/rubocop | 177 | 236 | 1 / 1 | 0 |
| ruby/ruby | 比較不可 | 比較不可 | 2 / 2 | 1 |
| Homebrew/brew | 1,582 | 1,608 | 1 / 1 | 0 |
| mastodon/mastodon | 1,349 | 1,752 | 2 / 2 | 0 |

完了した書き込みの構文検証で候補だけが壊したファイルは見つかりませんでした。ただし Rails・Mastodon・Ruby は拒否した書き込みなどで終了2となるため、全体の安全性Sに合格したとは扱いません。Ruby の参照補正は比較不可で、候補の再補正による変更も個別に記録します。CRLFへ正規化したときだけ一致する差も、未登録の互換一致へ読み替えません。

[今回の条件・残差・補正の詳細と生データ](docs/maintenance/2026-10-24.md)、[仕様と過去の証拠](CONFORMANCE.md)、[意図的差分のマニフェスト](tests/conformance/known_divergences.yml)を参照してください。

追加で RuboCop コーパスの `.rb` を CRLF 化した複製も調べ、仕様版1.89.0と1,807ファイル・6,167診断が全項目一致しました。補正結果は[詳細記録](docs/maintenance/2026-10-24.md#crlf-corpus-and-comparison-controls)に分けて保存しています。

## 性能

2026-10-24、Windows 11 build 26300.9711 / AMD Ryzen 7 5700X（8コア16論理CPU）で測定しました。Ruby **4.0.9**、RuboCop **1.92.1**、Parser **3.3.13.0**、rubocop-ast **1.50.0**を使用しています。最新 Prism 2.0.0 は rubocop-ast の `~>1.7` 制約に合わず、実ロードは **1.8.2**です。全構成要素を最新版にした計測は未完了で、以下は利用可能な組合せの結果です。YJITと本家の並列実行はこのWindows Rubyでは利用できません。

両者に同じ外部設定（`NewCops: disable` と実効対象版）を渡しました。cache有効で、coldは空の実パス、warmは対応するcold実行が保存したcacheを再利用します。各条件2回、先に実行するツールを交替し、全診断が各ツール自身の基準と一致することを確認しています。表は実時間の中央値です。時間比は RuboCop ÷ Sonicop で、**診断内容が異なるため、同じ仕事の速度倍率や以前のビルドからの改善率ではありません**。

| コーパス | ファイル | cache | RuboCop 単一(s) | Sonicop 単一(s) | 時間比 | Sonicop 並列(s) |
| --- | --- | --- | --- | --- | --- | --- |
| rails/rails | 3,588 | cold | 214.52 | 41.25 | 5.20 | 6.71 |
| rails/rails | 3,588 | warm | 16.19 | 1.36 | 11.95 | 0.83 |
| rubocop/rubocop | 1,807 | cold | 98.31 | 15.41 | 6.38 | 1.74 |
| rubocop/rubocop | 1,807 | warm | 4.98 | 0.31 | 16.18 | 0.14 |
| mastodon/mastodon | 3,354 | cold | 75.07 | 11.03 | 6.81 | 1.55 |
| mastodon/mastodon | 3,354 | warm | 10.62 | 0.66 | 16.14 | 0.34 |
| Homebrew/brew | 2,397 | cold | 78.93 | 13.23 | 5.96 | 1.92 |
| Homebrew/brew | 2,397 | warm | 7.60 | 0.60 | 12.67 | 0.33 |
| ruby/ruby | 7,522 | cold | 411.07 | 68.96 | 5.96 | 17.35 |
| ruby/ruby | 7,522 | warm | 27.58 | 5.62 | 4.90 | 4.43 |

既定有効は各394個ですが共通名は393個で、この測定では各版の有効集合をそのまま使います。対象版は Rails 3.3、Mastodon 4.0、残り2.7です。Ruby の計測複製では、本家を停止させる2ファイルだけを両者から除いて7,522対象を使います。探索時間、各回の最速値・最大値、子プロセスを含むuser/system CPU、hostの負荷、cache件数、終了コードは[生データ](docs/benchmarks/2026-10-24-windows.json)と[再現条件](docs/maintenance/2026-10-24.md)に記録しています。[2026-10-22 Linuxの数値](docs/benchmarks/2026-10-22.json)は異なるSHA・OSの過去記録です。

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
