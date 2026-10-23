# 正規表現の静的判定と名前付き capture

仕様は RuboCop 1.89.0 の実 CLI 出力。`=~` の左辺が regexp リテラルでも、
本家の AST が常に `match_with_lvasgn` になるわけではない。

| 対象と engine | 文字列だけの補間 | 空の補間 | 動的な式の補間 |
| --- | --- | --- | --- |
| 2.7 の default / 3.2 の whitequark | match_with_lvasgn | match_with_lvasgn | send |
| 3.3 の default / 明示 whitequark | send | send | send |
| 3.4 の whitequark / Prism | send | send | send |

補間がない regexp は、これらの条件すべてで `match_with_lvasgn` になる。
`parser_whitequark` を明示しても、3.3 以上なら補間を含む regexp は send である。
parser gem の `static_regexp_node` が `version >= 33` で分岐するためである。

`src/regexp_ast.rs` の `Semantics::is_static` がこの AST 分類を共用する。
SpaceAroundOperators / InverseMethods / SelectByRegexp / OutOfRangeRegexpRef /
IfUnlessModifier / AmbiguousRegexpLiteral と Metrics が同じ条件を見る。
OutOfRangeRegexpRef の状態更新も send と match_with_lvasgn で分ける。
補間により capture 数を計算できない match_with_lvasgn は、直前の状態を保持する。

名前付き capture が後ろの `y +1` の読み方まで変えるため、検査本体と回復用断片の parse にも
選択した版と engine を渡す。VariableForce と Metrics の変数解析も同じ名前抽出を使う。
通常の `parser::parse` は、設定を持たない内部の構文確認用として従来の入口を保つ。
旧 parser の構文診断は別の状態を使う。whitequark と同じ捕捉名を宣言して構文の受理を合わせても、
それをそのまま物理 tree や cop の変数束縛へ配らない。診断用状態と、下記の安全な束縛の契約を分ける。

## 本家に追従すると呼び出しを壊す形

Ruby 本体でも版によって挙動が変わる。Ruby 2.7.8 を公式ソースから構築して実行し、
`/#{'a'}(?<y>.)/` は本体でも y を宣言することを確認した。
Ruby 4 の実行結果を、対象 Ruby 2.7 の補正が壊れる証拠にしてはならない。

一方、whitequark の `static_string` は空 begin も空文字列へ変え、複文の **全 str** を連結する。
Ruby 2.7 の `new_evstr` / `block_append` / `new_regexp` は同じ処理ではない。
空 begin は EVSTR に残り、値だけの先行 str は捨てられる。

```ruby
def y(value)
  value
end
/(?<y>.)#{}/ =~ 'ab'
p y +1
```

この原本は Ruby 2.7.8 で `1` を出力して終了 0。本家 whitequark / TargetRubyVersion 2.7 の
`--only Layout/SpaceAroundOperators -a` は `p y + 1` に変え、終了 0 を返す。
その補正後は **同じ Ruby 2.7.8** でも y() の引数不足で ArgumentError / 終了 1 になる。
構文の検査だけでは防げない。

そこで `Semantics::named_groups` は、whitequark の全 str 連結パターンと、Ruby 2.7 の畳み込みを
使ったパターンから、それぞれ独立に capture 名を抽出し、両方にある名前だけを宣言する。
本家側の出現順と重複の除去を保つ。畳み込みは Str / EmptyBegin / Block の小さな構造で表し、
`block_append` が str を捨てる場合と、空括弧を含む block を残す場合を分ける。
先行空文、途中の空括弧を含む begin と、捨てた str にしかない capture も
新しい架空の束縛にしない。

これは Ruby 全版の実行時 AST を完全再現する宣言ではない。本家だけにある架空の束縛を増やさず、
安全に共通する名前の対応を進めるための制限である。
差分は `tests/conformance/known_divergences.yml` の `regexp_binding_divergences` に登録する。
回帰では元の本家出力との不一致が残ることも確認し、同じ入力を実 Prism で読んだ別の記録と比較する。
候補の出力を期待値へ写さず、差分を一致件数に数えない。

