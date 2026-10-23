# Ruby の静的正規表現コンパイラ

RuboCop 1.89.0 の parser builder は、静的な Regexp の各 String を結合し、
符号化オプションで変換してから Ruby の `Regexp.new` へ渡す。
この adapter は、検証基準の Ruby 4.0.8 と同じ Ruby fork の Onigmo と Unicode 17.0 を
静的リンクする。独立版 Onigmo / Oniguruma の構文・Unicode 対応へ置換しない。

## 出典

- Ruby: <https://github.com/ruby/ruby/tree/60c3d71bfbf9790229c682e26f8ec968f078c7da>
- 独立した hash table のみ Onigmo: <https://github.com/k-takata/Onigmo/tree/1d7ee878b3e4a9e41bf9825c937ae6cf0a9cd68c>
- `PROVENANCE.json` は各 vendor file の元のパス・commit・SHA-256 を記録する。
  vendor 原本は変更せず、生成済み Unicode/JIS 表も同じ commit のものを使用する。

Ruby 自身の `re.c` / `string.c` から、escape の前処理と RegexpError の表示関数を
`build.rs` が決めた境界で抽出する。`standalone/ruby_bytes.h` はこの範囲だけが使う
String/Encoding API の byte buffer adapter で、VM・GC・Ruby の header へ依存しない。
表示関数の長さ引数だけは Ruby String の `long` から Rust slice の `size_t` に合わせる。
Windows LLP64 でも暗黙の 32-bit 切り詰めをせず、escape/表示の本体は原文を保つ。
`regparse.c` は生成したコピーで、Ruby 固有の捕捉名規則（WORD に限定しない）と
`ESC_U_HEX4` 無効を選ぶ二つの条件だけを切り替える。元 vendor は変更せず、
抽出・切替の境界が変わったときは build を失敗させる。他の `RUBY` 分岐は VM の
header・警告・GC・検索時の timeout であり、コンパイルの文法へ置換しない。
Onigmo の内部コンパイラ・解放が再帰するため、専用 32 MiB worker 上で直列化する。
検索や Ruby スクリプトの実行はしない。

## 再現と更新

ビルド時に Ruby、bindgen、configure、ネットワーク、target executable の実行は不要。
`cc` が target compiler を選び、Cargo の target ABI から設定 header を生成する。
エクスポートは `rename.h` で分離する。vendor 更新後に開発環境で静的 archive をビルドし、
`ruby scripts/regexp_validation/generate_renames.rb <archive>` で名前の一覧を更新する。

符号化変換表は `ruby scripts/regexp_validation/generate_encoding_tables.rb` で生成する。
Ruby 4.0.8 の厳密な EUC-JP / Windows-31J 変換と逆変換を全 BMP scalar について保存する。
実行時の Ruby と OS 共有 codec は使用しない。

回帰記録は `ruby scripts/regexp_validation/record.rb` が実 RuboCop 1.89.0 の CLI を
起動して生成する。通常の Rust テストはこの記録を読み、Ruby のない PATH で診断全項目、
終了値、補正後 byte を比較する。CRLF は既登録の意図的な改行保存差だけを照合時に戻す。
現統合は Target 2.7–3.4 の whitequark と Target 3.3–3.4 の Prism が対象。
Target 2.0–2.6 の旧 fallback を完全一致として数えない。

## ライセンスと配布

Onigmo: BSD 2-Clause、Ruby の抽出範囲: Ruby が選択を許す 2-clause BSDL、
独立版の `st.c` / `st.h`: public domain。元の著作権表示を保持する。
完全な文書は `licenses/ONIGMO.txt` と `licenses/RUBY_{BSDL,COPYING,LEGAL}.txt`、
追加の著作権表示は `licenses/RUBY_REGEXP_COPYRIGHTS.txt` に収録する。
source gem は全 vendor・adapter・出典を含み、platform gem も上記 licenses と NOTICE を含む。
