# AGENTS.md

Working notes for AI agents and new contributors. `CLAUDE.md` is a symlink to this file.

## What this project is

Sonicop is a native RuboCop-compatible Ruby linter and formatter. **RuboCop 1.89.0 is the
specification.** Where the two disagree, Sonicop is wrong unless the difference is recorded in
`tests/conformance/known_divergences.yml` with a reason.

That single rule decides most questions. Before changing behaviour, run the real thing and compare:

```bash
rubocop _1.89.0_ --only <Cop/Name> --format json path.rb
sonicop --only <Cop/Name> --format json path.rb
```

Never write an expectation from Sonicop's current output — that bakes today's bugs in as the spec.

## Layout

| Path | What lives there |
|---|---|
| `src/engine.rs` | Inspection pipeline, the correction tree, and the result cache |
| `src/cli.rs` | Argument parsing, run modes, `--auto-gen-config` |
| `src/config/` | `.rubocop.yml` loading, `inherit_from` / `inherit_mode`, Include/Exclude matching |
| `src/rules/` | One file per cop, grouped by department; registered in `src/rules/mod.rs` |
| `src/formatter.rs` | Every output format |
| `src/directives.rs` | `# rubocop:disable` / `enable` handling |
| `config/default.yml` | Vendored from upstream; refresh with `scripts/sync_default_yml.sh <version>` |
| `tests/cops.rs` | Per-cop regression tests using caret annotations |
| `tests/conformance.rs` | Hand-written cases plus the known-divergence manifest |
| `tests/spec_fixtures.rs` | Every case RuboCop's own specs supply, checked against what upstream really reports |
| `tests/cli.rs` | End-to-end tests that drive the binary |
| `lib/`, `exe/`, `script/` | The Ruby gem wrapper that ships the binary |

## Commands

The Makefile is the single entry point; `Rakefile` holds only gem packaging and version syncing. Tool versions (Rust and Ruby) are pinned in `mise.toml`, and every target runs its tools through `mise exec`; `make help` lists every target.

```bash
make setup            # install the pinned toolchain (mise) and fetch dependencies
make build            # cargo build
make test             # Rust + Ruby wrapper tests
make check            # fmt-check, clippy -D warnings, version check
make ci               # check + test + build, install and run the source gem  <- the gate CI runs
make install          # build --release and install to /usr/local/bin
```

`make ci` is what CI runs. Run it before saying anything is done.

## Conventions

**Comment language.** Write comments in Japanese in code and tests changed for this project. When
an existing comment in the edited section is relevant to the change, translate it where useful.
Do not translate unrelated files just to make a language-only diff. The `Makefile` stays in English
so that `make help` matches the README's Development table.

**Comments say why, not what.** The existing comments name the upstream RuboCop method being
mirrored, or record the measurement behind a decision. That is the house style — a comment
restating the code is worse than none.

**One cop per file.** Add a cop under its department directory and register it in
`src/rules/mod.rs`. `every_cop_is_registered_once` and
`every_registered_cop_exists_in_the_default_configuration` will catch a mistake.

## Invariants worth knowing before you edit

**Byte offsets are not character offsets.** tree-sitter reports byte ranges; slicing a `&str` at a
non-boundary panics, and a linter must never abort on a valid file. Offsets a cop derived by
arithmetic are pulled back to a boundary rather than sliced — see `SourceFile::line_column` and
`diagnostic::character_length`. Lengths reported to the user are counted in characters, because
that is the unit RuboCop reports.

**The column `line_column` returns is one of those characters, and `line` is sliced by bytes**, so
`source.line(line)[..column - 1]` is the trap the two units make: it reads as "the text before the
node" and is right for every ASCII line. Two cops carried it — `Style/AccessModifierDeclarations`
and `Style/RequireOrder`, both asking whether anything precedes a node on its own line — and both
aborted the run on a line opening with a multi-byte character. Measure against the line's own start
instead, the way `rules/lint/cop_directives.rs` does:
`[..node.start_byte() - source.line_start(line)]`. Both sides of that subtraction are byte offsets,
so no unit converts.

**Display width is generated, not written.** `src/display_width_table.rs` is produced from the
`unicode-display_width` gem by `scripts/dump_display_width.rb`; do not hand-edit it. A hand-written
table stood there before and drifted — it counted the combining marks U+3099/U+309A as two columns,
so decomposed Japanese drew the wrong number of carets. Regenerate rather than patch:

```bash
ruby scripts/dump_display_width.rb > src/display_width_table.rs
```

`tests/fixtures/regexp_trees.jsonl` follows the same pattern; its provenance is in the neighbouring
`.PROVENANCE` file.

**結果キャッシュは本文も照合する。** ビルドの fingerprint、設定の digest、cop 選択、
ファイルのサイズ・更新時刻・権限を検査する。権限は `Lint/ScriptPermission` が使う。
同じサイズの書き換え後に mtime を戻すこともできるため、stat の一致だけでは再利用しない。
保存済みの復号本文 digest も毎回確認し、I/O の前に索引の read lock を解放する。
stat は本文を読む前に取得し、digest は NUL 処理前の本文から作る。
保存形式を変えた場合は `RESULT_CACHE_SCHEMA` を上げる。

**Remote inherited configurations are bounded.** Network operations time out after 30 seconds and
each response may contain at most 5 MiB. `ureq` rejects a body whose length equals the value passed
to `BodyWithConfig::limit`, so pass one byte beyond the logical maximum. Preserve tests for both the
exact limit and the first rejected byte when changing this path.

**Ask the index about the tree's shape, never the parser.** `Node::parent` walks down from the
root comparing byte ranges, and `Node::named_children` / `Node::children` open a tree cursor on
every call. A sampling profile of a run over RuboCop's own tree put 84% of the working CPU inside
those two, and it was structure being re-derived rather than anything a cop decided. `AstIndex`
records the answers on the walk it already makes:

| Instead of | Use | Shape |
|---|---|---|
| `node.parent()` | `node.parent_of(context)`, `index.parent_in_tree(node)` | one array lookup |
| `named_children(node)` | `named_children_of(node, context)` | a copy of a recorded slice |
| `nodes::children(node)` | `nodes::children_in(node, context)` | the same, filtered |
| a stack walk over descendants | `walk_named(node, context, &mut …)` | a slice of the pre-order list |
| `node.children(&mut cursor)` | `context.children(node)` | an iterator over the pre-order list |

