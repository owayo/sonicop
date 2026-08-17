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
一覧はバイナリ自身が正本です。

```bash
# 認識済み Cop と実装状況の一覧
sonicop --show-cops
```

**RuboCop 1.89 の全 609 Cop を実装しています。** 本家のレジストリと名前まで一致しており、
ただしこれは**レジストリについての主張**であって、すべての設定についての主張ではありません。
`Style/HashSyntax` は存在し既定の届く範囲では一致しますが、`EnforcedShorthandSyntax` の
既定以外の 4 つの値はいずれも未実装です。**Cop は半分欠けたまま、既定の届く範囲では
一致し続けられます** — [CONFORMANCE.md](CONFORMANCE.md) の *Limits* を参照してください。
`Enabled: pending` の 159 個と `Enabled: false` の 56 個も含みます。この 215 個は本家でも
既定の実行では走らないので、`--only` で名指しするか設定で有効にしてください。本家にも
存在しない Cop 名だけをエラーにし、必要なら `--ignore-unrecognized-cops` で続行できます。

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
サーバートランスポート、Ruby プラグイン実行、キャッシュ再利用、カスタム Cop、実装済み以外の
Cop は実行しません。

そのうち大半は、その旨を出力します。`--server` / `--no-server` / `--lsp` / `--mcp` /
`--plugin` は stderr に 1 行の注記を出します。cache 系は何も出しませんが、
無言でよいのは片方だけです。

- `--cache=false` はキャッシュを使わないことを求めるもので、sonicop はそれを満たしているため、
  無言が正しい応答です。
- `--cache=true` はキャッシュ再利用を求めるものですが、sonicop はこれを提供しておらず、
  それでも無言です。

Cop の設定値も後者と同じ挙動です。sonicop が実装していない設定値 — たとえば
`Style/HashSyntax` の `EnforcedShorthandSyntax` — は**警告なしに無視されます**。
名前を綴り間違えた設定値も同様に無視されます。つまり
**offense が 0 件であることは、その設定が効いた証拠にはなりません**。
無視された設定値と、違反の無いファイルが、同じ出力になるためです。

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
**これが説明するのは不足の向きだけ**です。**過剰の向きは別の問題で、未調査です** —
Homebrew の 263 件はすべて `Lint/Syntax` であることまでは確認していますが、
移植版だけが構文エラーと呼ぶその中身は調べていません。autocorrect は RuboCop 自身のツリーと
Mastodon でバイト単位に一致します。この 2 つは死守ラインとして扱い、バイト一致が崩れた場合は
既知差分ではなく退行として直します。

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
| rubocop/rubocop | 1,765 | 11.49 秒 | **3.45 秒** | 42.49 秒 | **16.39 秒** |
| mastodon/mastodon | 3,290 | 27.26 秒 | **6.03 秒** | 42.07 秒 | **19.53 秒** |
| Homebrew/brew | 2,179 | 20.37 秒 | **5.36 秒** | 45.08 秒 | **16.03 秒** |
| rails/rails | 3,551 | 45.89 秒 | **18.96 秒** | 99.56 秒 | **44.61 秒** |
| ruby/ruby | 7,466 | 117.79 秒 | **54.97 秒** | 215.87 秒 | **100.62 秒** |

差は並列で 2.1〜4.5 倍、単一プロセスで 2.1〜2.8 倍と幅があり、1 コーパスでは代表できません。
**単一プロセスの列を読み、並列は目安として扱ってください。** 同じ 2 つのバイナリを 1 日に
3 回測ったところ、単一プロセスの値は毎回 16% 以内に収まったのに対し、RuboCop 自身のツリーでの
並列の倍率は、マシンが他に何をしていたかだけで 3.3 倍から 9.2 倍まで動きました。単一プロセスは
エンジンを測っていますが、並列はエンジンに加えて「その実行でスケジューリングがそのツリーに
どれだけ噛み合ったか」を測っています。

仕事を省いて速いわけではありません。この同じ 394 Cop について、RuboCop 自身のツリー・
Rails・Mastodon の 3 つで**すべての offense が一致**します（計 188,812 件、どちらの側にも
残りません）。autocorrect は前者と後者でバイト単位に一致します。

再現時に注意が必要な点が 2 つあります。RuboCop は **`--cache false` と併用すると
`--parallel` を黙って無効化します**。そのためここでの並列実行はキャッシュを有効にしたうえで
実行ごとにキャッシュディレクトリを消しており、`--cache false --parallel` で計測すると
単一プロセスを測ることになり差が過大に出ます。また RuboCop の既定は単一プロセス、
Sonicop は `--no-parallel` を渡さない限り並列です。

```bash
# RuboCop（並列・キャッシュは毎回空・既定の全 394 Cop）
rubocop --force-default-config --cache true --cache-root "$(mktemp -d)" \
        --no-color --parallel -f quiet

# Sonicop
sonicop --force-default-config --format quiet
```

測定機は Apple M2（8 コア）、Ruby 4.0.6（YJIT 利用可）、RubyGems 導入の RuboCop 1.89.0。
1 分平均のロードアベレージは、計測開始時が 3.6、終了時が 9.6 でした。**アイドル状態ではありません**。
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
