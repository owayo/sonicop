# 開発

README の[開発](../README.ja.md#開発)の節に書ききれない、sonicop を開発するときの注意をまとめます。入口は `make` に一本化していて、引数なしの `make` で全ターゲットが並びます。ツール (Rust と Ruby) の版は `mise.toml` が正本です。

## make と Rakefile

Rakefile には gem の組み立てと版の処理だけがあり、`make` から呼びます。`make gem` は source gem を、`make gem-platform GEM_PLATFORM=<platform>` はリリース版のバイナリを包んだ platform gem を作り、`make version-sync` は `lib/sonicop/version.rb` を作り直します。`make ci` の最後の `make gem-check` は source gem を作って `tmp/gems` に入れ、入れたコマンドを実行します。リリース版のビルドを含むので時間がかかります。

`make cop-coverage` は、手書きのテストが到達する Cop を数えます。`make spec-capture` と `make spec-fixtures` は本家 spec のケースと期待値を録り直すためのもので、上流のチェックアウトとこのリポジトリの外にある道具が要ります。渡し方は Makefile のコメントにあります。

## コーパスの検証と速度測定

`make corpus-check CORPUS=<名前>` は独立した複製で `-a` と `-A` を比較し、乖離マニフェストのパスと出力ハッシュ、Ruby の構文、二度目の `-A` で変更されないことを検査します。`GATE_CORPUS_ROOT`、`GATE_ORACLE_ROOT`、`OUT_DIR`、`SONICOP` には明示的なパスを渡してください。`MISE_ENV=conformance` で `mise.conformance.toml` の検証用 Python を使えます。**Corpus conformance** ワークフローは固定した公開コーパスと RuboCop 1.89.0 を準備します。Ruby の全コーパスは本家の補正が中断するため、この補正用マトリクスには含めません。検出比較に必要な分割実行は [CONFORMANCE.md](../CONFORMANCE.md) に記載しています。

`syntaxcheck.py <corpus> --candidate-only` は、完全な sonicop の補正記録と有効な原本の `.rb` を照合し、本家との一致を主張せずに安全性を検査します。ワークフローは、本家が完走しない固定 Ruby コーパス全体にこの検査を別途実行します。２者の構文分類では、正の完了件数と対象パス集合の一致が必要です。途中停止した本家の複製を、全体の補正済み出力とは扱いません。

`make benchmark ARGS="--output tmp/benchmark --repeats 2"` は固定した公開 5 コーパスを RuboCop 1.92.0 と比較します。入力はプロジェクトの外に置き、親の gemspec による対象 Ruby 版の変化を防ぎます。両 CLI の実効版が固定コーパスの期待値と一致することも検査します。**Benchmark** ワークフローは専用のランナーで測り、条件、生の JSON、終了コード、時間を artifact に保存します。計測する全実行で対象集合の検査完了と、それぞれの基準 JSON に対する全診断項目の一致を確認します。新版は速度の比較先で、挙動の仕様は引き続き 1.89.0 です。処理内容が異なるため、時間と併せて検出数も記録します。

## Cop の追加

Cop は `src/rules/<デパートメント>/<cop>.rs` の 1 ファイルで、公開するのは `check(context, offenses)` の 1 関数だけです。登録はデパートメントの `mod.rs` に 1 行を足します。

```rust
department_rules! {
    "Layout";
    line_length => ("LineLength", Convention),
}
```

Cop 名と既定 severity を書くのはこの 1 行だけです。Cop 本体では名前は暗黙で、`context.setting("Max")` が `Layout/LineLength: Max` を読み、`context.offense(message, range)` がその Cop の名前と設定済み severity で報告します。Cop が自分の名前を 2 度書ける設計では、レジストリと食い違っても型検査では捕まりません。

全ノード走査より `context.nodes_of("kind")` を優先してください。Cop は全ファイルに対して走るため、Cop ごとの全走査はファイル規模ではなく Cop 数に比例して重くなります。

## 版

バージョンの正本は `Cargo.toml` です。`lib/sonicop/version.rb` は `make version-sync` で生成してコミットします（gemspec がパッケージ時に読むため）。両者が食い違うと CI が落ちます。

## 上流から取り込むファイル

`config/default.yml` は上流 RuboCop から取り込んだものです。再取得は `scripts/sync_default_yml.sh <rubocop-version>` で行い、由来のバージョンがファイル先頭に記録されます。

`src/display_width_table.rs` も生成物で、コミットします。RuboCop は表示桁を `unicode-display_width` gem で数えるため、この表は手書きせず gem から生成しています。手書きの例外表は実際にずれており、NFD 分解された日本語でキャレットの本数が合わなくなっていました。再生成は `ruby scripts/dump_display_width.rb > src/display_width_table.rs` で行い、gem と Unicode のバージョンがファイル先頭に記録されます。

## 依存

依存更新には `depup --install --include-pinned` を使います。再現可能性のため、パーサとプロジェクト索引の依存は `Cargo.toml` で版または fork のコミットを固定しています。更新による挙動の変化は、採用前に仕様版の RuboCop と比較してください。