A node the index does not know -- one of the extra trees `Metrics` parses to recover the fragments
the grammar swallowed -- falls back to the parser, so every one of these answers the same list
either way. `the_index_answers_what_the_parser_answers` holds the two to that.

The cursor forms are still there, because a helper without a context in reach cannot use the index.
**Reach for the index form when the context is at hand**; the difference is not small.

**The grammar lives in a `static`, and that is what makes a kind name `&'static str`.** Since
tree-sitter 0.27 the name tables borrow from the `Language` they were read out of -- `Node::kind`,
`TreeCursor::field_name`, `Language::node_kind_for_id` and `Language::field_name_for_id` all
returned `&'static str` before it and return a tied borrow now. `NodeExt::kind_str` is compared
against a literal at some 3,600 call sites and several helpers hand the field name on as
`&'static str`, so the answer has to outlive the tree it came from. `node_ext::LANGUAGE` is the one
`Language` the process holds; `KIND_NAMES` and `FIELD_NAMES` are built from it, and both keep
answering `&'static str` because a `static` is never dropped. **Do not build a `Language` per call
to read a name out of it** -- the borrow will not outlive the call.

Two consequences worth knowing. `ERROR` and `_ERROR` carry ids above `node_kind_count()` (65535 and
65534 against 368 kinds), so they are named by `kind_str`'s fallback rather than by the table, and
`Lint/Syntax` rests on that name: `an_error_node_is_named_the_way_the_parser_names_it` holds it to
what the parser says. And a field is read out of `FIELD_NAMES` by id rather than off the cursor --
`crate::rules::field_name_for_id(cursor.field_id())`, which
`the_field_table_answers_what_the_cursor_answers` holds to `TreeCursor::field_name`. Field ids mean
the same thing in every tree built from the same grammar, so this holds for the extra trees
`Metrics` parses too.

**A walk over a subtree is iterative, in a cop as much as in the index.** `AstIndex::collect` says
why: a rayon worker's stack is far smaller than the main thread's, so a recursion deep enough to
exhaust it aborts the whole process rather than failing one file. The tree nests once per operand
of a chain and once per bracket of a nested literal, so the depth is the source's to choose —
`Lint/UnreachableLoop` and `Lint/DuplicateHashKey` each went down on a generated file until their
walks were rewritten around an explicit stack. `push_named_children` and `push_named_children_in`
push a node's children so that popping yields them in source order; reach for one of those rather
than calling the walk from inside itself.

**A pattern built from the configuration cannot live in a `LazyLock`, so it needs the cache.**
`crate::rules::regex_cache::compiled` keeps a compiled pattern for the life of the process. Without
it, a cop rebuilds the same automaton for every file: `Layout/LineLength`'s `URISchemes` regex was
the single largest cop cost of a run until it went through the cache. `grep -n "Regex::new" src/`
and check that every hit outside a `LazyLock` is either cached or unreachable in a default run.

**The result cache index is one JSON file holding every offense, so it is written buffered.**
It carries each offense's message and the source line it was found on, which comes to 336 MB over
`ruby/ruby`. `serde_json::to_writer` hands the writer a token at a time, so pointing it at a bare
`File` turns one index into millions of `write` calls -- 115 seconds of a 135-second run, all of it
blocked in the kernel while `user` time stayed flat. A default run has the cache on, so this is the
path most users are on and the one a `--cache false` comparison never measures. Keep the
`BufWriter` in `persist_cache_index`, and record `real` beside `user` when timing this path.

**補正は原本の書き込み可否と属性を守る。** 読み取り専用の原本を親への rename 権限だけで
変更してはいけない。まず truncate せず書き込み用に開き、OS の判定に従う。通常は一時
ファイルを使うが、symlink・hardlink・Unix の所有者や拡張属性・macOS ACL を失う場合は
同じファイルへ書く。親へ一時ファイルを作れない場合も、原本に書ければ本家と同じ経路を使う。
この経路の書き込みは原子的ではない。Windows のリンク数はファイル情報 API で取得し、
失敗時は書き込まずエラーにする。診断だけでなくディスク上の本文・リンク・属性を検査する。

**補正の結合順は Registry の実順序を使う。** `config/autocorrect_support.json` は
単純な名前順と異なる本家の登録順を記録したものでもある。
結合順と無限ループの cop 表示順は同じ記録へ向ける。`unless !a` と入れ子 if の組み合わせは、
名前順へ戻すと `-a` と `-A` の両方で出力が変わる。6 件の本家 CLI 記録をディスクで照合する。

**検査結果は対象ファイルの順を保つ。** 本家はディレクトリ内を文字列順に展開し、
明示引数は指定された順に検査する。検査後に Path の構成要素で並べ直すと、
`foo/bar.rb` が `foo.rb` より先になり、引数の順も失われる。
rayon の indexed collect と逐次の collect が保った入力順をそのまま使う。
並列・単一、キャッシュの有無と再実行、ディレクトリ・正順・逆順の引数を一緒に検査する。

**入れ子設定の探索位置とプロセスの cwd は別物。** サブディレクトリの設定を読み込んでも
診断中のパスは起動位置を基準にする。絶対パスの stdin・相対パスの stdin・ディスク探索を
一緒に確認し、設定ファイルのディレクトリで cwd を上書きしない。
Windows の短縮名と長い名前を、表示のために無条件で同一視しない。本家は cwd との
文字列の一致で短縮するため、同じファイルでも入力の表記によって絶対表示を保つ。
`conformance.yml` の `windows_reference_only` は3形式の stdin と通常探索を実測する。
Ruby の `File.realpath` は Windows の短縮名を保持し、Rust の canonicalize と同じ文字列に
なるとは限らない。比較用入力の長い名前は Windows API で取得し、実際の cwd と両者の
JSON を保存する。パスの同一性だけを根拠に表示の互換性を変更しない。

**依存更新も構文診断を変える。** tree-sitter 0.27.1 は旧 grammar で有効な percent literal に
誤った `tSTAR` を追加した。grammar v26.10.103 との組でその回帰が解消することを実測し、
両者を固定している。回帰テストは新たな誤診断を拒むが、既知の EOF 誤診断を期待値へ固定しない。

