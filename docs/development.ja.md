# 開発

README の[開発](../README.ja.md#開発)の節に書ききれない、sonicop を開発するときの注意をまとめます。入口は `make` に一本化していて、引数なしの `make` で全ターゲットが並びます。ツール (Rust と Ruby) の版は `mise.toml` が正本です。

## make と Rakefile

Rakefile には gem の組み立てと版の処理だけがあり、`make` から呼びます。`make gem` は source gem を、`make gem-platform GEM_PLATFORM=<platform>` はリリース版のバイナリを包んだ platform gem を作り、`make version-sync` は `lib/sonicop/version.rb` を作り直します。`make ci` の最後の `make gem-check` は source gem を作って `tmp/gems` に入れ、入れたコマンドを実行します。リリース版のビルドを含むので時間がかかります。

`make cop-coverage` は、手書きのテストが到達する Cop を数えます。`make spec-capture` と `make spec-fixtures` は本家 spec のケースと期待値を録り直すためのもので、上流のチェックアウトとこのリポジトリの外にある道具が要ります。渡し方は Makefile のコメントにあります。

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

依存更新には `depup --install` を使います。Ruby grammar は再現可能性のため `Cargo.toml` で fork のコミットを固定しています。
