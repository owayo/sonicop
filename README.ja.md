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

Bundler、Gemspec、Layout、Lint、Metrics、Migration、Naming、Security、Style の各デパートメントの
Cop を実装しています。一覧はバイナリ自身が正本です。

```bash
# 認識済み Cop と実装状況の一覧
sonicop --show-cops
```

**RuboCop 1.89 の全 609 Cop を実装しています。** 本家のレジストリと名前まで一致しています。
`Enabled: pending` の 159 個と `Enabled: false` の 56 個も含みます。この 215 個は本家でも
既定の実行では走らないので、`--only` で名指しするか設定で有効にしてください。本家に存在しない
Cop 名を書いた場合だけエラーで止まります（`--ignore-unrecognized-cops` で続行できます）。

### Cop 別の一致状況

全 609 Cop を両者で有効にし、rubocop/rubocop・mastodon/mastodon・rails/rails・Homebrew/brew の
4 プロジェクト計 10,792 ファイルで比較した結果です。**完全一致**とは、その Cop の offense が
位置・メッセージ・重大度・修正可否まで 1 件残らず一致し、どちらにも余りがないことを指します。

<!-- conformance:start -->
| デパートメント | Cop 数 | 検証済み | 完全一致 | 相違 |
|---|---:|---:|---:|---:|
| Bundler | 7 | 3 | **3 ✓** | 0 |
| Gemspec | 10 | 4 | **4 ✓** | 0 |
| Layout | 100 | 84 | **84 ✓** | 0 |
| Lint | 157 | 80 | 78 | 2 |
| Metrics | 10 | 10 | **10 ✓** | 0 |
| Migration | 1 | 0 | 0 | 0 |
| Naming | 19 | 19 | **19 ✓** | 0 |
| Security | 7 | 6 | **6 ✓** | 0 |
| Style | 298 | 234 | 232 | 2 |
| **合計** | **609** | **440** | **436 (99.1%)** | **4** |
<!-- conformance:end -->

**先に読むべきは「検証済み」の列です。** このコーパスで一度も発火しなかった Cop は、
沈黙が一致と見分けられないため、一致にも相違にも数えていません。発火しなかった 169 個は
測定の外にあり、合格したわけではありません。割合を 609 ではなく 440 で割っているのはそのためです。

この差を埋めることが現在の目標です。**Cop 数・検証済み・完全一致がすべて 609 になること。**
実コードを足しても届きません。RuboCop が既定で無効にしている 56 Cop と、pending として
出荷している Cop の多くは、ツリーがどれだけ大きくても素の実行では発火しないためです。
そこに届くのは本家の spec が供給する入力の方で、`tests/fixtures/upstream_spec_capture.jsonl`
に記録した 44,070 ケースは **609 Cop すべてに到達します**（実測）。これを第 2 のコーパスとして
数えることが分母を動かす手段であり、「発火しなかった」を「一致した」と書かせないための
安全装置が「検証済み」の列です。

相違した 4 つは `Lint/Syntax`（1,275 箇所。すべて Homebrew のもので、構文エラーからの回復手順が
2 つのパーサで異なることによる差です。**どのファイルを構文エラーと判定するかは完全に一致**します）、
`Style/EmptyElse`（42）、`Style/DisableCopsWithinSourceCodeDirective`（3）、
`Lint/InterpolationCheck`（2）です。

設定値については別に測っています。既定値でしか一致しない Cop は半分しか実装していないのと
同じだからです。`Enforced*` 系の設定を持つ 111 Cop すべてを**既定以外の値**に倒して同じコーパスを
流すと、**622,317 件の offense のうち 99.995% が一致**し、発火した 96 Cop のうち 85 個が完全一致です。
残るのは 10 Cop（いずれも 17 件以下）と、本家がクラッシュして sonicop が正常に検出する 1 Cop です。
内訳は [CONFORMANCE.md](CONFORMANCE.md) にあります。

どちらの表も `scripts/conformance_table.rb` で再現できます。

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

# Cop／デパートメントを指定
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

既存コマンドとの互換性を保つため、server/LSP/MCP、plugin 系の引数も受理します。
サーバートランスポート、Ruby プラグイン実行、カスタム Cop、実装済み以外の Cop は実行しません。
これらはその旨を出力します。`--server` / `--no-server` / `--lsp` / `--mcp` / `--plugin` は
stderr に 1 行の注記を出します。

cache 系の引数は、受理するだけでなく実際に効きます。sonicop は独自の結果キャッシュを持ち、
検査時からサイズ・更新時刻・パーミッションのいずれも動いていないファイルには、
保存済みのレポートをそのまま返します。