**grammar はローカル変数の束縛を知らない。** v26.10.102 から `name +x` を `name` の引数として
読む (未知の識別子に対する Ruby の読み方)。`-x`・`*x`・`**x`・`&x`・`[x]`・`<<X`・`/x/`・`%w[]`
も同じで、`name` がその位置でローカル変数なら、Ruby は二項演算や添字として読む。そのまま渡すと
変数を使っていないと誤って報告し、`-A` が引数名を `_name` へ変えたり代入を消したりしてから
`name(+x)` と括弧を足す。cop ごとには橋渡ししない。`src/lexical_locals.rs` が束縛を判定し、
`parser::parse` が演算子の直前の空白を改ページ (grammar の手がかり) にして読み直す。本文にもとから
ある改ページは、トークンの外にあるものだけを空白へ変え、文字列・コメント・heredoc の終端語の中の
ものは残す。手がかりで `<<` をシフトと読むと heredoc だった本文がコードやコメントになるように、
読み直すと束縛も改ページの位置づけも変わるので、手がかりと空白にする改ページは読むたびにいまの木から
計算し直し、変わらなくなるまで繰り返す (前に通った状態へ戻ったら止める)。手がかりが次の手がかりを
露出させる入れ子 (`x %(x %(x +1))`) は段ごとに 1 回読み直す。深さ 400 のような入力では
本家より遅くなるため、深さと検査する cop をそろえて計測する。通常のソースの深さは数段。
演算子が行継続の直後なら行継続の前の空白に置き、grammar
は行継続を越えて手がかりを保つ。名前と行継続が接した `x\` + 改行 + `+1` のように空白が無い場合は、
名前の直後へ仮想の改ページを挿入し、parse の直後に末尾から `Tree::edit` で消す。返す木の全ノードの
バイト範囲と TSPoint は原文へ戻し、束縛と元の改ページの再判定も原文と同じ長さの本文で行う。
束縛は評価の順ではなく字句の順で決まり、修飾子の本文は条件より先に、heredoc の本文は `<<~X` の
直後に読まれる。`def`・`class` は外側の変数を見ず、ブロックとラムダは見る。名前付きキャプチャの名前は
`rules::lint::named_captures` が Ruby のリテラルを復号し、`regexp_names` が Onigmo の規則で取り出す。
変数解析も同じものを使う。`it` (Ruby 3.4 以降) は参照した後も字句解析ではローカル変数にならない (`it +1` は呼び出し)。
宣言の種類を足すときは `lexical_locals` の単体テストと、全 cop を既定設定で動かす CLI テストを
本家の実出力から増やす。手がかりで読み直した木に構文エラーがあっても、手がかりのない木へは戻らない
(`x = 1; x +1, 2` は Ruby でも構文エラー)。`?` の三項演算子と `::` は grammar の内部の字句解析が
決めるので、この仕組みの対象外。

**heredoc の補間は引数の位置で計測する。** 終端の後からチェーンを続けると、本文が外側の
call の直下へ移る。receiver と arguments だけの走査では補間を落とす。開始と本文の対応表で
引数の位置から処理し、本文の本来の位置では二重に数えない。ローカル変数の宣言と safe navigation
の割引も同じ順序を使う。同じ開始行の後続引数と、終端後のチェーンの引数を両方検査する。
本文の補間で開いた heredoc は grammar v26.10.103 で読めるようになった (owayo/tree-sitter-ruby#8)。
本家の whitequark/parser は内側の本文を外側の本文にも重ねて含め、Metrics の値が食い違う (#20)。
3 段では末端の補間が 4 回になり、内側の単一引用の本文も外側で補間として読まれる。
この重複を製品の評価順へ持ち込まない。whitequark の実診断は共有 fixture に保存し、物理ソースと
本家 Prism に従う移植版との差を `known_divergences.yml` で毎回監視する。
重複した代入を根拠に `Lint/UselessAssignment` へ寄せると、実際には後続の heredoc が読む変数の
宣言を本家の `-a` / `-A` が消し、構文は有効なまま `NameError` になる。診断も補正も意図的な差とする。
`ParserEngine` の明示値が優先し、既定は Ruby 2.7 で whitequark、3.3 以上で Prism。
CLI 回帰は全フィールド・LF/CRLF の原本バイトと、2.7 + Prism の設定拒否を検査する。

**ローカル変数の後の `?2` は三項演算子。** grammar の character は内部 lexer の最長一致なので、
二項演算の改ページでは切り替わらない。名前のその位置の束縛と、直後の空白・行継続を確認し、
`?` の直後へ仮想の空白を挿入する。`?foo` が引数の外の ERROR に回復される場合もあるため、
call の arguments の開始だけで判定しない。仮想の空白も parse 直後の Tree::edit で消し、原文の
byte 範囲と TSPoint を保つ。未宣言の名前や明示した receiver への `?2` などの単一文字引数と、
名前に `?` が接するメソッド呼び出しを保つ。水平空白には VT と単独 CR も含め、CRLF の通常改行は
越えない。`?foo` のように識別文字が 2 文字続く場合は未束縛でも三項になり、
明示 receiver の後でも同じ。`?a`・`?2`・単一 Unicode 文字・escape の文字引数と分ける。
設定のない parse は現 Ruby の物理的な規則を保つ。configured 入口は既存の Semantics 経由で
実 engine を選び、whitequark の有効な `?あand false` を Unicode の三項へ変えない。
`yield`・`super` も先頭 keyword から同じ複数識別文字の規則を調べる。読み直しで束縛が消えた
手がかりは撤回する。colon 欠落は原本を読む backend で診断し、有効なコードへすり替えない。

**VT は Ruby の水平空白。** 元の VT と改ページは、現在の木のトークン外にある位置だけ同じ長さの
空白へ置換する。文字列・正規表現・コメント・heredoc の終端名の中では原文の文字を保つ。
character の escape や percent literal の制御文字区切りは grammar が葉外へ落とすこともあるため、
原本の Prism token で開始と終了を対応付けて補う。現時点の木が同じ開始を `%` や `?` の演算子として
読み、その直前名がその字句位置で束縛済みの場合だけ、終了側も一緒に補助保護を撤回する。
回復した literal 内の代入・仮引数・scope は、この追加判定の根拠にしない。範囲は宣言の来歴だけに
使い、本文や補間コード全体を空白化から除外しない。`%w[a\v b]` の WORDS_SEP は保護しない。
SpaceAroundOperators も同じ根拠で保護中の開始だけを除外する。最初の誤 `%` 補正で literal 開始を
壊すと次の pass が本文を code と読み、新しい診断と補正を足すため。本文や補間の演算子を範囲ごと
沈黙させず、VT/FF のない通常ファイルでは追加の字句解析を行わない。
control 区切り等を読む grammar 自体の既存制約は残る。元の文字を保護したことを、その不完全な木が
Ruby を正しく表す証拠にしない。診断・補正バイトを元版と比較し、新しい受理や補正の成功へ数えない。

**source gem の出力先は Cargo に問い合わせる。** `CARGO_TARGET_DIR` や Cargo の
`build.target-dir` があると、バイナリは gem 内の `target/release` にあるとは限らない。
`cargo metadata` の `target_directory` を使い、取得失敗時は Makefile の生成前に止める。
通常・絶対・相対・設定ファイルの各指定を、小さな実ビルドから導入・起動まで検査する。

**補正後は部分式ではなくスクリプトとして検査する。** 診断用 Prism の partial_script は、
トップレベルの break 等も受理する。診断設定は互換性のため維持し、書き戻す最終出力を
main_script で別に検査する。同じ設定で原本が有効な場合だけ新しい破壊と判断する。
対象版を使い、Prism の下限より古い対象には 3.3 を使う。この下限を元の対象版を検証した
証拠にしてはいけない。CRLF はディスクへ出す形式に戻してから比較する。

検査には復号前の原本と、実際に書き出す予定のバイト列を渡す。復号済み UTF-8 を
固定すると、US-ASCII 等で Unicode とバイトのエスケープが混在する構文破壊を見逃す。
宣言・BOM・別名は Prism 自身に解釈させ、診断用の encoding lock と分ける。
stdin も同じバイト列の検査を使い、拒否したときは元の入力を返す。

**単一引用のバイトエスケープは文字列のまま。** Lint/LiteralInInterpolation で
`'\x80'` や `%q(\201)` を二重引用と同じように復号すると、ASCII の値を
不正な UTF-8 と誤認する。本家が展開する値と、ディスク上の補正後の値を両方確認する。

本家が構文を壊す補正は Claude の判定と最小の実出力を差分マニフェストへ登録する。
ファイル単位で拒否し、元のバイト列と未補正の診断、stderr の理由、終了 2 を返す。
非 Ruby の shebang から次の Ruby shebang までにある独立した exec 行の消去も防ぐ。
任意のシェル式や、exec 行の内容が変わった場合の意味保存を保証するものではない。
コメント・通常の空白・本文だけの無害な補正は拒否せず、LF/CRLF の陰性対照を保つ。

**heredoc の終端名はセミコロンの折り位置ではない。** Layout/LineLength の字句除外には
heredoc_beginning も必要で、文字列の名前に含まれる ; を分割すると Ruby を壊す。
本家が有効な補正を返す場合、安全網による拒否で代用せず cop の補正を直す。

**BOM の行分類とマジックコメントの解釈を分ける。** FrozenStringLiteralComment の
本家実装は、非コメントトークンの位置で先頭コメントを区切り、判定には元の行を渡す。
BOM 付き符号化宣言で区切ってしまうと、後続の frozen コメントを毎パス挿入し続ける。
一方、判定時にも BOM を除くと、本家が認識しない先頭の frozen コメントまで認識してしまう。
always_true はコメント行全体を置換するが、never はコメントの前の BOM・字下げを保つ。
36 件の本家実出力を、診断全項目・ディスク上の補正バイト・再実行で検査する。

**200 パス到達は収束ではない。** 本家も上限到達時は途中の補正を書き戻し、次回実行で
さらに補正することがある。205 段の入れ子で、本家と同じ終了 1・Infinite loop 診断・
途中の 86,261 バイト、次回の 86,311 バイトを確認した。無断で上限を増やしたり、
上限到達を安全性ゲートの成功へ読み替えたりしない。Ruby コーパス全体の冪等性は未達で、
本家側の非冪等性だけを根拠に、候補の出力差を互換一致として扱ってはいけない。

**`Style/CaseLikeIf` has an unsafe upstream correction.** For an `if`/`elsif` chain whose `else`
contains another `if`, `unless`, or a modifier form of either, RuboCop reports an offense but
rewrites the inner keyword to `when` after the `else`. Ruby rejects the result. The grammar
currently accepts it without an error node, so the general syntax guard cannot detect it. Keep
the diagnostic compatible and withhold that correction; the intentional differences are recorded
in `tests/conformance/known_divergences.yml` and covered by a CLI test that checks the file on disk.

**NumericPredicate の send とブロックを分ける。** ブロック付き call の診断と置換は
NumericPredicate 内の `send_range` で引数かメソッド名の終端までとし、行継続・縦タブと
ブロックの本文を消さない。比較へ戻す設定では述語 send の親が
ブロックなので、その外の否定を直接の親に数えない。本家は `foo.zero? { }` を `foo == 0 { }` に
変えて構文を壊す。診断は一致させ、既存の main_script 検査でファイル全体の書き戻しを拒否する。
#23 の実出力は `numeric_predicate_block_divergences` へ登録する。#17 の splat 等の差とは分ける。

**A guard clause can change a local variable into a method call.** If the condition first assigns
`error`, converting `if error = value; raise error; end` to `raise error if error = value` moves the
read before Ruby has parsed the declaration. RuboCop's correction raises `NameError` at runtime;
the later `Lint/UselessAssignment` correction only removes an assignment that is already unused.
Keep the offense but withhold this correction when the guard reads a variable first declared in
its condition. A variable declared earlier in the scope still permits the upstream correction.
Register the intentional difference in the divergence manifest for each correction mode where it
actually occurs; full `-A` runs can take another correction path and converge to identical bytes.

**Preserve CRLF bytes when correcting a CRLF file.** RuboCop's source buffer normalizes CRLF to LF;
its writeback then changes even the bytes after `__END__`, which changes `DATA.read`. Sonicop keeps
the original line ending format on disk. After a correction it re-inspects LF text, as RuboCop does,
so later passes still find the same offenses. New lines inserted by corrections must become CRLF
before writeback. The intentional byte difference is recorded in the divergence manifest.
The tree-sitter comment node includes the `\r` in CRLF, while RuboCop's comment token excludes it.
Trim that byte from indexed comment ranges before cops calculate offense locations or source text.

`Layout/EndOfLine` の検査終点は物理的な最終行ではなく、本家の `tokens.last.line` に合わせる。
末尾コメントや heredoc 本文を読み終えてから返る `tNL` は、直前の文の行番号へ戻る。
終端改行の無い通常コメントは whitequark では `tNL` より後、Prism では前となる。
終端なしの `=begin` コメントは両 engine で後となるため、EOF と NUL 投影を区別する。
トークンが無い場合だけ生の行数を使うため、`__END__` だけの入力も除外しない。
NUL 限定の DATA 探索と通常ファイルの終点判定を分け、LF/CRLF/mixed・コメント・継続・
heredoc・空文・トークン無しを、診断全項目と非補正の実バイトで本家と照合する。

**ブロックコメントの範囲は行末まで含む。** `=begin` のコメントは `=end` の後の改行までが
1 トークンで、whitequark で末尾改行のない EOF に閉じると報告範囲はさらに 2 文字先へ伸びる。
読み取り・編集用の範囲は本文内に保ち、報告用の範囲と長さだけを分ける。Prism は EOF で止まる。
`Style/BlockComments` の囲いは本家の正規化済み文字位置で測り、元の UTF-8 バイト境界へ戻す。
バイト数で固定長を引くと `=end あいう` で panic し、CRLF では囲いの一部が残る。
コメント全体を置換する cop は、whitequark の本文外の範囲で本家が診断の記録前に失敗する場合も
照合する。LF / CRLF、末尾改行、パーサ、診断と実ファイルの補正バイトを合わせて検査する。

**YAML scalar types must follow Ruby's YAML 1.1 loader.** Plain `yes`/`no`/`on`/`off`
and case variants of `true`/`false` are booleans; quoted strings and explicit string tags keep
string values. `src/config/yaml_booleans.rs` scans tokens before serde_yaml so scalar styles,
anchors and tag directives are preserved. The pinned unsafe-libyaml scanner's `mark.index` is a
UTF-8 byte offset. Check the actual scanner's `READ`/`SKIP` width arithmetic before changing that
assumption. A raw-YAML test must write the configuration directly: a YAML 1.2 serializer can drop
the quotes around `no` and silently change the test's meaning. RuboCop 1.89.0 rejects a config
with a UTF-8 BOM, so accepting one here would also be a compatibility change.

**Configured String and Regexp patterns are different Ruby inputs.** Upstream interpolation
keeps String alternation ungrouped while a Regexp's string representation groups it. For example,
`Style/NumericLiterals`'s `\A#{regexp}\z` anchors `"123|123456"` differently from `/123|123456/`.
Preserve tags and match upstream's real result before simplifying either pattern.

