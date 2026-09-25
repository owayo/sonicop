<p align="center">
  <img src="docs/images/sonicop_logo_header.png" width="128" alt="sonicop">
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

対象ファイルごとに `.rubocop.yml` を解決するため、1 回の実行でもサブディレクトリ設定が適用されます。ローカル／HTTPS の `inherit_from`、`inherit_gem`、`inherit_mode`、`AllCops/DisabledByDefault`、`Include`／`Exclude`、Cop ごとの `Enabled`、`Exclude`、`Severity`、`Safe`、`SafeAutoCorrect` と設定値に対応します。宣言されたプラグイン由来の Cop は「認識済み・未実装」として受理し、Ruby プラグインコード自体は実行しません。リモート設定のリクエストには 30 秒のネットワークタイムアウトを設け、応答は 1 件あたり 5 MiB に制限します。

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

実装済み Cop は、RuboCop 自身・Rails・Ruby・Homebrew・Mastodon の 5 プロジェクト計 18,251 ファイルに対して、両者とも本家既定設定で検証しています。offense は Cop 名・パス・行・桁・終端行・終端桁・長さ・メッセージ・重大度・修正可否のすべてで突き合わせます。

5 つのうち 3 つが**完全一致**です。RuboCop 自身のツリー（5,766 件）、Rails（167,760 件）、Mastodon（15,286 件）で、過剰も不足もメタデータ差もありません。対象ファイル一覧は 5 つすべてで**件数だけでなくパスまで**一致します（集合として比較。どの側にも余りはありません）。残る差分は `Lint/Syntax` に集中しています。その大半は、本家の LALR パーサが構文エラーから回復して出す追加診断を tree-sitter では再現できないことによるもので、診断位置の差は不足と過剰の両方に出ます。Homebrew の不足 997 件・過剰 263 件はすべて `Lint/Syntax` ですが、**構文エラーと判定したファイル集合は 569 対 569 で完全一致**し、移植版だけが拒否したファイルは 0 件です。過剰 263 件は共有エラーファイル 135 件にあり、すべて同じファイル内の共通診断より後ろにあるため、別の受理判定バグではなく回復後の診断位置差です。Homebrew を問題の構文をサポートする Ruby 3.1 として測ると、両者とも `Lint/Syntax` は 0 件になります。autocorrect は RuboCop 自身のツリーと Mastodon でバイト単位に一致します。この 2 つは死守ラインとして扱い、バイト一致が崩れた場合は既知差分ではなく退行として直します。

コマンド、この数値を測ったコーパスのコミット、この種の計測が誤った結論を導く 2 つの罠は [CONFORMANCE.md](CONFORMANCE.md) にまとめています。

## 性能