- キャッシュは既定で有効です。`--cache false` で無効化できます。設定ファイルの
  `AllCops/MaxFilesInCache: 0` でも同じです。
- 置き場所は `--cache-root DIR` で指定します。省略時は `$XDG_CACHE_HOME/sonicop`、
  macOS では `~/Library/Caches/sonicop`、それ以外は `~/.cache/sonicop` です。
  `--cache-root` は `--cache false` とは併用できません。
- 保持するレポート数の上限は `AllCops/MaxFilesInCache` で、既定は本家と同じ 20,000 件です。
- autocorrect 実行、`--stdin`、`--profile`、`--memory` では読み書きしません。
- 本家のキャッシュとは共有しません。形式が別物であり、書いたときとまったく同じ
  ビルドの sonicop にしかエントリを返さないためです。

無言なのは Cop の設定値のほうです。sonicop が実装していない設定値は**警告なしに無視されます**。
名前を綴り間違えた設定値も同様です。つまり
**offense が 0 件であることは、その設定が効いた証拠にはなりません**。
無視された設定値と、違反の無いファイルが、同じ出力になるためです。
どの設定値まで検証済みかは上の *Cop 別の一致状況* を参照してください。

Cop の*名前*は検査されます。設定ファイルに未知の Cop 名があれば、実行はエラーで止まります。
素通りするのは、既知の Cop の中の設定値です。

### 適合性

実装済み Cop は、RuboCop 自身・Rails・Ruby・Homebrew・Mastodon の 5 プロジェクト
計 18,251 ファイルに対して、両者とも本家既定設定で検証しています。offense は Cop 名・パス・
行・桁・終端行・終端桁・長さ・メッセージ・重大度・修正可否のすべてで突き合わせます。

5 つのうち 3 つが**完全一致**です。RuboCop 自身のツリー（5,766 件）、Rails（167,760 件）、
Mastodon（15,286 件）で、過剰も不足もメタデータ差もありません。対象ファイル一覧は 5 つすべてで
**件数だけでなくパスまで**一致します（集合として比較。どの側にも余りはありません）。
残る差分は `Lint/Syntax` に集中しています。その大半は、本家の LALR パーサが
構文エラーから回復して出す追加診断を tree-sitter では再現できないことによるもので、
診断位置の差は不足と過剰の両方に出ます。Homebrew の不足 997 件・過剰 263 件はすべて
`Lint/Syntax` ですが、**構文エラーと判定したファイル集合は 569 対 569 で完全一致**し、
移植版だけが拒否したファイルは 0 件です。過剰 263 件は共有エラーファイル 135 件にあり、
すべて同じファイル内の共通診断より後ろにあるため、別の受理判定バグではなく回復後の診断位置差です。
Homebrew を問題の構文をサポートする Ruby 3.1 として測ると、両者とも `Lint/Syntax` は 0 件になります。
autocorrect は RuboCop 自身のツリーと Mastodon でバイト単位に一致します。この 2 つは死守ラインと
して扱い、バイト一致が崩れた場合は既知差分ではなく退行として直します。

コマンド、この数値を測ったコーパスのコミット、この種の計測が誤った結論を導く 2 つの罠は
[CONFORMANCE.md](CONFORMANCE.md) にまとめています。

### 性能

適合性検証に使う 5 コーパスすべてで測定しました。両者とも同梱の既定設定
（`--force-default-config`）で走らせているためプロジェクト側の `.rubocop.yml` は読まず、
どのコーパスでも**対象ファイル数は一致**しています。この計測に必要なのはそこまでで、
どちらの側も少なく検査してはいない、と言えます。パス単位の一致はより強い主張で、
上の*適合性*の節で 5 つすべてについて示していますが、それは固定したリビジョンでの測定であって
この速度計測の run そのものではありません。

両者とも既定の全 Cop で走らせています。**同じ 394 Cop** が名前まで一致しているため、
どちらも絞る必要がなく、素の実行がそのまま対等な比較になります。
（394 は 609 から `Enabled: pending` の 159 個と `Enabled: false` の 56 個を除いた残りで、
既定の実行はどちらの群にも届きません。）
各値は暖機後 2 回の最速値です。

| コーパス | ファイル | RuboCop 並列 | Sonicop 並列 | RuboCop 単一 | Sonicop 単一 |
|---|---:|---:|---:|---:|---:|
| rubocop/rubocop | 1,765 | 12.71 秒 | **4.59 秒** | 42.74 秒 | **13.75 秒** |
| mastodon/mastodon | 3,290 | 29.95 秒 | **6.48 秒** | 37.60 秒 | **15.88 秒** |
| Homebrew/brew | 2,179 | 18.20 秒 | **4.31 秒** | 38.72 秒 | **11.67 秒** |
| rails/rails | 3,551 | 52.55 秒 | **16.88 秒** | 162.55 秒 | **63.97 秒** |
| ruby/ruby | 7,466 | 132.17 秒 | **43.10 秒** | 199.78 秒 | **76.07 秒** |