**A visibility declaration can report an offense without any correction.** With
`AllowModifiersOnSymbols: false`, `protected *METHOD_NAMES` has no corresponding definition
node, so upstream's `def_nodes.any?` guard leaves it in place and reports `correctable: false`.
Deleting that declaration and inserting a bare modifier later makes existing methods public.
Test the file on disk and Ruby's actual method visibility, not just its syntax.

**Line-length ranges use normalized character positions without clamping to a line.** Leading
tabs can put their endpoints on the following line or past EOF. `reported_offset` maps those
positions to original UTF-8 bytes. At a CRLF line's end it returns the boundary before `\r`,
not between `\r` and `\n`; otherwise only `last_column` changes while counts, start positions
and length still match. Keep LF/CRLF, endless-method and tab-crossing controls together.

**登録数と発火数を混同しない。** 全項目の CLI 掃引は `docs/cop-conformance.md` に記録する。
プロジェクト索引を必要とする 3 cop も索引を有効にして測る。索引を無効にした沈黙を
完全一致に数えない。README の表は `scripts/conformance_table.rb` で生成する。
このスクリプトは正の完了件数、対象パス集合、診断全項目の多重集合を比較し、
別々の入力で生じた過剰と不足を打ち消さない。

**旧版の構文は parser gem の状態機械で判定する。** `src/legacy_parser` は固定した parser
3.3.12.0 と racc 1.8.1 の字句器・還元動作・C cparse の回復を使う。対象 2.7〜3.2 と
明示した parser_whitequark の 3.3/3.4 は、tree-sitter の ERROR の有無に関係なく全ファイルを通す。
error の合成 shift ではトークンを消費せず errstatus を減らさない。回復で失われた def の
文脈も正常な終了動作が走るまで戻さない。構文ごとの手書きゲートや後続診断を追加しない。
`tables` は `ruby scripts/legacy_parser/generate.rb` で再生成し、固定版・入力ハッシュ・ライセンスは
`src/legacy_parser/PROVENANCE.md` を参照する。Ruby 版 Racc の結果を C 版の代用にしない。
LF/CRLF、文字単位の EOF 範囲、同範囲で先の診断を残す順序、-a/-A が原本を変更しないことを
実 CLI の記録で検査する。中間ノードも rayon worker の小さいスタックで反復して解放する。

