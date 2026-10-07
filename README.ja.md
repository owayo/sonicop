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

**RuboCop 1.89 の全 609 Cop 名を登録しています。** 本家のレジストリと名前まで一致しています。プロジェクト索引が必要な３つの Cop は、索引を無効にした場合の動作のみ対応しており、索引を使った検出は未実装です。`Enabled: pending` の 159 個と `Enabled: false` の 56 個も含みます。この 215 個は本家でも既定の実行では走らないので、`--only` で名指しするか設定で有効にしてください。本家に存在しない Cop 名を書いた場合だけエラーで止まります（`--ignore-unrecognized-cops` で続行できます）。

### Cop 別の一致状況

2026-10-08 に RuboCop 1.89.0 と比較しました。本家 spec の記録から **42,435 入力・1,545 設定群**を実行し、設定群ごとの `TargetRubyVersion`、ファイル種別、Ruby の設定値の型を保っています。**完全一致**は開始・終端位置、length、メッセージ、重大度、修正可否、同じ記録の重複まで一致することを指します。異なる実行の差分は相殺しません。

別の６設定群・161 入力は、本家の spec helper が許す null の `EnforcedStyle` を CLI が拒否するため未測定です。表は測定できた入力の結果であり、すべての設定値を保証するものではありません。

<!-- conformance:start -->
| デパートメント | Cop 数 | 発火 | 完全一致 | 相違 |
|---|---:|---:|---:|---:|
| Bundler | 7 | 7 | **7 ✓** | 0 |
| Gemspec | 10 | 10 | **10 ✓** | 0 |
| Layout | 100 | 100 | **100 ✓** | 0 |
| Lint | 157 | 154 | 153 | 1 |
| Metrics | 10 | 10 | **10 ✓** | 0 |
| Migration | 1 | 1 | **1 ✓** | 0 |
| Naming | 19 | 19 | **19 ✓** | 0 |
| Security | 7 | 7 | **7 ✓** | 0 |
| Style | 298 | 298 | 296 | 2 |
| **合計** | **609** | **606** | **603** | **3** |
<!-- conformance:end -->

**先に読むべきは「発火」の列です。** 今回発火したのは 606 Cop です。`Lint/DeprecatedReference`、`Lint/NameTypo`、`Lint/UnusedPrivateMethod` は未実装のプロジェクト索引を必要とするため、完全一致に数えていません。測定できた中で差が残るのは `Lint/Syntax`、`Style/FormatStringToken`、`Style/RedundantArgument` です。

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

cache 系の引数は、受理するだけでなく実際に効きます。sonicop は独自の結果キャッシュを持ち、検査時からサイズ・更新時刻・パーミッションのいずれも動いていないファイルには、保存済みのレポートをそのまま返します。

- キャッシュは既定で有効です。`--cache false` で無効化できます。設定ファイルの `AllCops/MaxFilesInCache: 0` でも同じです。
- 置き場所は `--cache-root DIR` で指定します。省略時は `$XDG_CACHE_HOME/sonicop`、macOS では `~/Library/Caches/sonicop`、それ以外は `~/.cache/sonicop` です。`--cache-root` は `--cache false` とは併用できません。
- 保持するレポート数の上限は `AllCops/MaxFilesInCache` で、既定は本家と同じ 20,000 件です。
- autocorrect 実行、`--stdin`、`--profile`、`--memory` では読み書きしません。
- 本家のキャッシュとは共有しません。形式が別物であり、書いたときとまったく同じビルドの sonicop にしかエントリを返さないためです。