文字列の復号では raw byte escape と Unicode escape の符号化を保持する。
ASCII-8BIT 原本の通常文字は元の byte に戻すが、Unicode escape の部品は UTF-8 として連結する。
原本の符号化を結合後にもう一度適用すると、後ろの識別子と別の捕捉名を同一視してしまう。

## character の構文木の欠落

[tree-sitter-ruby #14](https://github.com/owayo/tree-sitter-ruby/issues/14) の
`?\C-\x43` は、8 byte の character を 5 byte の character と後続 ERROR に分ける。
このまま束縛を落とすと、正当な capture をメソッド呼び出しへ変える補正になる。
正規表現の静的補間内では、原本の character を明示的なループで字句解析して消費範囲を回復する。
C/M の重複は拒否し、再帰を使わない。両モデルの消費終端が同じ場合だけ、終端内の artifact をまとめる。
終端をまたぐノード、後続の副作用、未知の ERROR、切れた escape は静的値に読み替えない。

nested long/short control、raw control、Binary meta、括弧・複文・動的な式の 9 原本を
LF/CRLF と `-a`/`-A` で比較した 36 条件は、診断・終了値・補正バイトが本家と一致し、
補正前後を同じ MRI 2.7.8 で実行した終了値と標準出力も変わらない。
これは物理 tree 全体の文法修正ではない。capture 解析の範囲を回復する局所処理であり、
他の cop が見る character ノードの範囲まで本家と一致したという主張はしない。
## 無効な character の既存境界

`?\C-\x` の切れた hex は実 MRI 2.7.8 が `invalid hex escape` / 終了 1 で拒否するが、
RuboCop 1.89.0 の 2.7/default (whitequark) は受理して後続の `y +1` を補正する。
物理 lexer が有効な character を確定できないため、Sonicop は静的 literal として分類せず原本を保つ。
LF/CRLF と `-a`/`-A` の 4 条件は変更前の parser-backend baseline でも同じで、今回の新退行ではない。
実本家の全診断・終了値・正規化しない書き込み bytes は `regexp_character_boundary.json`、
理由と候補の原本保持方針は manifest の `regexp_character_boundary_divergences` に記録する。
この 4 条件は通常の 3,768 条件にも安全 binding の 72 条件にも加算しない。
再記録は `python3 scripts/record_regexp_semantics.py --character-boundary-only`。
## 捕捉名の符号化と物理的な参照

捕捉名は、表示上の文字列だけでなく compiled regexp の byte と effective encoding で比較する。
本家の Buffer は Binary 以外を UTF-8 にするが、元 EUC-JP の `/e` や Windows-31J の `/s` は
その符号化の捕捉 Symbol を作る。後ろの識別子は本家では method、物理 MRI では local になり得る。
逆に `(?<\u3042>x)` は UTF-8 の捕捉 Symbol を作るため、元 EUC-JP/Windows-31J の識別子は
物理 MRI では別 Symbol の method、本家では同じ UTF-8 local になり得る。
ASCII の名前は US-ASCII Symbol として両方に一致する。flag だけから名前の符号化を決めない。

旧構文診断は `String#encode`→`Regexp.new` の本家状態を保持する。cop の通常束縛は従来の
#31 の畳み込み積集合を保ち、物理 literal が有効と確かめられたときだけ、元符号化の実際の
捕捉 Symbol にも存在する名前へ絞る。非補間にもこの同一性検査が必要である。
物理的に無効、未対応の符号化、値の復号・資源不足による判定不能を区別し、後二者を
「正常にコンパイルして名前が無かった」と扱わない。これらでは既存診断の fallback を保ち、
無効・未対応入力の意味保存を新たに保証しない。AST の cop dispatch は引き続き本家の条件で決める。

`Lint/AmbiguousOperator` / `Lint/AmbiguousRegexpLiteral` が実際に適用する括弧の開始置換だけを、
全文・scope・宣言順を保った物理変数解析で検査する。capture の符号化差で実 local だった参照を
method に変える場合は、ファイル全体を最初の原本 byte と未補正診断へ戻し、理由と終了 2 を返す。
`corrected` は衝突で捨てた編集にも立つため、最終 Action 木に残った置換だけを使う。
先行する空白補正の次の pass で初めて括弧が残る場合も、最初の原本まで戻す。
receiver、明示 call、宣言前、別 method scope、block 引数、先行する通常代入を区別し、
無害な空白・コメントの補正をファイルの符号化だけで拒否しない。一般の全 cop の意味保存を保証する機能ではない。

実 MRI 2.7.8 の Symbol と whitequark 宣言を 280 条件で別記録し、物理 regexp の直接検査は
そのうち非補間 190 条件を使う。`capture_parentheses_reports.json` は実本家の 174 条件を保持する。
36 危険/衝突条件、90 無害対照、48 逆方向 Symbol 条件であり、LF/CRLF と inspect/-a/-A を含む。
危険原本の評価は String x、本家の元符号化復元後は NoMethodError、逆方向の自作 method 対照は
原本 Integer 1、本家の元符号化復元後は ArgumentError になる。取得ソースのコードは実行していない。
実行時の意味保存証拠は使用した MRI の版に限る。特に 3.3 以上の補間を MRI 2.7 の宣言規則で
評価した結果を、その新しい対象版の破壊の証明へ読み替えない。

## 既存 gate が覆っていた物理側だけの捕捉 local

この安全性漏れは [Issue #59](https://github.com/owayo/sonicop/issues/59) で追跡する。

`discarded_extended`、`encoding_meta_hex_True`、`encoding_meta_octal_True` は、本家に捕捉名がなく
物理 MRI にだけ local がある逆向きの差である。前者の実 pattern は `#(?<y>.)` で、
本家が全 str をつなぐ `(?x)#(?<y>.)` とは異なる。後二者の実捕捉名は Binary byte C3A9。
本家と親版の `y(+1)` / `é(+1)` は明示 method call であり、この guard の正しい拒否対象になる。
元の `+1` は match すると String+Integer の TypeError なので、`+''` の自作対照でも検査した。
実 MRI2.7.8 では原本 String byte78、本家/親の補正後 NoMethodError になる。
fold bytes の一致を guard の条件にするとこの危険を再許可するため、その制約は置かない。

元の 3,768 条件の本家 raw 記録は改変しない。旧 #31 の72条件とは別に、3 IDs ×
Target2.7/3.2 × LF/CRLF × -a/-A の24条件だけを専用 metadata で登録し、最初の inspect 診断と
原本 byte、拒否理由、終了2を検査する。これらを除く通常本家一致は3,672条件となる。
Target3.2 は公式 MRI3.2.0（revision a528908271）でも独立に144実行と直接最小18条件を確認し、
原本/rollback String byte78・旧補正 NoMethodError が再現した。MRI2.7の測定とは別に保存する。
最小の実本家出力と各環境の実行値は `regexp_capture_fold_safety.json` に保持する。

## 再現

```sh
python3 scripts/record_regexp_semantics.py
cargo test --test regexp_semantics
python3 scripts/record_regexp_bindings.py --ruby27 /path/to/ruby-2.7.8/miniruby
cargo test --lib regexp_ast
make ci
```

記録するのは各 engine / 版、LF / CRLF、検査 / `-a` / `-A` の実出力。
同じ設定のファイルだけを一つの CLI 実行にまとめ、診断の全項目、正の検査完了件数、対象パス集合、
実 CLI の終了値とディスク上の補正バイトを照合する。
本家が補正時に CRLF を LF へ変える既知の差だけ、候補が CRLF を保つ形式へ戻して比較する。

実 Ruby 2.7.8 の構造監査は、5 つの str / 空括弧 / 先行空文を含む括弧を 0〜3 式並べ、
先頭と末尾のセミコロン、0〜2 重の括弧を組み合わせた 1,872 条件（1,870 個の原本）を使う。
1 プロセス内の独立した匿名 lambda ごとに `defined?(y)` を記録し、外側へ束縛が漏れないことも確認する。
234 条件が宣言し、1,638 条件は宣言しない。この実出力を `regexp_ruby27_bindings.json` へ保存し、
過剰な宣言も正当な宣言の欠落も同じ回帰で拒む。通常のテストに旧 Ruby の実行環境は要らない。
character の nested control escape `?\C-\x43` を第 6 の式に加えた構造監査は
3,108 条件へ raw control/meta terminal の 24 条件を加えた 3,132 条件（3,130 個の原本）で、528 条件が宣言し、2,604 条件は宣言しない。
この集合は先の 1,872 条件を含むため、独立した 4,980 入力とは数えない。

literal 値の監査は UTF-8 / ASCII-8BIT の 912 入力を、実 Ruby 2.7.8 と
Parser::Ruby27 / parser 3.3.12.0 でそれぞれ 1 プロセスで採取する。
両者が str / character として成功した 710 入力だけで、バイト列と符号化を各モデルに照合する。
198 入力の参照側構文エラー・内部例外、4 入力の非 leaf は正常値へ数えない。
primitive decoder への入力範囲は Parser の expression source を使い、物理 tree の欠落と分けて検査する。
再記録は `scripts/record_regexp_literal_values.py --ruby27 /path/to/miniruby --output tests/fixtures/regexp_literal_values.json`。
modifier escape の leaf 値も、本家と実 Ruby で別々に復号する。
parser 3.3.12.0 は `\M-\x43` を 83、`\M-\103` を B1 と残りの `03` にするが、
Ruby 2.7.8 は両方 C3 にする。この差まで同じ値にそろえると架空の capture を宣言する。
ASCII-8BIT の有効入力はこの gate で比較し、無効な UTF-8 regexp の compile 診断 48 条件は
別の regexp validation gate の責務として明示する。元の 3,456 条件から 48 条件を分け、character の回復を 360 条件追加した 3,768 条件を検査する。
このうち 72 条件の安全な束縛の意図差分は通常一致件数へ含めない。
regexp 全体を Onigmo と同じようにコンパイルして妥当性を判定する機能は、この判定の役割ではない。
非 ASCII capture 名と `/e`・`/s` の符号化による宣言差は、上記の目的別 API と専用 fixture で検査する。
記録器は `--raise-cop-error` を使い、内部例外を JSON 内の 0 診断へ読み替えない。
本家自身の VariableForce 例外や、互換しない符号化を連結した際の内部例外も、通常の診断一致には数えない。

出典:

- [parser 3.3.12.0 の static_regexp_node / static_string](https://github.com/whitequark/parser/blob/v3.3.12.0/lib/parser/builders/default.rb)
- [Ruby 2.7.8 の parse.y](https://github.com/ruby/ruby/blob/v2_7_8/parse.y)
## 混在改行と補正前の物理本文

括弧の適用位置と全文scopeの照合は、そのpassの解析用UTF-8 byte位置を使う。
CR復元や元符号化のbyte位置へそのoffsetを移さない。初回のscript検査は復号前原本を使い、
後続passではcurrentの改行由来を最終書出しと同じ私有helperで復元してから再符号化する。
nextの位置表で旧本文を復元せず、pureのdefault CRと追加元bare CR、mixedのcopyしたCRを分ける。
本家の24安全差や174条件を維持し、追加のmixed/pure元bare CRのdisk/stdin対照を実CLI記録で検査する。

`capture_line_endings.json` の18 raw記録では、本家がUTF-8を書いた後に元codecで再読込し、
無害対照も符号化fatalで途中停止する。この途中結果を、完成した括弧補正や再実行の期待値にはしない。
同じ元codecへ戻した次入力を本家へ渡す6chainを `capture_line_endings_safe.json` に別記録する。
Ruby4.0.7 Windows上のRuboCop1.89.0/rubocop-ast1.50.0/parser3.3.12.0で、16回の実補正と
各回の実inspectを記録した。Windowsの実書込byteも保持し、codeの元EOLと最初のDATAを復元する。
各次入力のSyntax受理、6件すべての実括弧増分1、最後の実CLIのexit0/診断0/byte不変を確認する。
全passを単純連結せず、実Offenseの比較属性で最初の診断を保持する。これは本家のCLI記録であり、
実MRI2.7のruntime証明をWindows Rubyへ置き換えるものではない。