**正規表現の AST 分類と安全な束縛を分ける。** whitequark は 3.3 未満の対象で、文字列だけの
補間や空 begin まで連結し、`=~` を `match_with_lvasgn` にする。3.3 以上は明示 whitequark でも
補間があれば send。`regexp_ast::Semantics` の分類を cop 間で共有する。名前の宣言は同じ条件だけで
決めず、本家と Ruby 2.7 の畳み込みの両方に残る capture だけを使う。空 begin・先行空文・捨てる str の
架空の宣言に追従すると、空白補正だけで y(+1) を y() + 1 へ変えて実行を壊す。
一方、普通の単一 str 補間は実 Ruby 2.7 でも宣言するため、Ruby 4 の実行を根拠に拒否しない。
版・engine 別の実 CLI、旧 Ruby 本体での実行と陰性対照は `docs/regexp-semantics.md` に記録する。

## Adding a cop test

`tests/cops.rs` uses upstream's caret notation: the annotation points at the preceding source line,
leading spaces give the column and the run of `^` gives the length.

```rust
expect_offense("Style/RedundantReturn", r#"
    def foo
      return 1
      ^^^^^^ Redundant `return` detected.
    end
"#);
expect_no_offenses("Style/RedundantReturn", "def foo\n  1\nend\n");
expect_correction("Style/RedundantReturn", before, after);
```

Cases that match upstream belong in `tests/cops.rs`. Cases that do not belong in
`tests/conformance.rs` together with an entry in the divergence manifest explaining why.

## The cases nobody wrote by hand

Hand-written tests only cover what somebody thought to write down. Counted on 2026-08-23, the
1,918 of them reach 606 cops but check an actual **offense** for only 427 — and `make cop-coverage`
recounts it, because reading the test source does not answer the question (cop names get passed
through `const`s).