無言なのは Cop の設定値のほうです。sonicop が実装していない設定値は**警告なしに無視されます**。名前を綴り間違えた設定値も同様です。つまり **offense が 0 件であることは、その設定が効いた証拠にはなりません**。無視された設定値と、違反の無いファイルが、同じ出力になるためです。どの設定値まで検証済みかは [既定以外の設定値](docs/cop-conformance.ja.md#既定以外の設定値) を参照してください。

Cop の*名前*は検査されます。設定ファイルに未知の Cop 名があれば、実行はエラーで止まります。素通りするのは、既知の Cop の中の設定値です。

## 適合性

sonicop は仕様とする RuboCop 1.89.0 の Cop 名を **609 / 609** 登録しています。３つの Cop はプロジェクト索引を使った検出が未実装です。固定した 5 つの Ruby プロジェクト（対象 18,251 ファイル）で、対象パスの集合は両者で一致します。既定設定の JSON は RuboCop 自身（offense 5,766 件）、Rails（167,760 件）、Mastodon（15,286 件）で、位置と全項目まで一致しました。Homebrew の差は `Lint/Syntax` のエラー復帰後の位置だけで、構文エラーと判定する 569 ファイルの集合は一致します。Ruby にはパーサと Cop の差が残るため、完全一致には数えません。

autocorrect は両者が別々の複製を修正した結果をバイト単位で比較します。本家が有効な Ruby を構文エラーに変える、代入順によって値を失う、ローカル変数の参照を `NameError` になるメソッド呼び出しへ変える場合は、安全のため意図的に一致させません。既知差分は[マニフェスト](tests/conformance/known_divergences.yml)に修正後ファイルのハッシュで固定しています。本家が最後まで処理できる 4 コーパスでは、`-a`・`-A` とも新規差分は 0 件です。Ruby は本家が途中で停止するため、完全な修正後ツリーを比較できません。Homebrew の修正後ツリーは両モードともバイト単位で一致します。

コーパスのコミット、版、offense の比較項目、実測値と限界は [CONFORMANCE.md](CONFORMANCE.md) に記録しています。[Cop の適合表](#cop-別の一致状況)は別の計測で、発火 606 Cop、完全一致 603 Cop、差が残るもの３ Cop、索引が必要で未発火のもの３ Cop です。609 Cop すべての発火と完全一致は未達です。

## 性能

2026-10-08 に Apple M2（8 コア）上で、Ruby 4.0.7（YJIT 無効）、RuboCop 1.91.0、sonicop のコミット `ca1de57` を使って測定しました。[適合性](#適合性)の仕様は RuboCop 1.89.0 です。新しい本家には 613 Cop があり、sonicop は 1.89.0 の 609 Cop を実装しています。既定有効は両者 394 個ですが、互いに 1 個ずつ名前が異なります。速度測定では本家の `Lint/CopDirectiveSyntax`、sonicop の `Style/DoubleCopDisableDirective` を除き、**共通の 393 Cop** に揃えました。

両者に `--force-default-config` を付け、JSON 実行で対象パス集合と最後まで検査した件数を確認しました。Ruby では本家を停止させる 2 ファイルを両者の測定用複製から除き、7,466 対象のうち 7,464 ファイルを測りました。以下の秒数は、キャッシュを空にして 2 回走らせた**最速値 / 中央値**です。本家の並列実行は `--parallel --cache true` と毎回新しい実パスのキャッシュ、sonicop の並列実行も毎回新しいキャッシュを使い、毎回キャッシュのファイルが作られたことを確認しています。単一プロセスでは両者のキャッシュを無効にし、sonicop に `--no-parallel` を指定しました。速度測定の全実行は offense を報告して終了コード 1 を返しました。

| コーパス | リビジョン | ファイル | 本家 offense | sonicop offense | 本家 並列 | sonicop 並列 | 本家 単一 | sonicop 単一 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| rails/rails | `62b5458` | 3,551 | 167,899 | 167,760 | 31.91 / 32.15 | 7.01 / 7.56 | 307.00 / 308.05 | 59.37 / 59.72 |
| rubocop/rubocop | `f009b33` | 1,765 | 5,501 | 5,766 | 16.13 / 17.79 | 2.15 / 2.39 | 162.09 / 167.21 | 21.22 / 23.30 |
| mastodon/mastodon | `fad3685` | 3,290 | 15,292 | 15,286 | 15.84 / 15.99 | 2.41 / 2.44 | 119.31 / 120.57 | 22.62 / 22.97 |
| Homebrew/brew | `38ee325` | 2,179 | 49,926 | 49,326 | 16.12 / 16.22 | 2.37 / 2.40 | 127.63 / 129.47 | 19.39 / 20.09 |
| ruby/ruby | `3349f41` | 7,464 | 763,186 | 761,186 | 79.96 / 81.06 | 14.93 / 16.36 | 692.99 / 698.78 | 96.85 / 102.63 |

検出数も同じ Cop 選択と設定で採取しています。本家の新しい挙動によって仕事量が変わるため、この時間から同じ仕事をした場合の速度倍率は求められません。下の単一プロセスの CPU 時間を併せて読むと、エンジンの処理とスケジューリング待ちを区別できます。並列の実時間には OS のスケジューリングも影響します。

| コーパス | 本家 user CPU | sonicop user CPU | 本家 system CPU | sonicop system CPU |
| --- | --- | ---: | ---: | ---: |
| rails/rails | 162.10 / 162.52 | 31.83 / 32.09 | 31.94 / 32.05 | 6.43 / 6.45 |
| rubocop/rubocop | 88.46 / 91.19 | 11.41 / 12.31 | 14.13 / 15.11 | 2.26 / 2.35 |
| mastodon/mastodon | 64.74 / 65.36 | 11.97 / 12.02 | 12.59 / 12.59 | 2.61 / 2.63 |
| Homebrew/brew | 68.65 / 69.28 | 10.36 / 10.57 | 13.60 / 13.65 | 2.18 / 2.23 |
| ruby/ruby | 379.22 / 383.38 | 53.25 / 55.57 | 66.76 / 67.12 | 10.21 / 10.95 |

マシンは測定中も OS の別処理による負荷が高い状態でした。この条件で観測した時間であり、以前のビルドから速度が改善したことを示す値ではありません。ロードアベレージの記録は次のとおりです。

| コーパス | 開始時 | 終了時 | 記録した最大値 |
| --- | --- | ---: | ---: |
| rails/rails | 14.69 | 17.18 | 26.37 |
| rubocop/rubocop | 17.18 | 16.07 | 33.48 |
| mastodon/mastodon | 16.07 | 16.17 | 25.26 |
| Homebrew/brew | 16.17 | 19.35 | 34.06 |
| ruby/ruby | 16.98 | 17.21 | 48.19 |

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