差は並列で 2.8〜4.6 倍、単一プロセスで 2.4〜3.3 倍と幅があり、1 コーパスでは代表できません。
**単一プロセスの列を読み、並列は目安として扱ってください。** 同じ 2 つのバイナリを 1 日に
3 回測ったところ、単一プロセスの値は毎回 16% 以内に収まったのに対し、RuboCop 自身のツリーでの
並列の倍率は、マシンが他に何をしていたかだけで 3.3 倍から 9.2 倍まで動きました。単一プロセスは
エンジンを測っていますが、並列はエンジンに加えて「その実行でスケジューリングがそのツリーに
どれだけ噛み合ったか」を測っています。

仕事を省いて速いわけではありません。この同じ 394 Cop について、RuboCop 自身のツリー・
Rails・Mastodon の 3 つで**すべての offense が一致**します（計 188,812 件、どちらの側にも
残りません）。autocorrect は前者と後者でバイト単位に一致します。

再現時に注意が必要な点が 3 つあります。RuboCop は **`--cache false` と併用すると
`--parallel` を黙って無効化します**。そのためここでの並列実行はキャッシュを有効にしたうえで
実行ごとにキャッシュディレクトリを消しており、`--cache false --parallel` で計測すると
単一プロセスを測ることになり差が過大に出ます。また RuboCop の既定は単一プロセス、
Sonicop は `--no-parallel` を渡さない限り並列です。そして**両方ともキャッシュを空にする**
必要があります。Sonicop も既定でキャッシュするため、同じツリーを 2 回目に流すと自分の
キャッシュが答えてしまい、エンジンについては何も測れません。どちらにも使い捨ての
キャッシュディレクトリを渡してください。

```bash
# RuboCop（並列・キャッシュは毎回空・既定の全 394 Cop）
rubocop --force-default-config --cache true --cache-root "$(mktemp -d)" \
        --no-color --parallel -f quiet

# Sonicop（キャッシュは毎回空）
sonicop --force-default-config --cache-root "$(mktemp -d)" --format quiet
```

測定機は Apple M2（8 コア）、Ruby 4.0.6（YJIT 利用可）、RubyGems 導入の RuboCop 1.89.0。
1 分平均のロードアベレージは、計測開始時が 4.0、終了時が 3.1 でした。**アイドル状態ではありません**。
RuboCop 自身のツリーだけは後から単独で測り直しています（負荷 3.7 → 4.0）。1 回目はリリース
ビルドの終わりがけに走ってしまい、値が 60% ほど高く出たためです。**負荷の違う 1 行を同じ表に
並べることはできません**。
両者を同じ条件で測っているため倍率は保たれますが、秒数そのものは下限ではなく、静かなマシンなら
より速く出ます。コアを奪い合うものが動いていると両者とも膨らみ、その度合いは一致しません。それが
並列の列があれだけ動く理由です。秒数そのものが重要なときは、他に負荷のない状態で測り、**実行の
前後でロードアベレージを記録してください** — その情報が無い数値は、別の数値と比べられません。

## 開発

入口は `make` に一本化しています。`make help` で全ターゲットを確認できます。gem 配布の
タスクは Rakefile 側にあり、`make` から呼び出します。

```bash
make build   # デバッグビルド
make check   # fmt、clippy、Rust テスト、Ruby ラッパーテスト、バージョン整合
make gem     # source gem
```

### Cop の追加

Cop は `src/rules/<デパートメント>/<cop>.rs` の 1 ファイルで、公開するのは
`check(context, offenses)` の 1 関数だけです。登録はデパートメントの `mod.rs` に 1 行を足します。

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

`src/display_width_table.rs` も生成物で、コミットします。RuboCop は表示桁を
`unicode-display_width` gem で数えるため、この表は手書きせず gem から生成しています。
手書きの例外表は実際にずれており、NFD 分解された日本語でキャレットの本数が合わなくなっていました。
再生成は `ruby scripts/dump_display_width.rb > src/display_width_table.rs` で行い、
gem と Unicode のバージョンがファイル先頭に記録されます。

依存更新には `depup --install` を使います。Ruby grammar は再現可能性のため `Cargo.toml` で
fork のコミットを固定しています。

## ライセンス

[MIT](LICENSE)。同梱する RuboCop 既定設定とパーサー依存の著作権表示は
[NOTICE](NOTICE) および [`licenses/`](licenses/) に収録しています。