`tests/spec_fixtures.rs` closes that gap from the other side: every case RuboCop's own specs
supply becomes a case here. **The expectation is not the spec text — it is what upstream really
reports**, recorded once by `make spec-fixtures`, because upstream does not always behave the way
its specs say. The recording is committed, so a normal test run needs no rubocop gem.

**The input is collected by running the specs, not by reading them.** `make spec-capture` loads
`spec_capture.rb` into upstream's own suite and records what reaches `expect_offense` — by then
the source is a plain string and the cop is configured exactly as the example meant it. Reading
the files instead leaves four whole categories out, and with them 54 of the 609 cops: a
`shared_examples` driven by `it_behaves_like`, a `#{}` the spec evaluates, an
`expect_offense(wrap(<<~RUBY))` where the heredoc is an argument, and a `cop_config` a surrounding
`context` builds. Two cops (`Lint/Syntax`, `Bundler/GemFilename`) never call `expect_offense` at
all, so the hook also wraps `CopHelper#_investigate` and `Commissioner#investigate`.

Running the suite needs upstream's development bundle, which the pinned tree at
`~/tmp/rubocop-v1.89.0` does not ship with. `bundle install` fails there under Ruby 4 — install
`rspec`, `webmock` and `mcp` as plain gems and drive RSpec through
`ruby -e 'require "rspec/core"; exit RSpec::Core::Runner.run(ARGV)'` instead. The Makefile does
not know where the upstream tree, the capture hook and the generator live; pass them as
`UPSTREAM_SPEC_TREE=`, `SPEC_CAPTURE_HOOK=` and `SPEC_FIXTURE_GEN=`.

As of 2026-08-23 all **11,300** recorded cases match, across the **555** cops the specs reach, with
no entry in `spec_known_divergences.yml`. A difference appearing there is a regression, not a
backlog item.

Two things about this gate are easy to get backwards.

**Its value is in the cases where upstream stays silent.** Corpora can only show what upstream
reports, so **over-detection is invisible to them** — the shapes upstream is quiet about are
infinite and appear in real code only by accident. Roughly 44% of the recorded cases are
`no_offenses` ones, and that half is the point of the file.

**Reproducing the recording means reproducing its conditions.** Seven ways of getting that wrong
have already turned the harness into a difference generator, and each looked exactly like a bug in
a cop:

| Mistake | What it produced |
|---|---|
| Running `.rb` where upstream ran `.gemspec` / `.gemfile` | Every Gemspec and Bundler cop silently matched nothing |
| Passing the printed `length` as the caret count | Every offense spanning lines became a `range` difference — 57 in three Metrics cops alone |
| Feeding a recorded output through `CopCase::corrected` | It dedents, for the hand-written `<<~RUBY` cases; recorded output loses its leading newline and indentation. **20 differences, all of them mine** — use `corrected_verbatim` |
| Treating `foo&.bar` as a `send` | The grammar spells `send` and `csend` alike; upstream's `:send` arm excludes `csend`, and reading it as one makes a cop claim its own receiver is non-nil |
| Reading and writing the case with Python's default newline handling | Universal-newline translation turns a CRLF case into an LF one on the way in **and** on the way out, so `Layout/EndOfLine` — the cop whose whole subject is the line ending — records an expectation nobody can reproduce. Both sides need `newline=""` |
| Serialising the recorded `cop_config` as JSON | JSON has no symbol, so `{ any?: :none? }` lands as `{"any?": "none?"}` and YAML reads it back as string keys. `Style/InverseMethods` looks its methods up by symbol, so the recorded configuration disabled the very thing the case was testing. Record with `inspect`, and write the YAML **as a block mapping** — Psych reads an unquoted `:any?` as a symbol, a flow mapping `{:any?: :none?}` is a syntax error, and quoting it makes it a string again |
| Omitting the `Enabled: true` the generator writes for `--only` | A cop that ships disabled behaves differently with that one line: `Style/DisableCopsWithinSourceCodeDirective` refuses to be switched off by a directive **only** when `Enabled` is explicitly true, so without it the case's own `# rubocop:disable Style` silenced the cop under test |

The first two accounted for 54 of the first 65 failures; the last three for 39 of the 133 that
remained after the cop work. Before believing a difference, reproduce
it from the command line against the real upstream — the recording says what upstream did, not
what the harness asked it.

**Print the tree before guessing at it.** Most of what is left in that gate is a place where the
grammar and upstream's AST disagree about shape, and reasoning about which node holds what is
slower and less reliable than looking:

```bash
cargo run --release --example dump_ast -- 'do_something(**{foo: bar, **{baz: qux}})'
cargo run --release --example dump_ast -- --file path/to/source.rb
```

Two differences that had each survived a round of guessing fell in minutes once the tree was on
screen: a nested `**{…}` is a `hash_splat_argument` **among the pairs** of the hash above it (so
`node.pairs` is not `children`), and `not bar ? a : b` parses as `unary(not, conditional(…))`
rather than as a conditional over a negation.

**One grammar node often stands for several upstream ones.** These have each cost more than one
cop, and the second time is always in a cop whose author had read the first fix:

| Written as | Grammar | Upstream |
|---|---|---|
| `a && b`, `a \|\| b` | `binary` | `and` / `or` — **not** a `send`, so `operator_method?` says no |
| `a << b`, `a + b` | `binary` | `send` — an operator call, which `(send (lvar :a) :<< …)` matches |
| `/re/ =~ str` | `binary` | `match_with_lvasgn` — answers nothing to `arguments`. **Only for a regexp the parser builds while reading**: an interpolation keeps it a `send`, though before 3.3 an interpolation of string literals alone still builds |
| `foo&.bar` | `call` | `csend` — upstream's `:send` arm excludes it, `on_csend` is separate |
| `foo.bar { … }` | `call` holding the `block` | `block` around the `send` — the send's range stops before the block, and the block is its parent |
| `a[b] = c` | `assignment` | `send` of `:[]=`, a binary operation like `a[b]`; `a.b = c` sends `:b=`, which is no operator |
| `not(x)`, `not (x)` | the same `unary` over `parenthesized_statements` | `(send x :!)` and `(send (begin x) :!)`: parentheses written against the keyword are its own, spaced ones are a `begin` |
| `a.+(b)`, `a.+ (b)` | a `call` whose `argument_list` opens with `(` | only the first is `parenthesized?`; the spaced `(b)` is a `begin` argument |
| `%w[…]`, `%i[…]` | `string_array` / `symbol_array` | `array`, with `percent_literal?` true |
| `%w[…]`'s elements | `bare_string` / `bare_symbol` | `str` / `sym` |
| `/foo/`'s text | `string_content` | `str` |
| a heredoc's body | one `heredoc_content` per run | one `str` **per line** |
| `{ a: 1 }`'s key | `hash_key_symbol` | `sym` |
| `(foo)` | `parenthesized_statements` | `begin` — and node accessors see through it |
| a `def`'s body | `body_statement` between the two | the body hangs off the `def` directly |
| the top level | `program` is a parent | `node.parent` is **nil** |
| `_1`, `it` | `block` | `numblock` / `itblock` |
| `foo 1 do … end` | one `call` whose `block` field holds the block | `block` around the `send` -- `each_child_node(:send)` does not see it |

