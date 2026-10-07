# Prism 1.8.1

`vendor/src` と `vendor/include` は公式 `ruby-prism-sys` 1.8.1 の crates.io 配布物に含まれる Prism C ソースをそのまま収録している。ライセンスは `LICENSE.md`。Ruby プロセスや libclang はビルドにも検査にも不要。

RuboCop 1.89.0 が使う Prism の診断を固定するため、この版を更新しない。`rename.h` は未加工の静的ライブラリの外部 `pm_` シンボルから生成した名前の置換表で、関数と大域変数の両方を分離する。元の C ファイルは変更せず、ビルド時にこのヘッダーを先に読む翻訳単位を生成する。