適合性検証に使う 5 コーパスすべてで測定しました。両者とも同梱の既定設定（`--force-default-config`）で走らせているためプロジェクト側の `.rubocop.yml` は読まず、どのコーパスでも**対象ファイル数は一致**しています。この計測に必要なのはそこまでで、どちらの側も少なく検査してはいない、と言えます。パス単位の一致はより強い主張で、上の[適合性](#適合性)の節で 5 つすべてについて示していますが、それは固定したリビジョンでの測定であってこの速度計測の run そのものではありません。

両者とも既定の全 Cop で走らせています。**同じ 394 Cop** が名前まで一致しているため、どちらも絞る必要がなく、素の実行がそのまま対等な比較になります。（394 は 609 から `Enabled: pending` の 159 個と `Enabled: false` の 56 個を除いた残りで、既定の実行はどちらの群にも届きません。）各値は暖機後 2 回の最速値です。

| コーパス | ファイル | offense | RuboCop 並列 | Sonicop 並列 | RuboCop 単一 | Sonicop 単一 |
|---|---:|---:|---:|---:|---:|---:|
| rubocop/rubocop | 1,780 | 5,826 | 10.85 秒 | **1.53 秒** | 41.44 秒 | **8.96 秒** |
| mastodon/mastodon | 3,292 | 15,293 | 22.59 秒 | **3.04 秒** | 34.74 秒 | **6.86 秒** |
| Homebrew/brew | 2,296 | 51,527 | 13.56 秒 | **2.21 秒** | 40.36 秒 | **6.51 秒** |
| rails/rails | 3,562 | 168,615 | 32.90 秒 | **8.84 秒** | 85.71 秒 | **19.17 秒** |
| ruby/ruby | 7,477 | 765,975 | 86.89 秒 | **15.59 秒** | 193.59 秒 | **37.98 秒** |

差は並列で 3.7〜7.4 倍、単一プロセスで 4.5〜6.2 倍と幅があり、1 コーパスでは代表できません。**単一プロセスの列を読み、並列は目安として扱ってください。** 同じ 2 つのバイナリを 1 日に 3 回測ったところ、単一プロセスの値は毎回 16% 以内に収まったのに対し、RuboCop 自身のツリーでの並列の倍率は、マシンが他に何をしていたかだけで 3.3 倍から 9.2 倍まで動きました。単一プロセスはエンジンを測っていますが、並列はエンジンに加えて「その実行でスケジューリングがそのツリーにどれだけ噛み合ったか」を測っています。

仕事を省いて速いわけではありません。この同じ 394 Cop について、表のどのコーパスでも **offense の総数が一致**し、RuboCop 自身のツリーと Mastodon ではその 1 件ずつが同じ位置・同じメッセージ・同じ severity です。Rails は 168,615 件のうち 2 件だけ食い違います（Sonicop が出す `Style/CaseLikeIf` 1 件と、出さない `Metrics/AbcSize` 1 件）。RuboCop 自身のツリーでは 1 件の `correctable` フラグが違います。autocorrect は前者と後者でバイト単位に一致します。

再現時に注意が必要な点が 4 つあります。RuboCop は **`--cache false` と併用すると `--parallel` を黙って無効化します**。そのためここでの並列実行はキャッシュを有効にしたうえで実行ごとにキャッシュディレクトリを消しており、`--cache false --parallel` で計測すると単一プロセスを測ることになり差が過大に出ます。また RuboCop の既定は単一プロセス、Sonicop は `--no-parallel` を渡さない限り並列です。そして**両方ともキャッシュを空にする**必要があります。Sonicop も既定でキャッシュするため、同じツリーを 2 回目に流すと自分のキャッシュが答えてしまい、エンジンについては何も測れません。どちらにも使い捨てのキャッシュディレクトリを渡してください。最後に、そのキャッシュディレクトリは**実パス**である必要があります。macOS の `mktemp -d` は `/var/folders/…` を返し、その `/var` は symlink なので RuboCop はそこを拒み、キャッシュ無しで走ってしまいます。

```bash
# RuboCop（並列・キャッシュは毎回空・既定の全 394 Cop）
root=$(mktemp -d /private/tmp/bench.XXXXXX)
rubocop --force-default-config --cache true --cache-root "$root" \
        --no-color --parallel -f quiet

# Sonicop（キャッシュは毎回空）
sonicop --force-default-config --cache-root "$root" --format quiet
```

キャッシュの書き込みもこの数値に含まれており、無料ではありません。索引は全 offense を「見つかった行のテキスト」付きで保持するため、`ruby/ruby` では 336 MB になります。2 回目の実行（キャッシュ命中）は `ruby/ruby` で 1.59 秒、Rails で 0.42 秒です。

測定機は Apple M2（8 コア）、Ruby 4.0.6（YJIT 利用可）、RubyGems 導入の RuboCop 1.89.0。2026-08-31 に、`rubocop_rubocop` 2693129 / `mastodon_mastodon` b59ddc7 / `Homebrew_brew` b42173b / `rails_rails` a19f07f / `ruby_ruby` 22e4a75 の各リビジョンに対して測定しました。1 分平均のロードアベレージは各行の測定時点で 4.5〜7.1 で、その大半は RuboCop 自身の並列ワーカーです（計測する以上避けられません）。**アイドル状態ではありません。** 各コーパスで両者を連続して同じ条件で測っているため倍率は保たれますが、秒数そのものは下限ではなく、静かなマシンならより速く出ます。コアを奪い合うものが動いていると両者とも膨らみ、その度合いは一致しません。それが並列の列があれだけ動く理由です。秒数そのものが重要なときは、他に負荷のない状態で測り、**実行の前後でロードアベレージを記録してください** — その情報が無い数値は、別の数値と比べられません。

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