**Print the corrections too, when a correction does not land.** A cop can build a perfectly good
set of edits and still leave the file untouched: the edit applier drops what falls outside the
offense's anchor, and the syntax guard throws away a pass whose output does not parse. Neither
says which edit was at fault.

```bash
cargo run --release --example dump_corrections -- --config .rubocop.yml --only Style/Foo file.rb
```

`Style/Next` was reported as "correctable" and never corrected, because its edits reach the `then`
and the `end` beyond the range it reports and nothing anchored them there. `Style/MutableConstant`
under `Recursive: true` was withheld as unparsable, because a hash key is a `hash_key_symbol` here
and was missing from the immutable list, so the second pass tried to append `.freeze` to it.

複数の cop の補正が合流した結果を調べるときは `SONICOP_TRACE_CORRECTORS=1` を付ける。cop ごとの
corrector を結合に渡す木の形 (範囲・`before`・置換・`after`、子は字下げ) で出し、取り込んだか・
非互換の宣言で見送ったか・衝突で丸ごと捨てたかを添える。編集の一覧だけでは、空白を `(` に
置換する補正と、空白を消してその前に `(` を挿す補正が同じに見える。

**The same source can have two right answers.** `TargetRubyVersion` changes the tree upstream
builds, not just which constructs it accepts: `a, b = 1, 2 rescue nil` puts the `rescue` around the
whole assignment before 2.7 and around the right-hand side after it, and the correction differs
accordingly. When a recording disagrees with the upstream you run by hand, check the recorded
`target_ruby` before concluding either is wrong.

**A construct the grammar cannot read is not a syntax error.** `Lint/Syntax` reports what `parser`
reports; an `ERROR` node over Ruby the real thing accepts is a false positive that also stops every
other cop from running on the file. Two are known and skipped by name in `src/rules/lint/syntax.rs`
-- a multi-line array pattern (`in bar,\n baz`) and a heredoc opened inside an interpolation. Both
also mislead the cops that then do run: silencing the first one turned
`Style/MultilineInPatternThen` into a false positive, because the pattern the grammar closed early
looked single-line.

**The grammar can also accept Ruby that `parser` rejects.** With a previously declared local,
`value [0] += 1` is an index assignment; without that declaration, Ruby reads `value [0]` as a
method call with an array argument and rejects the assignment operator. The tree-sitter grammar
builds `operator_assignment` in both cases. `Lint/Syntax` uses the existing variable analysis to
distinguish them. Check both sides against RuboCop when updating the grammar: fixing the valid
case alone can turn a false syntax offense into a missing one on invalid source, allowing `-A` to
rewrite a file RuboCop refuses to inspect. Numbered parameters (`_1` through `_9`) also need a
version check: RuboCop accepts their spaced index assignment starting with target Ruby 3.2,
including at top level; the variable analysis does not register implicit numbered parameters as
ordinary locals. When the right-hand side is missing at the operator, RuboCop stops recovery at
that operator; tree-sitter's later errors from the same expression must not become extra offenses.

Constants and argumentless qualified calls have the same command-argument ambiguity:
`Foo [0] = 1` and `self.value [0] = 1` are invalid, even if the constant is already defined.
Calls closed by arguments or a block (`value() [0] = 1`, `value { 1 } [0] = 1`) remain valid.
A root constant behaves differently: `::Foo [0]` is rejected at `[` even without an assignment.
Compare both the rejection and the accepted controls with the pinned gem, and inspect the file
on disk after both correction modes.

**先頭空行の診断には実トークンを使う。** コメントや空文はASTがなくてもtokenを持つが、
行継続・BOM・コードNUL・DATAだけの入力は持たない。空programの範囲をtokenとして報告しない。
通常の単一引用・二重引用は文字列全体が先頭tokenになる。raw NULがあるとwhitequarkは
引用符を分け、Prismは文字列全体を保つ。二重引用の行継続は物理行をまたいでも単一tokenで、
単一引用の行継続や通常の複数行文字列とは診断範囲が異なる。

**先頭コメントの位置は本家のトークン列で決まる。** `Style/FrozenStringLiteralComment`
はコメントだけでなく、先頭の通常文字列の復号済みバイトも shebang・符号化宣言として扱う。
`\xFF` のような非 UTF-8 バイトを Unicode 文字へ置き換えると判定が変わる。
whitequark と Prism は空行・行継続・EOF のコメントに対する `tNL` の順序も異なるため、
物理行や改行の個数だけで挿入位置を決めない。9,426 件の本家トークン記録と、
57 件の 200 パス停止時の実ファイル出力、93 件の収束する入力を別々に検査する。

**本家の corrector の呼び出しを畳まない。** 同じ範囲への `remove` と `insert_before` は、
置換を `""` のまま持つ 1 つの action になる。`(` への置換 1 つに畳むと、同じ範囲を `(` に
置換する別 cop と同じ置換として合流し、本家なら衝突で捨てられる側の `)` まで入る。
`puts(foo bar { 1 })` では Lint/AmbiguousBlockAssociation と Style/NestedParenthesizedCalls が
括弧を 2 組足し、構文の安全網が書き込みを止めていた。1 つの offense の挿入が別々の範囲に
掛かるときは、`Offense::corrected_by_all_anchored` と `InsertionAnchor` で範囲と向きを編集ごとに
渡す。空の範囲への `insert_after` (`Util#args_end`) は向きでしか表せない。

**字下げの挿入アンカーは空範囲になる。** 本家の `AlignmentCorrector` は行頭への
挿入を `start..start` にアンカーする。診断全体を使うと、同じパスの空白削除と
誤って衝突し、補正順序や 200 パス到達時の出力が変わる。最終出力だけでなく、
字下げと行末空白が同じパスで直ることも検査する。

