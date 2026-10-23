# parser gem と Racc の由来

RuboCop 1.89.0 の parser 3.3.12.0、RuboCop AST 1.50.0 の Builder、Racc 1.8.1 を使う診断バックエンド。
既定では TargetRubyVersion 2.7〜3.2、`ParserEngine: parser_whitequark` では 3.3/3.4 も使う。
公開 AST は作らず、診断と字句状態に必要な中間ノードを保持する。

- `tables/*.rs` は `ruby scripts/legacy_parser/generate.rb` で生成する。generator は gem と
  Racc の版を固定し、各入力 SHA-256 と還元動作本文から計算した Action ID を出力する。
  生成後に rustfmt を適用する。生成表を手で変更しない。
- `parser.rs` は `racc-1.8.1/ext/racc/cparse/cparse.c` の shift/reduce と error recovery を移した。
  Ruby 版 Racc は error の shift の扱いが異なるので仕様に使わない。
- `lexer/` は parser の Lexer / LexerStrings / Literal / StaticEnvironment を移した。
  `tables/lexer_exec.rs` は `scripts/legacy_parser/lexer_translate.rb` が Ruby の実行部を翻訳する。
- `actions.rs` と `builder.rs` は parser の各 grammar reduction と Builders::Default の診断・
  静的環境・範囲に必要な処理を移した。RuboCop::AST::Builder の string_value override も使い、
  既定 builder の invalid encoding 診断を独自に加えない。版の更新で Action ID が変われば手実装も見直す。
- parser の完全な MIT 許諾文と著作権表示は `licenses/PARSER.txt`、Racc の BSDL は
  `licenses/RACC.txt` に同梱する。source/native gem とも licenses glob で収録する。

入力ファイルの SHA-256:

| 入力 | SHA-256 |
|---|---|
| `parser-3.3.12.0/lib/parser/lexer-F1.rb` | `0ec464f012ea5a8c40ffdccb7dc5e2155c525c811f0f3641035996280fdad2d2` |
| `parser-3.3.12.0/lib/parser/lexer-strings.rb` | `6f0c912b71462338e8502a80e76a02e04cda5d80e97219b984ad7e89bef023e4` |
| `parser-3.3.12.0/lib/parser/builders/default.rb` | `df74c093bbfceeb6491ca3fe7e638c1405afbfc8cc99f2d571357379710a1219` |
| `parser-3.3.12.0/lib/parser/base.rb` | `00b229cdadbc80e172febf887a4fd8ef024fd5419d9ca2831db6dd225bd0d575` |
| `parser-3.3.12.0/lib/parser/static_environment.rb` | `aab0233640b6199ea22dea9f8a7ac1cf1bae091bb891ee6d867f7094bc78fc2c` |
| `racc-1.8.1/ext/racc/cparse/cparse.c` | `8b044b5a04f6963d289409705b0cbf6a6859f49d693c980c22b1f6b14c0d556c` |
| `parser-3.3.12.0/lib/parser/ruby27.rb` | `d188c3d4c07c5985dffd6aaac4232ceed5e75df8b01d5bf57a83c244900fbebe` |
| `parser-3.3.12.0/lib/parser/ruby30.rb` | `479792311c8b717fcaee278a0fdbe98335557e4a2ac8a4da9d5bc762eebd3902` |
| `parser-3.3.12.0/lib/parser/ruby31.rb` | `fb90bba48db249c33b40bd1243084fcd73ee9842a86881024d6603a3111dad04` |
| `parser-3.3.12.0/lib/parser/ruby32.rb` | `555871b34282dfc42241b7c2a0539634a18dbabeaf76653af1927cca19177d7c` |
| `parser-3.3.12.0/lib/parser/ruby33.rb` | `7fef62ed31c2ceff535c62c916e3a6db015399957deb35cf91741f4de75ee2d0` |
| `parser-3.3.12.0/lib/parser/ruby34.rb` | `6e415e98dcaff14dbbde746fe5086d3129e2226d24c22e8164462952b3350d56` |
| `rubocop-ast-1.50.0/lib/rubocop/ast/builder.rb` | `c66b4e9c5f5b8ddd8af742c3111554b6f4454314351b96bd7818f74a229c1362` |