**無限ループの原因一覧は補正可否と登録順にも依存する。** 最後のパスでは未補正でも
補正機能を持つ cop を列挙する。本家の Registry の順序は通常の名前順と一部異なる。
`config/autocorrect_support.json` を出典付きで再生成し、全 609 cop の対応を検査する。

## Conformance measurement

2026-10-22 の CLI 掃引は **登録 609 / 発火 609 / 全項目一致 609**。
測定可能な 42,433 入力・1,543 設定群に、索引 174 ケースとファイル名依存 2 ケースを加えた。
本家 CLI が設定を拒む 6 設定群・161 入力は未測定として分ける。
テスト関数は Rust 2,328・Ruby 82・検証ツール 15 の計 2,425 件。
このメンテナンスで追加したものは 15 件で、fixture 内の入力数とは分けて数える。
実コードの既知差分まで解消したという意味ではない。固定 SHA と数値の正本は
`CONFORMANCE.md` とし、README のテーブルは必ず計測結果から生成する。

**--show-cops も Ruby の YAML 1.1 で再読込する。** 説明文のバッククォートや
「: 」だけでなく、no / yes / on の暗黙変換、NEL の正規化、制御文字にも注意する。
全 cop の出力を解析し、カスタム説明を設定へ戻して同じ文字列を保つことを検査する。

**プロジェクト索引は実行全体で共有する。** `AllCops/UseProjectIndex` が有効なとき、
`src/project_index.rs` が検査対象と設定ルート配下の Ruby ソースを索引化する。
`Lint/DeprecatedReference`・`Lint/NameTypo`・`Lint/UnusedPrivateMethod` と既存 12 cop が使う。
別ファイルの変更でも診断が変わるため、索引を使う実行ではファイル単位の結果キャッシュを
利用しない。`ProjectIndexIncludesGems` の gem ソース取得は Ruby ラッパーが渡す Bundler
環境がある場合だけ行い、通常のネイティブ実行は Ruby を起動せず完結させる。

索引の回帰は `tests/project_index.rs` と、本家 RuboCop 1.89.0 / rubydex 0.4.1 の CLI から
記録した 174 ケースで検査する。`make project-index-fixtures` で期待値を再録できる。
索引の名前解決を cop 側の AST の意味へ広げる前に、本家が実際にどの名前と lexical scope を
索引へ渡しているかを確認する。例として `Lint/InheritException` の未解決定数名は末尾の
名前だけであり、見た目の完全修飾名へ変えると検出が変わる。

**Conformance tools must reject incomplete evidence.** `scripts/conformance_table.rb` compares
offense multisets, including `length`, and includes the run identity in each key: a missing offense
in one input must not cancel an extra offense in another. Both table and diff scripts require
complete nonzero inspections and identical target path sets before comparing offenses. An
unexercised cop is not exact. The table exits 0 only when every listed cop fired and matched,
1 for differences or unexercised cops, and 2 when the measurement is incomplete.

**CRLF 掃引も全項目で比較する。** cop ごとの件数が一致しても `last_column` だけの
退行は検出できない。`crlf_probe.sh` は `compare_reports.py` で検査完了・対象パスと
診断の全フィールドを多重集合として比較する。差分は終了コード 1、未測定は 2 とし、
JSON・stderr・終了コードを `OUT_DIR` に残す。比較処理を変えたら、一致する正例に加え、
`last_column` だけを変えた陰性対照が必ず失敗することを確認する。

**補正ゲートは `make corpus-check` で再現する。** `scripts/conformance/` が
`safe` / `unsafe` 別の複製でパス集合、補正バイト列、構文、冪等性を検査する。
終了 2 はそのまま記録する。登録済みパスの構文保護メッセージだけが出ている場合に限り
比較を続け、マニフェストの両出力ハッシュと件数まで照合する。無条件には許容しない。
構文検査ツールの I/O 例外は終了 2 とし、本家だけが壊れる分類結果の終了 1 と混ぜない。
本家の内部エラーは stderr で判定する。stdout のソース行には同じ文言が現れ得る。
補正した実行から progress と JSON を両方保存し、構文の分類にも正の完了件数と同じ対象パス集合を要求する。
出力先は複製へ移動する前に実パスへ正規化する。呼び出し元の Ruby を別の導入先で上書きしない。途中停止した本家の複製を
補正済み全体と見なすと、未検査の原本と比較して候補側だけの破壊と誤分類する。
本家が完走しない場合は --candidate-only で候補の安全性を分けて測り、互換性は主張しない。

2026-10-22 の速度表は専用実行 `39778331289` の結果から生成した。
Ruby 4.0.8 / RuboCop 1.92.1 と共通393 copで測り、挙動の仕様版は1.89.0のままにする。
CPU・コーパスSHA・全計測の終了コードと診断数は `docs/benchmarks/2026-10-22.json` が正本。
検出内容が違う新版との時間を、同じ仕事での速度倍率として説明しない。

**速度は `make benchmark` と専用の Benchmark ワークフローで測る。**
新しい本家との比較でも挙動のオラクルは変えない。計測する全実行で検査完了と全診断の
基準 JSON 一致を確認し、時間・CPU 時間・終了コード・負荷・検出数を残す。
入力の複製はプロジェクトの外に置く。出力配下へ clone すると親の gemspec の
Ruby 2.6 指定を拾うことがあり、両ツールが同じ誤った版でも比較が成立して見える。
構文エラーの stdin から実効版を両 CLI に答えさせ、固定コーパスで確認した値
（Rails 3.3、Mastodon 4.0、残り 2.7）と照合する。読めない場合も測定を止める。

`CONFORMANCE.md` records offense-by-offense comparisons against five pinned corpora (18,251 files).
The commits are pinned because the numbers move with them. Anything that changes file discovery or
path matching can move the target-file lists, so re-measure after touching `src/config/paths.rs`.
The behavioral oracle stays RuboCop 1.89.0 even when the corpus checkout has advanced to a later
release. If a measurement script's version guard reads that checkout as its source version, put the
installed 1.89.0 gem's `lib/rubocop/version.rb` in a temporary `CORPUS_ROOT` and pass the pinned
corpus by absolute path. Keep the guard enabled and record the gem version and corpus commit.

## Versioning

`Cargo.toml` and `lib/sonicop/version.rb` must agree; `make version-check` enforces it and CI fails
otherwise. 公開版の番号は Release ワークフローが自動採番する。Cargo.toml と Ruby の版を手で変更しない。
