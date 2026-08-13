//! cop 単体の回帰テスト。サブプロセスを起こさず `engine::inspect_source` を
//! 直接呼ぶので、1 ケースあたりの費用が低く、offense の集合を完全一致で見られる。
//!
//! ソースはキャレット注記付きで書く。注記は直前のソース行を指し、行頭の空白で
//! カラム、`^` の本数でレンジの長さを表す。`^{}` は長さ 0 のレンジ。
//!
//! **期待値は本家 RuboCop 1.89.0 の実出力を根拠にしている。** sonicop の現在の
//! 出力を写すと既存のバグを仕様として焼き付けてしまうため、`--only <cop>
//! --format json` の実測と突き合わせて確定させた。
//!
//! ここには**本家と一致している**ことの回帰テストだけを置く。陰性ケース、
//! autocorrect、sonicop 側の取り違えの回帰などが対象。本家と食い違っている
//! ものは `tests/conformance.rs` のケース一覧と既知差分マニフェストで扱う。

mod support;

use sonicop::diagnostic::Severity;
use support::annotation::Annotation;
use support::case::{CopCase, expect_correction, expect_no_offenses, expect_offense};

/// ハーネス自身の担保。ここが壊れると cop のテストが全部あてにならない。
mod harness {
    use super::*;

    #[test]
    fn caret_notation_yields_line_column_length_and_message() {
        let case = CopCase::annotated(
            "Style/RedundantReturn",
            r#"
            def foo
              return 1
              ^^^^^^ Redundant `return` detected.
            end
            "#,
        );
        assert_eq!(case.source, "def foo\n  return 1\nend\n");
        assert_eq!(
            case.expected,
            Some(vec![Annotation::new(
                2,
                3,
                6,
                "Redundant `return` detected."
            )])
        );
    }

    #[test]
    fn empty_range_notation_round_trips() {
        let case = CopCase::annotated(
            "Style/FrozenStringLiteralComment",
            "puts 1\n^{} Missing frozen string literal comment.\n",
        );
        assert_eq!(case.source, "puts 1\n");
        let expected = case.expected.expect("注記が読めていない");
        assert_eq!((expected[0].column, expected[0].length), (1, 0));
        assert_eq!(
            support::annotation::render(&case.source, &expected),
            "puts 1\n^{} Missing frozen string literal comment.\n"
        );
    }

    #[test]
    fn escaped_carets_stay_source_lines() {
        let case = CopCase::annotated("Style/Semicolon", "a = 1\n\\^^^ not an annotation\n");
        assert_eq!(case.source, "a = 1\n\\^^^ not an annotation\n");
        assert_eq!(case.expected, Some(Vec::new()));
    }

    #[test]
    fn placeholders_expand_to_matching_widths() {
        let case = CopCase::annotated_with(
            "Lint/UselessAssignment",
            "%{name} = 1\n^{name} Useless assignment to variable - `%{name}`.\n",
            &[("name", "value")],
        );
        assert_eq!(case.source, "value = 1\n");
        assert_eq!(
            case.expected,
            Some(vec![Annotation::new(
                1,
                1,
                5,
                "Useless assignment to variable - `value`."
            )])
        );
    }

    #[test]
    fn abbreviated_messages_match_by_prefix() {
        expect_offense(
            "Layout/TrailingWhitespace",
            "value = 1  \n         ^^ Trailing whitespace [...]\n",
        );
    }

    // 部分一致ではなく集合の完全一致であることの担保。1 件だけ書いた期待に
    // 2 件目が出たら落ちなければならない。差分の分類まで見る。
    #[test]
    #[should_panic(expected = "[false_positive]")]
    fn extra_offenses_fail_the_comparison() {
        expect_offense(
            "Layout/TrailingWhitespace",
            "a = 1  \n     ^^ Trailing whitespace detected.\nb = 2  \n",
        );
    }

    #[test]
    #[should_panic(expected = "[range]")]
    fn wrong_column_fails_the_comparison() {
        expect_offense(
            "Layout/TrailingWhitespace",
            "a = 1  \n    ^^^ Trailing whitespace detected.\n",
        );
    }

    #[test]
    #[should_panic(expected = "[message]")]
    fn wrong_message_fails_the_comparison() {
        expect_offense(
            "Layout/TrailingWhitespace",
            "a = 1  \n     ^^ Trailing whitespace.\n",
        );
    }

    #[test]
    fn target_ruby_version_is_selectable_per_case() {
        CopCase::annotated("Lint/Syntax", "source[..position]\n")
            .target_ruby("2.7")
            .run();
        let report = CopCase::annotated("Lint/Syntax", "source[..position]\n")
            .target_ruby("2.6")
            .without_offense_check()
            .inspect();
        assert!(
            report
                .offenses
                .iter()
                .any(|offense| offense.message.contains("unexpected token tDOT2")),
            "Ruby 2.6 では beginless range が構文エラーになるべき: {:?}",
            report.offenses
        );
    }

    #[test]
    fn configuration_is_selectable_per_case() {
        CopCase::annotated(
            "Style/StringLiterals",
            r#"
            value = 'single'
                    ^^^^^^^^ Prefer double-quoted strings [...]
            "#,
        )
        .config("Style/StringLiterals:\n  EnforcedStyle: double_quotes\n")
        .run();
        expect_no_offenses("Style/StringLiterals", "value = 'single'\n");
    }
}

mod layout {
    use super::*;

    #[test]
    fn empty_line_after_magic_comment_accepts_a_blank_line() {
        expect_no_offenses(
            "Layout/EmptyLineAfterMagicComment",
            "# encoding: utf-8\n\nputs 1\n",
        );
    }

    #[test]
    fn end_of_line_accepts_lf() {
        CopCase::new("Layout/EndOfLine", "x = 1\n", Vec::new())
            .config("Layout/EndOfLine:\n  EnforcedStyle: lf\n")
            .run();
    }

    #[test]
    fn line_length() {
        CopCase::annotated(
            "Layout/LineLength",
            r#"
            x = 1234567890
                      ^^^^ Line is too long. [14/10]
            "#,
        )
        .config("Layout/LineLength:\n  Max: 10\n")
        .run();
        CopCase::new("Layout/LineLength", "x = 1\n", Vec::new())
            .config("Layout/LineLength:\n  Max: 10\n")
            .run();
    }

    /// URI と修飾名の免除は「上限より前で始まり、行末で終わる」ことが条件で、
    /// 行末で終わらないものは超過部分の**直後**から報告される。`]` を巻き込んだ
    /// YARD リンクは `URI.parse` が弾くので URI 扱いされず、上限の位置から始まる。
    /// cop ディレクティブのある行はディレクティブを除いた長さで測り直す。
    ///
    /// 期待値は本家 1.89.0 の `--only Layout/LineLength --format json` 実測。
    #[test]
    fn line_length_exempts_only_what_runs_to_the_end_of_the_line() {
        let source = concat!(
            "# see https://example.com/aaaaaaaaaaaaaaaaaaaa\n",
            "# see https://example.com/aaaaaaaaaaaaaaaaaaaaaaaa word\n",
            "# {Guide}[https://example.com/aaaaaaaaaaaa#sec]\n",
            "# note Foo::BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB and more\n",
            "value = RuboCop::Cop::Layout::LineLength.new(1)\n",
            "x = 1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9 + 10 # rubocop:disable Style/Lambda\n",
        );
        CopCase::new(
            "Layout/LineLength",
            source,
            vec![
                Annotation::new(2, 51, 5, "Line is too long. [55/40]"),
                Annotation::new(3, 41, 7, "Line is too long. [47/40]"),
                Annotation::new(4, 51, 9, "Line is too long. [59/40]"),
                Annotation::new(6, 41, 2, "Line is too long. [42/40]"),
            ],
        )
        .config("Layout/LineLength:\n  Max: 40\n")
        .run();
    }

    /// エンドレスメソッドは通常のメソッドへ書き直せば短くできるので、本家は
    /// 免除の判定より先に報告する。行末で終わる修飾名があっても、長さの原因が
    /// cop ディレクティブでも関係なく、行全体の長さで出る。
    ///
    /// 期待値は本家 1.89.0 の `--only Layout/LineLength --format json` 実測。
    #[test]
    fn line_length_reports_endless_methods_before_any_exemption() {
        let source = concat!(
            "def opts = RuboCop::Cop::Layout::LineLen.new\n",
            "def self.o = RuboCop::Cop::Layout::LineL.new\n",
            "value = RuboCop::Cop::Layout::LineLength.new(1)\n",
            "def opts2 = 1 + 2 + 3 + 4 # rubocop:disable Style/Lambda\n",
        );
        CopCase::new(
            "Layout/LineLength",
            source,
            vec![
                Annotation::new(1, 41, 4, "Line is too long. [44/40]"),
                Annotation::new(2, 41, 4, "Line is too long. [44/40]"),
                Annotation::new(4, 41, 16, "Line is too long. [56/40]"),
            ],
        )
        .target_ruby("3.0")
        .config("Layout/LineLength:\n  Max: 40\n")
        .run();
    }

    /// 本家は `String#length` で数えるので、全角 5 文字の行も 7 文字でしかない。
    /// 表示幅で数えると本家が見逃す行を報告してしまう。
    #[test]
    fn line_length_counts_characters_not_display_width() {
        CopCase::new("Layout/LineLength", "# あああああ\n", Vec::new())
            .config("Layout/LineLength:\n  Max: 7\n")
            .run();
    }

    #[test]
    fn space_after_comma() {
        expect_offense(
            "Layout/SpaceAfterComma",
            r#"
            [1,2]
              ^ Space missing after comma.
            "#,
        );
        expect_no_offenses("Layout/SpaceAfterComma", "[1, 2]\n");
        expect_correction("Layout/SpaceAfterComma", "[1,2]\n", "[1, 2]\n");
    }

    #[test]
    fn space_around_operators() {
        expect_offense(
            "Layout/SpaceAroundOperators",
            r#"
            1+2
             ^ Surrounding space missing for operator `+`.
            "#,
        );
        expect_no_offenses("Layout/SpaceAroundOperators", "1 + 2\n");
        expect_correction("Layout/SpaceAroundOperators", "1+2\n", "1 + 2\n");
    }

    #[test]
    fn space_inside_parens() {
        expect_offense(
            "Layout/SpaceInsideParens",
            r#"
            puts( 1)
                 ^ Space inside parentheses detected.
            "#,
        );
        expect_no_offenses("Layout/SpaceInsideParens", "puts(1)\n");
        expect_correction("Layout/SpaceInsideParens", "puts( 1)\n", "puts(1)\n");
    }

    #[test]
    fn trailing_empty_lines_accepts_a_single_final_newline() {
        expect_no_offenses("Layout/TrailingEmptyLines", "x = 1\n");
    }

    /// 最終改行が無いときはファイル末尾 (最後の文字の「次」) を指す。本家は末尾の空白の
    /// 開始位置から報告範囲を組み立てるので、最後の文字そのものを指すと 1 桁ずれる。
    #[test]
    fn trailing_empty_lines_points_past_the_last_character_when_the_newline_is_missing() {
        CopCase::new(
            "Layout/TrailingEmptyLines",
            "x = 1",
            vec![Annotation::new(1, 6, 0, "Final newline missing.")],
        )
        // 空範囲なので本家の last_column は開始位置の 1 つ手前になる。
        .locations(&[(1, 6, 1, 5)])
        .run();
    }

    /// 余分な空行があるときは、余った 1 行目を指す。文言も本家の
    /// 「N trailing blank lines detected.」に揃える。
    #[test]
    fn trailing_empty_lines_counts_the_extra_blank_lines() {
        CopCase::new(
            "Layout/TrailingEmptyLines",
            "x = 1\n\n",
            // レンジは改行 1 文字ぶんだが、次の行まで跨るのでキャレットは
            // 本家 `column_length` と同じく開始行の残り幅 (= 0) になる。
            vec![Annotation::new(2, 1, 0, "1 trailing blank lines detected.")],
        )
        // 範囲が改行で終わるので、本家の last_line はその次の行になる。
        .locations(&[(2, 1, 3, 1)])
        .run();
    }

    #[test]
    fn trailing_whitespace() {
        expect_offense(
            "Layout/TrailingWhitespace",
            "x = 1  \n     ^^ Trailing whitespace detected.\n",
        );
        expect_no_offenses("Layout/TrailingWhitespace", "x = 1\n");
        expect_correction("Layout/TrailingWhitespace", "x = 1  \n", "x = 1\n");
    }

    /// ヒアドキュメント内の行末空白は文字列の一部なので、消すとプログラムが変わる。
    /// 本家は「字下げとして剥がされる分」だけを消し、それ以外は補間で保存する。
    /// 期待値はすべて本家 1.89.0 の `-A` 実出力から取得。
    #[test]
    fn trailing_whitespace_inside_a_heredoc_is_preserved() {
        // 内容のある行では、空白を補間に包んで残す。
        expect_correction(
            "Layout/TrailingWhitespace",
            "x = <<~RUBY\n  a  \n  b\nRUBY\n",
            "x = <<~RUBY\n  a#{'  '}\n  b\nRUBY\n",
        );
        // 非補間ヒアドキュメントでは包めないので、報告だけして直さない。
        expect_correction(
            "Layout/TrailingWhitespace",
            "x = <<~'RUBY'\n  a  \n  b\nRUBY\n",
            "x = <<~'RUBY'\n  a  \n  b\nRUBY\n",
        );
        // 空白だけの行は、字下げに収まる分なら消す。
        expect_correction(
            "Layout/TrailingWhitespace",
            "x = <<~RUBY\n  a\n  \n  b\nRUBY\n",
            "x = <<~RUBY\n  a\n\n  b\nRUBY\n",
        );
        // 字下げを超える分は、超えた分だけを包んで残す。
        expect_correction(
            "Layout/TrailingWhitespace",
            "x = <<~RUBY\n  a\n      \n  b\nRUBY\n",
            "x = <<~RUBY\n  a\n  #{'    '}\n  b\nRUBY\n",
        );
    }
}

mod lint {
    use super::*;

    #[test]
    fn duplicate_methods_accepts_a_single_definition() {
        expect_no_offenses("Lint/DuplicateMethods", "def foo\nend\n");
    }

    /// 本家は名前空間付きのメソッド名とファイルパスを文言に出し、`def` キーワードから
    /// メソッド名の末尾までを指す。
    #[test]
    fn duplicate_methods_names_the_scope_and_points_from_the_def_keyword() {
        CopCase::annotated(
            "Lint/DuplicateMethods",
            r#"
            def foo
            end
            def foo
            ^^^^^^^ Method `Object#foo` is defined at both example.rb:1 and example.rb:3.
            end
            "#,
        )
        .locations(&[(3, 1, 3, 7)])
        .run();
    }

    /// `attr_reader` は本家では読み取りメソッドの定義として数えられるので、同名の `def`
    /// と衝突する。`alias foo foo` の自己 alias は再定義を意図的と宣言する印なので免除。
    #[test]
    fn duplicate_methods_counts_attr_readers_and_honours_the_self_alias_trick() {
        CopCase::annotated(
            "Lint/DuplicateMethods",
            r#"
            class Klass
              attr_reader :ra
              def ra; end
              ^^^^^^ Method `Klass#ra` is defined at both example.rb:2 and example.rb:3.
              alias same same
              def same; end
            end
            "#,
        )
        .locations(&[(3, 3, 3, 8)])
        .run();
    }

    /// `Class.new` ブロック内の定義は無名クラスに載る。本家はブロックの置かれ方から
    /// スコープ ID を作り、それが決まらないブロック同士は同じスコープに寄せる。裸で並べた
    /// 2 つは寄せられて衝突し、`.new` を続けて値にしたものは別スコープになる。
    #[test]
    fn duplicate_methods_scopes_anonymous_class_blocks_by_their_surroundings() {
        CopCase::annotated(
            "Lint/DuplicateMethods",
            r#"
            Class.new do
              def m; end
            end
            Class.new do
              def m; end
              ^^^^^ Method `Object#m` is defined at both example.rb:2 and example.rb:5.
            end
            "#,
        )
        .locations(&[(5, 3, 5, 7)])
        .run();
        expect_no_offenses(
            "Lint/DuplicateMethods",
            "a = Class.new do\n  def m; end\nend.new\nb = Class.new do\n  def m; end\nend.new\n",
        );
    }

    /// `if` の下の定義はプラットフォーム別の出し分けである可能性が高く、本家は両方とも
    /// 見逃す。
    #[test]
    fn duplicate_methods_ignores_definitions_under_a_condition() {
        expect_no_offenses(
            "Lint/DuplicateMethods",
            "if RUBY_PLATFORM\n  def m; end\nelse\n  def m; end\nend\n",
        );
    }

    #[test]
    fn syntax_accepts_valid_source() {
        expect_no_offenses("Lint/Syntax", "puts 1\n");
    }

    #[test]
    fn unused_block_argument_accepts_a_referenced_argument() {
        expect_no_offenses("Lint/UnusedBlockArgument", "[1].each { |x| puts x }\n");
    }

    /// 引数がどれも参照されていないブロックには「省略できる」側の文面が出る。複数あるときは
    /// 「全部省略できる」に変わる。
    #[test]
    fn unused_block_argument_suggests_omitting_unreferenced_arguments() {
        expect_offense(
            "Lint/UnusedBlockArgument",
            r#"
            [1].each { |x, y| puts 1 }
                        ^ Unused block argument - `x`. You can omit all the arguments if you don't care about them.
                           ^ Unused block argument - `y`. You can omit all the arguments if you don't care about them.
            "#,
        );
    }

    /// lambda はアリティが呼び出し側に効くので「省略できる」とは言えず、`_` 前置の案内に
    /// proc への言い換えが足される。`->` リテラルは block/do_block とは別ノードなので、
    /// ここを取りこぼすと lambda の引数が丸ごと検査対象から外れる。
    #[test]
    fn unused_block_argument_covers_lambda_literals() {
        expect_offense(
            "Lint/UnusedBlockArgument",
            r#"
            ->(env) { [200, {}, []] }
               ^^^ Unused block argument - `env`. If it's necessary, use `_` or `_env` as an argument name to indicate that it won't be used. Also consider using a proc without arguments instead of a lambda if you want it to accept any arguments but don't care about them.
            "#,
        );
    }

    /// `binding` はスコープ全体を呼び出し側へ渡すので、本家は届く変数を全部「参照済み」に
    /// する。
    #[test]
    fn unused_block_argument_treats_a_binding_call_as_referencing_everything() {
        expect_no_offenses(
            "Lint/UnusedBlockArgument",
            "lambda { |message, callstack| bindings << binding }\n",
        );
    }

    /// `|x; y|` の `y` はブロックローカル変数で、文面も別。代入されていれば役目を
    /// 果たしているので報告しない。
    #[test]
    fn unused_block_argument_reports_untouched_block_locals() {
        expect_offense(
            "Lint/UnusedBlockArgument",
            r#"
            [1].each { |x; y| puts x }
                           ^ Unused block local variable - `y`.
            "#,
        );
        expect_no_offenses(
            "Lint/UnusedBlockArgument",
            "[1].each { |x; y| y = 1; puts x }\n",
        );
    }

    /// 代入は参照ではない。`x = 1` しかしていないブロック引数は未使用のまま。
    #[test]
    fn unused_block_argument_does_not_count_an_assignment_as_a_reference() {
        expect_offense(
            "Lint/UnusedBlockArgument",
            r#"
            [1].each { |x| x = 1 }
                        ^ Unused block argument - `x`. You can omit the argument if you don't care about it.
            "#,
        );
    }

    #[test]
    fn useless_assignment() {
        expect_offense(
            "Lint/UselessAssignment",
            r#"
            x = 1
            ^ Useless assignment to variable - `x`.
            "#,
        );
        expect_no_offenses("Lint/UselessAssignment", "x = 1\nputs x\n");
    }

    /// 本家のパターンは `(const {nil? cbase} {:Fixnum :Bignum})`。`::Fixnum` は一致して
    /// `::` ごと指すが、名前空間の付いた `Foo::Fixnum` は別の定数なので見逃す。
    #[test]
    fn unified_integer_matches_only_a_top_level_constant() {
        CopCase::annotated(
            "Lint/UnifiedInteger",
            r#"
            1.is_a?(::Bignum)
                    ^^^^^^^^ Use `Integer` instead of `Bignum`.
            "#,
        )
        .run();
        expect_no_offenses("Lint/UnifiedInteger", "1.is_a?(Foo::Fixnum)\n");
        expect_correction(
            "Lint/UnifiedInteger",
            "1.is_a?(::Fixnum)\n",
            "1.is_a?(::Integer)\n",
        );
    }

    /// 補正は `remove` 3 回 (selector / dot / cbase) で、置換 1 個ではない。
    #[test]
    fn big_decimal_new_removes_the_selector_the_dot_and_the_cbase() {
        expect_correction(
            "Lint/BigDecimalNew",
            "::BigDecimal.new(1)\nBigDecimal.new(2)\n",
            "BigDecimal(1)\nBigDecimal(2)\n",
        );
        expect_no_offenses("Lint/BigDecimalNew", "Foo::BigDecimal.new(1)\n");
    }

    /// `rand(-1)` は本家では `(int -1)` 1 個に畳まれる。`rand(2)` と裸の `rand` は無傷。
    #[test]
    fn rand_one_folds_the_sign_into_the_literal() {
        CopCase::annotated(
            "Lint/RandOne",
            r#"
            Kernel.rand(-1)
            ^^^^^^^^^^^^^^^ `Kernel.rand(-1)` always returns `0`. Perhaps you meant `rand(2)` or `rand`?
            "#,
        )
        .run();
        expect_no_offenses("Lint/RandOne", "rand(2)\nrand\nrand(1, 2)\nFoo.rand(1)\n");
    }

    /// 両辺とも `object_id` の呼び出しでなければ一致しない。ワイルドカードはレシーバ無しに
    /// 当たらないので、裸の `object_id` を含む比較は offense にならない。
    #[test]
    fn identity_comparison_needs_a_receiver_on_both_sides() {
        expect_no_offenses(
            "Lint/IdentityComparison",
            "foo.object_id == object_id\nobject_id == bar.object_id\nfoo.object_id == bar\n",
        );
        expect_correction(
            "Lint/IdentityComparison",
            "foo.object_id != baz.object_id\n",
            "!foo.equal?(baz)\n",
        );
    }

    /// `Socket` だけは置き換え先が別クラスなので補正が付かない。`attr` は引数が 2 個で
    /// 2 番目が真偽値のときだけ、`ENV.freeze` は式全体が `ENV` になる。
    #[test]
    fn deprecated_class_methods_leaves_socket_uncorrected() {
        CopCase::annotated(
            "Lint/DeprecatedClassMethods",
            r#"
            Socket.gethostbyaddr(host)
            ^^^^^^^^^^^^^^^^^^^^ `Socket.gethostbyaddr` is deprecated in favor of `Addrinfo#getnameinfo`.
            "#,
        )
        .correctable(false)
        .run();
        expect_no_offenses(
            "Lint/DeprecatedClassMethods",
            "attr :name\nattr :name, other\nENV.freeze(1)\nFile.exists?\n",
        );
        expect_correction(
            "Lint/DeprecatedClassMethods",
            "ENV.freeze\nENV.dup\nattr :name, false\niterator?\n",
            "ENV\nENV.to_h\nattr_reader :name\nblock_given?\n",
        );
    }

    /// メッセージは `::URI` と `URI` を書き分ける。
    #[test]
    fn uri_escape_unescape_spells_the_cbase_in_the_message() {
        expect_offense(
            "Lint/UriEscapeUnescape",
            r#"
            ::URI.decode(x)
            ^^^^^^^^^^^^^^^ `::URI.decode` method is obsolete and should not be used. Instead, use `CGI.unescape`, `URI.decode_www_form` or `URI.decode_www_form_component` depending on your specific use case.
            "#,
        );
        expect_no_offenses(
            "Lint/UriEscapeUnescape",
            "Foo::URI.escape(x)\nCGI.escape(x)\n",
        );
    }

    /// 引数の無い形は置き換え先にも引数を付けない。
    #[test]
    fn uri_regexp_keeps_the_argument_list_only_when_one_was_written() {
        expect_correction(
            "Lint/UriRegexp",
            "::URI.regexp\nURI.regexp(a, b)\n",
            "::URI::DEFAULT_PARSER.make_regexp\nURI::DEFAULT_PARSER.make_regexp(a)\n",
        );
    }

    /// `if` の両辺がどちらも脱出するときだけ、その後ろが到達不能になる。
    #[test]
    fn unreachable_code_needs_both_branches_of_a_condition() {
        expect_offense(
            "Lint/UnreachableCode",
            r#"
            def m
              if c
                return
              else
                raise
              end
              dead
              ^^^^ Unreachable code detected.
            end
            "#,
        );
        expect_no_offenses(
            "Lint/UnreachableCode",
            "def m\n  if c\n    return\n  end\n  alive\nend\n",
        );
    }

    /// ファイル内で `raise` が定義されると、それ以降の裸の `raise` は脱出と見なされない。
    /// `instance_eval` の中も、`self` が何か分からないので同じく見逃す。
    #[test]
    fn unreachable_code_honours_a_redefinition_and_instance_eval() {
        expect_no_offenses(
            "Lint/UnreachableCode",
            "def foo\n  def raise\n  end\n  x\nend\ndef bar\n  raise\n  y\nend\n",
        );
        expect_no_offenses(
            "Lint/UnreachableCode",
            "x.instance_eval do\n  raise\n  y\nend\n",
        );
    }

    /// `AllowedPatterns` の既定は RSpec の `exactly(2).times` を逃がす。`break` の前に
    /// `next` があるループも、2 周目に入り得るので offense にならない。
    #[test]
    fn unreachable_loop_honours_allowed_patterns_and_a_preceding_next() {
        expect_offense(
            "Lint/UnreachableLoop",
            r#"
            2.times { raise ArgumentError }
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ This loop will have at most one iteration.
            "#,
        );
        expect_no_offenses(
            "Lint/UnreachableLoop",
            "exactly(2).times { raise StandardError }\nloop do\n  next if a\n  break\nend\n",
        );
    }

    /// `ensure` の中の `return` は、内側の `def` と lambda の中に書かれたものだけ免除される。
    /// 素のブロックと `proc` の中では外側のメソッドから返ってしまう。
    #[test]
    fn ensure_return_only_excuses_an_inner_def_or_lambda() {
        expect_offense(
            "Lint/EnsureReturn",
            r#"
            def m
              x
            ensure
              [1].each { return 1 }
                         ^^^^^^^^ Do not return from an `ensure` block.
              proc { return 2 }
                     ^^^^^^^^ Do not return from an `ensure` block.
            end
            "#,
        );
        expect_no_offenses(
            "Lint/EnsureReturn",
            "def m\n  x\nensure\n  lambda { return 1 }\n  -> { return 2 }\n  def inner\n    return 3\n  end\nend\n",
        );
    }

    /// `ensure` を消すのはキーワードだけで、空行はそのまま残る。
    #[test]
    fn empty_ensure_removes_only_the_keyword() {
        expect_correction(
            "Lint/EmptyEnsure",
            "def m\n  x\nensure\nend\n",
            "def m\n  x\n\nend\n",
        );
        expect_no_offenses("Lint/EmptyEnsure", "def m\n  x\nensure\n  y\nend\n");
    }

    /// キーは本家のノード等価で比べる。`'a'` と `:a` は型が違うので別のキー、`1` と `1.0`
    /// も別、`0x1` は `1` と同じ。`**splat` はキーを持たない。
    #[test]
    fn duplicate_hash_key_compares_the_parsed_value() {
        expect_offense(
            "Lint/DuplicateHashKey",
            r#"
            { 'a' => 1, a: 2, :a => 3, 1 => 4, 1.0 => 5, 0x1 => 6 }
                              ^^ Duplicated key in hash literal.
                                                         ^^^ Duplicated key in hash literal.
            "#,
        );
        expect_no_offenses(
            "Lint/DuplicateHashKey",
            "{ **other, x: 1, y: 2 }\n{ a => 1, a => 2 }\n",
        );
    }

    /// 条件は `when` をまたいで数える。
    #[test]
    fn duplicate_case_condition_counts_across_when_branches() {
        expect_offense(
            "Lint/DuplicateCaseCondition",
            r#"
            case x
            when 1, 2
              a
            when 2, 3
                 ^ Duplicate `when` condition detected.
              b
            end
            "#,
        );
        expect_no_offenses(
            "Lint/DuplicateCaseCondition",
            "case x\nwhen 1\n  a\nwhen 2\n  b\nelse\n  c\nend\n",
        );
    }

    /// `unless` は `if?` にも `elsif?` にも当たらないので、鎖をたどらない。
    #[test]
    fn duplicate_elsif_condition_ignores_unless() {
        expect_no_offenses(
            "Lint/DuplicateElsifCondition",
            "unless a\n  x\nelse\n  y\nend\na if a\n",
        );
        expect_offense(
            "Lint/DuplicateElsifCondition",
            r#"
            if a
              w
            elsif b
              x
            elsif a
                  ^ Duplicate `elsif` condition detected.
              y
            end
            "#,
        );
    }

    /// 例外は節をまたいで数える。`rescue A, B` の後の `rescue B` は重複。
    #[test]
    fn duplicate_rescue_exception_counts_across_clauses() {
        expect_offense(
            "Lint/DuplicateRescueException",
            r#"
            begin
              x
            rescue A, B
              y
            rescue B
                   ^ Duplicate `rescue` exception detected.
              z
            end
            "#,
        );
        expect_no_offenses(
            "Lint/DuplicateRescueException",
            "begin\n  x\nrescue A\n  y\nrescue B\n  z\nend\n",
        );
    }

    /// 重複の単位は本家の親ノード。`if` と `else` に 1 文ずつ書かれた `require` は同じ
    /// `if` ノードにぶら下がるので重複するが、`when` の枝は枝ごとに別のノードになる。
    /// `require_relative` は別のメソッドなので `require` とは衝突しない。
    #[test]
    fn duplicate_require_groups_by_the_parser_parent() {
        expect_offense(
            "Lint/DuplicateRequire",
            r#"
            if cond
              require 'y'
            else
              require 'y'
              ^^^^^^^^^^^ Duplicate `require` detected.
            end
            "#,
        );
        expect_no_offenses(
            "Lint/DuplicateRequire",
            "require 'foo'\nrequire_relative 'foo'\ncase x\nwhen 1 then require 'z'\nwhen 2 then require 'z'\nend\n",
        );
    }

    /// 補正は行ごと (末尾の改行込み) 消す。レシーバ付きの `Kernel.require` も同じキーに
    /// 数えられる。
    #[test]
    fn duplicate_require_removes_the_whole_line() {
        expect_correction(
            "Lint/DuplicateRequire",
            "require 'a'\nrequire 'a'\nKernel.require 'a'\n",
            "require 'a'\n",
        );
    }

    /// `rescue` 節の付いた `begin ... end` は、本家では `kwbegin` が `rescue` ノード 1 個だけを
    /// 持つ。中の `return` は外から見えないので、その後ろは到達不能にならない。
    #[test]
    fn unreachable_code_stops_at_a_rescue_clause() {
        expect_offense(
            "Lint/UnreachableCode",
            r#"
            def n1
              begin
                return
                dead1
                ^^^^^ Unreachable code detected.
              rescue
                x
              end
              alive
            end
            "#,
        );
        expect_offense(
            "Lint/UnreachableCode",
            r#"
            def n2
              begin
                return
                dead3
                ^^^^^ Unreachable code detected.
              end
              dead4
              ^^^^^ Unreachable code detected.
            end
            "#,
        );
    }

    /// 同じ理由で、`begin ... rescue ... end` の中の `break` は外側のループから見えない。
    #[test]
    fn unreachable_loop_stops_at_a_rescue_clause() {
        expect_no_offenses(
            "Lint/UnreachableLoop",
            "[1].each do\n  break\nrescue\n  z\nend\nwhile c\n  begin\n    break\n  rescue\n    z\n  end\nend\n",
        );
    }

    /// `ensure` は本家では本体と後始末をまとめて 1 つのノードにする。どちらも 1 文なら
    /// 親が同じになるので、同じ `require` は重複と数えられる。
    #[test]
    fn duplicate_require_shares_the_parent_an_ensure_introduces() {
        expect_offense(
            "Lint/DuplicateRequire",
            r#"
            def m1
              require 'a'
            ensure
              require 'a'
              ^^^^^^^^^^^ Duplicate `require` detected.
            end
            "#,
        );
        expect_no_offenses(
            "Lint/DuplicateRequire",
            "def m2\n  require 'b'\n  require 'c'\nensure\n  require 'b'\nend\ndef m3\n  require 'd'\nrescue\n  require 'd'\nend\n",
        );
    }

    /// `BEGIN { }` と `END { }` も本家では文の並びを `begin` で包む。
    #[test]
    fn unreachable_code_covers_begin_and_end_blocks() {
        expect_offense(
            "Lint/UnreachableCode",
            r#"
            END {
              exit
              puts "x"
              ^^^^^^^^ Unreachable code detected.
            }
            "#,
        );
    }

    /// 既に空の括弧があるときは中へ、無いときは名前の後ろへ挿す。
    #[test]
    fn to_json_inserts_the_argument_where_the_parentheses_are() {
        expect_correction(
            "Lint/ToJSON",
            "def to_json\nend\ndef to_json()\nend\n",
            "def to_json(*_args)\nend\ndef to_json(*_args)\nend\n",
        );
        expect_no_offenses("Lint/ToJSON", "def to_json(a)\nend\ndef to_s\nend\n");
    }

    /// ブロックやメソッドの中の `return` はトップレベルではない。
    #[test]
    fn top_level_return_with_argument_ignores_inner_scopes() {
        expect_no_offenses(
            "Lint/TopLevelReturnWithArgument",
            "return\ndef m\n  return 1\nend\n[1].each { return 2 }\n",
        );
        expect_correction("Lint/TopLevelReturnWithArgument", "return 1\n", "return\n");
    }

    /// 引数が immutable なリテラルのときだけ。`{}` や `[]` は積み上げられる。
    #[test]
    fn each_with_object_argument_only_reports_immutable_literals() {
        expect_no_offenses(
            "Lint/EachWithObjectArgument",
            "x.each_with_object({}) { }\nx.each_with_object([]) { }\nx.each_with_object(y) { }\n",
        );
        expect_offense(
            "Lint/EachWithObjectArgument",
            r#"
            x&.each_with_object(nil) { |a, b| b }
            ^^^^^^^^^^^^^^^^^^^^^^^^ The argument to each_with_object cannot be immutable.
            "#,
        );
    }

    /// 本家のパターンは `reduce` に引数が 1 個あることを求める。裸の `reduce` は当たらない。
    #[test]
    fn next_without_accumulator_needs_a_seed_argument() {
        expect_no_offenses(
            "Lint/NextWithoutAccumulator",
            "[1, 2].reduce do |acc, e|\n  acc + e\n  next\nend\n[1, 2].reduce(:+)\n",
        );
    }

    /// `def` が最後の引数で、かつ引数が 2 個以上あるときだけ。
    #[test]
    fn trailing_comma_in_attribute_declaration_needs_a_preceding_name() {
        expect_no_offenses(
            "Lint/TrailingCommaInAttributeDeclaration",
            "attr_reader :a, :b\nattr_reader def foo\nend\n",
        );
        expect_correction(
            "Lint/TrailingCommaInAttributeDeclaration",
            "attr_accessor :foo,\ndef bar\nend\n",
            "attr_accessor :foo\ndef bar\nend\n",
        );
    }

    /// ブロックの引数は 0 個か 1 個まで。numblock の arity は `_1` だけのときに 1 で、
    /// `_2` まで読むと 2 になり当たらない。
    #[test]
    fn redundant_with_index_counts_the_block_parameters() {
        expect_no_offenses(
            "Lint/RedundantWithIndex",
            "ary.each_with_index { |x, i| p x }\nary.each_with_index { _1 + _2 }\n",
        );
        expect_correction(
            "Lint/RedundantWithIndex",
            "ary.each_with_index { _1 }\n",
            "ary.each { _1 }\n",
        );
    }

    /// `with_index` はレシーバ自身がレシーバを持つ呼び出しのときだけ冗長になる。
    #[test]
    fn redundant_with_index_needs_a_chained_receiver() {
        expect_no_offenses("Lint/RedundantWithIndex", "foo.with_index { |x| p x }\n");
        expect_correction(
            "Lint/RedundantWithIndex",
            "bar.each.with_index { |x| p x }\n",
            "bar.each { |x| p x }\n",
        );
    }

    /// `each_with_object` は引数 1 個・ブロック引数 1 個のときだけ。
    #[test]
    fn redundant_with_object_needs_one_plain_parameter() {
        expect_no_offenses(
            "Lint/RedundantWithObject",
            "ary.each_with_object([]) { |x, h| p x }\n",
        );
        expect_correction(
            "Lint/RedundantWithObject",
            "ary.each.with_object({}) { |x| p x }\n",
            "ary.each { |x| p x }\n",
        );
    }

    /// 補正は `rescue` の後ろを有効な例外だけで置き換える。全部無効なら空になる。
    #[test]
    fn rescue_type_keeps_the_valid_exceptions() {
        expect_correction(
            "Lint/RescueType",
            "begin\n  a\nrescue Foo, 'str', Bar\n  b\nrescue nil\n  c\nend\n",
            "begin\n  a\nrescue Foo, Bar\n  b\nrescue\n  c\nend\n",
        );
    }

    /// 三項演算子の条件が `&&` / `||` / `and` / `or` のときだけ。
    #[test]
    fn require_parentheses_needs_an_operator_keyword() {
        expect_no_offenses(
            "Lint/RequireParentheses",
            "foo a ? 1 : 2\nfoo?(a && b)\nfoo? a\n",
        );
    }

    /// 条件の位置でなければ `match_current_line` にならないので当たらない。
    #[test]
    fn regexp_as_condition_needs_a_condition_position() {
        expect_no_offenses("Lint/RegexpAsCondition", "x = /re/\n!/re2/\n");
        expect_correction(
            "Lint/RegexpAsCondition",
            "if /re/\n  p 1\nend\n",
            "if /re/ =~ $_\n  p 1\nend\n",
        );
    }

    /// `on_begin` は `begin ... end` (kwbegin) には来ない。
    #[test]
    fn empty_expression_ignores_begin_end() {
        expect_no_offenses("Lint/EmptyExpression", "begin\nend\n");
    }

    /// 既定値が引数自身を読むときだけ。別の名前なら当たらない。
    #[test]
    fn circular_argument_reference_needs_the_same_name() {
        expect_no_offenses(
            "Lint/CircularArgumentReference",
            "def foo(bar = baz)\nend\ndef qux(a: b)\nend\n",
        );
    }

    /// 引数付きの `to_s` は冗長ではない。レシーバが無いときは `self` に置き換える。
    #[test]
    fn redundant_string_coercion_needs_a_bare_to_s() {
        expect_no_offenses("Lint/RedundantStringCoercion", "puts 1.to_s(2)\n");
        expect_correction("Lint/RedundantStringCoercion", "warn to_s\n", "warn self\n");
    }

    /// 混ぜ込む対象は定数でなければならない。
    #[test]
    fn send_with_mixin_argument_needs_constant_arguments() {
        expect_no_offenses(
            "Lint/SendWithMixinArgument",
            "send(:include, foo)\nsend(:puts, Foo)\n",
        );
        expect_correction(
            "Lint/SendWithMixinArgument",
            "Klass.public_send('prepend', A::B)\n",
            "Klass.prepend A::B\n",
        );
    }

    /// 集合演算子で繋いだ比較は連鎖比較ではない。
    #[test]
    fn multiple_comparison_allows_set_operations() {
        expect_no_offenses("Lint/MultipleComparison", "p 1 >= 2 & 3 < 4\n");
        expect_correction(
            "Lint/MultipleComparison",
            "p x < y < z\n",
            "p x < y && y < z\n",
        );
    }

    /// 符号は本家のパーサがリテラルへ畳み込むので、レンジは符号から始まる。
    #[test]
    fn float_out_of_range_reports_the_folded_sign() {
        expect_offense(
            "Lint/FloatOutOfRange",
            "a = -1.0e400\n    ^^^^^^^^ Float out of range.\n",
        );
        expect_no_offenses("Lint/FloatOutOfRange", "a = 0.0\nb = 1.0\n");
    }

    /// 対象の版で既に読み込まれている機能だけ。`set` は 3.2 から。
    #[test]
    fn redundant_require_statement_follows_the_target_version() {
        expect_no_offenses(
            "Lint/RedundantRequireStatement",
            "require 'set'\nrequire 'json'\n",
        );
        expect_correction(
            "Lint/RedundantRequireStatement",
            "require 'enumerator'\nputs 1\n",
            "puts 1\n",
        );
    }

    /// 英数字を持たない語は意図的な記号とみなして数えない。
    #[test]
    fn percent_string_array_skips_punctuation_only_words() {
        expect_no_offenses("Lint/PercentStringArray", "a = %w[' \"]\n");
        expect_correction(
            "Lint/PercentStringArray",
            "a = %w[one, \"two\"]\n",
            "a = %w[one two]\n",
        );
    }

    #[test]
    fn percent_symbol_array_removes_the_punctuation() {
        expect_no_offenses("Lint/PercentSymbolArray", "a = %i[one two]\n");
        expect_correction(
            "Lint/PercentSymbolArray",
            "a = %i[:one, :two]\n",
            "a = %i[one two]\n",
        );
    }

    /// 接頭辞のあとに区切り文字が続くときだけ入れ子とみなす。
    #[test]
    fn nested_percent_literal_needs_a_delimiter_after_the_prefix() {
        expect_no_offenses("Lint/NestedPercentLiteral", "a = %w[%s]\nb = %w[%foo]\n");
    }

    /// エンコーディングが先頭にあれば正しい順序。
    #[test]
    fn ordered_magic_comments_accepts_the_encoding_first() {
        expect_no_offenses(
            "Lint/OrderedMagicComments",
            "# encoding: ascii\n# frozen_string_literal: true\nputs 1\n",
        );
        expect_correction(
            "Lint/OrderedMagicComments",
            "# frozen_string_literal: true\n# encoding: ascii\nputs 1\n",
            "# encoding: ascii\n# frozen_string_literal: true\nputs 1\n",
        );
    }

    /// `then` を書いた `if` の `else` は、本体が 2 文以上のときだけ odd になる。
    #[test]
    fn else_layout_ignores_a_single_statement_after_then() {
        expect_no_offenses("Lint/ElseLayout", "if x\nthen y\nelse foo(1,\n  2)\nend\n");
        expect_correction(
            "Lint/ElseLayout",
            "if x\n  y\nelse z\n  w\nend\n",
            "if x\n  y\nelse\n  z\n  w\nend\n",
        );
    }

    /// `"..."%[...]` は書式演算子で、隣接した文字列リテラルではない。
    #[test]
    fn implicit_string_concatenation_ignores_the_format_operator() {
        expect_no_offenses(
            "Lint/ImplicitStringConcatenation",
            "puts \"%3d %s\"%[1, 2]\n",
        );
        expect_correction(
            "Lint/ImplicitStringConcatenation",
            "y = \"g\" \"h\"\n",
            "y = \"g\" + \"h\"\n",
        );
    }

    /// クラスを組み立てるブロックや `instance_eval` は自前のスコープを開く。
    #[test]
    fn nested_method_definition_allows_scoping_blocks() {
        expect_no_offenses(
            "Lint/NestedMethodDefinition",
            "def foo\n  Class.new do\n    def bar; end\n  end\n  instance_eval do\n    def baz; end\n  end\nend\n",
        );
    }

    /// 2.6 以降は構文エラーになるので、cop 自体が組み立てられない。
    #[test]
    fn useless_else_without_rescue_is_off_for_newer_rubies() {
        expect_no_offenses(
            "Lint/UselessElseWithoutRescue",
            "begin\n  do_something\nelse\n  handle\nend\n",
        );
    }

    /// `OpenSSL::Digest` を直に呼ぶ形は対象外。アルゴリズム名は 3 文字ずつ区切られる。
    #[test]
    fn deprecated_open_ssl_constant_needs_an_algorithm_constant() {
        expect_no_offenses(
            "Lint/DeprecatedOpenSSLConstant",
            "OpenSSL::Digest.new('SHA256')\nOpenSSL::Cipher::AES.new(key)\n",
        );
        expect_correction(
            "Lint/DeprecatedOpenSSLConstant",
            "OpenSSL::Cipher::AES128.new(:GCM)\nOpenSSL::Digest::SHA256.digest('foo')\nOpenSSL::Cipher::BF.new\n",
            "OpenSSL::Cipher.new('aes-128-gcm')\nOpenSSL::Digest.digest('SHA256', 'foo')\nOpenSSL::Cipher.new('bf')\n",
        );
    }

    /// 引数を取らないエントリポイントが値の位置に立っているときは名前とみなす。
    #[test]
    fn debugger_ignores_an_entry_point_used_as_a_value() {
        expect_no_offenses("Lint/Debugger", "p byebug\nsomething_else\n");
        expect_offense(
            "Lint/Debugger",
            "Kernel.binding.irb\n^^^^^^^^^^^^^^^^^^ Remove debugger entry point `Kernel.binding.irb`.\n",
        );
    }

    /// レシーバがまた安全呼び出しなら当たらない。
    #[test]
    fn safe_navigation_with_empty_needs_a_plain_receiver() {
        expect_no_offenses(
            "Lint/SafeNavigationWithEmpty",
            "if qux&.quux&.empty?\n  p 2\nend\nfoo&.empty?\n",
        );
        expect_correction(
            "Lint/SafeNavigationWithEmpty",
            "p 1 if baz&.empty?\n",
            "p 1 if baz && baz.empty?\n",
        );
    }

    /// `then` と `end` が同じ行なら意図された空、コメントがあれば説明とみなす。
    #[test]
    fn empty_conditional_body_allows_comments_and_one_line_forms() {
        expect_no_offenses(
            "Lint/EmptyConditionalBody",
            "unless true; end\nif a\n  # explanation\nend\n",
        );
        expect_correction(
            "Lint/EmptyConditionalBody",
            "if h\nelse\n  i\nend\n",
            "unless h\n  i\nend\n",
        );
    }

    /// 2 回以上回るなら無駄ではない。符号はリテラルへ畳み込まれる。
    #[test]
    fn useless_times_counts_the_folded_sign() {
        expect_no_offenses("Lint/UselessTimes", "2.times { |i| p i }\n");
        expect_correction(
            "Lint/UselessTimes",
            "-2.times { |i| p i }\n1.times { |i| p i }\n",
            "p 0\n",
        );
    }

    /// 文字クラスの中と `(?#...)` の中の丸括弧は捕獲ではない。
    #[test]
    fn mixed_regexp_capture_types_reads_the_pattern() {
        expect_no_offenses(
            "Lint/MixedRegexpCaptureTypes",
            "a = /(?<n>x)(?<m>y)/\nb = /(x)(y)/\nc = /#{x}(?<n>a)(b)/\n",
        );
        expect_offense(
            "Lint/MixedRegexpCaptureTypes",
            "d = /[()](b)(?<c>d)/\n    ^^^^^^^^^^^^^^^^ Do not mix named captures and numbered captures in a Regexp literal.\n",
        );
    }

    /// 範囲リテラルが条件の位置にあるときだけ flip-flop になる。
    #[test]
    fn flip_flop_needs_a_condition_position() {
        expect_no_offenses("Lint/FlipFlop", "x = 1..2\nz = [1..2]\n");
        expect_offense(
            "Lint/FlipFlop",
            "while (a..b)\n       ^^^^ Avoid the use of flip-flop operators.\n  p 2\nend\n",
        );
    }

    /// ブロックの中は呼び出しを抜けたあとなので、その正規表現の捕獲数が効く。
    #[test]
    fn out_of_range_regexp_ref_sees_the_call_before_its_block() {
        expect_no_offenses(
            "Lint/OutOfRangeRegexpRef",
            "\"foo\".sub(/(a)(b)/) { $2 + $1 }\n",
        );
        expect_offense(
            "Lint/OutOfRangeRegexpRef",
            "case x\nwhen /(a)/\n  puts $2\n       ^^ $2 is out of range (1 regexp capture group detected).\nend\n",
        );
    }
}

mod metrics {
    use super::*;

    #[test]
    fn block_length() {
        CopCase::annotated(
            "Metrics/BlockLength",
            r#"
            [1].each do |i|
            ^^^^^^^^^^^^^^^ Block has too many lines. [2/1]
              puts 1
              puts 2
            end
            "#,
        )
        .config("Metrics/BlockLength:\n  Max: 1\n")
        .run();
        CopCase::new(
            "Metrics/BlockLength",
            "[1].each { |i| puts i }\n",
            Vec::new(),
        )
        .config("Metrics/BlockLength:\n  Max: 1\n")
        .run();
    }

    /// `Class.new`/`Module.new`/`Struct.new`/`Data.define` の中身はクラス定義そのものなので、
    /// 本家は Metrics/ClassLength に任せてここでは数えない。名前空間付きの定数は別物なので
    /// 免除されない。
    #[test]
    fn block_length_skips_class_constructors() {
        for constructor in ["Class.new", "Module.new", "Struct.new", "Data.define"] {
            CopCase::new(
                "Metrics/BlockLength",
                format!("{constructor} do\n  puts 1\n  puts 2\nend\n"),
                Vec::new(),
            )
            .config("Metrics/BlockLength:\n  Max: 1\n")
            .run();
        }
        CopCase::annotated(
            "Metrics/BlockLength",
            r#"
            Foo::Struct.new do
            ^^^^^^^^^^^^^^^^^^ Block has too many lines. [2/1]
              puts 1
              puts 2
            end
            "#,
        )
        .config("Metrics/BlockLength:\n  Max: 1\n")
        .locations(&[(1, 1, 4, 3)])
        .run();
    }

    // キャレットは先頭行しか表せないので、class 全体に跨るレンジの終端は
    // `locations` で固定する。
    #[test]
    fn class_length() {
        CopCase::annotated(
            "Metrics/ClassLength",
            r#"
            class Foo
            ^^^^^^^^^ Class has too many lines. [2/1]
              puts 1
              puts 2
            end
            "#,
        )
        .config("Metrics/ClassLength:\n  Max: 1\n")
        .locations(&[(1, 1, 4, 3)])
        .run();
        CopCase::new("Metrics/ClassLength", "class Foo\nend\n", Vec::new())
            .config("Metrics/ClassLength:\n  Max: 1\n")
            .run();
    }

    #[test]
    fn method_length() {
        CopCase::annotated(
            "Metrics/MethodLength",
            r#"
            def foo
            ^^^^^^^ Method has too many lines. [2/1]
              puts 1
              puts 2
            end
            "#,
        )
        .config("Metrics/MethodLength:\n  Max: 1\n")
        .run();
        CopCase::new("Metrics/MethodLength", "def foo\n  1\nend\n", Vec::new())
            .config("Metrics/MethodLength:\n  Max: 1\n")
            .run();
    }

    /// `define_method` のブロックも本家はメソッドとして数え、`define_method` の呼び出しから
    /// 報告する。
    #[test]
    fn method_length_measures_define_method_blocks() {
        CopCase::annotated(
            "Metrics/MethodLength",
            r#"
            define_method(:foo) do
            ^^^^^^^^^^^^^^^^^^^^^^ Method has too many lines. [2/1]
              puts 1
              puts 2
            end
            "#,
        )
        .config("Metrics/MethodLength:\n  Max: 1\n")
        .locations(&[(1, 1, 4, 3)])
        .run();
    }

    /// 本家はメソッド本体の *ノードのソース* を数える。ヒアドキュメントのノードは開始札しか
    /// 持たないので、本体がヒアドキュメント 1 個だけなら中身が何行あっても 1 行。
    #[test]
    fn method_length_counts_a_bare_heredoc_body_as_one_line() {
        CopCase::new(
            "Metrics/MethodLength",
            "def configs\n  <<-YAML\n    a: 1\n    b: 2\n    c: 3\n  YAML\nend\n",
            Vec::new(),
        )
        .config("Metrics/MethodLength:\n  Max: 2\n")
        .run();
    }

    /// 本体にヒアドキュメントが混じるときだけ、本家は行範囲を「本体の子孫が触れた最後の行」
    /// まで (ヒアドキュメントは終了札まで) に切り替える。閉じる `end` の行は本体ノード自身の
    /// ものなので範囲から落ちる。ここでは 2..6 行目の 5 行が数えられ、7 行目の `end` は入らない。
    #[test]
    fn method_length_stops_at_the_last_descendant_when_a_heredoc_is_present() {
        CopCase::new(
            "Metrics/MethodLength",
            "def report\n  if flag\n    text = <<~MSG\n      hello\n    MSG\n    puts text\n  end\nend\n",
            Vec::new(),
        )
        .config("Metrics/MethodLength:\n  Max: 5\n")
        .run();
        CopCase::annotated(
            "Metrics/MethodLength",
            r#"
            def report
            ^^^^^^^^^^ Method has too many lines. [5/4]
              if flag
                text = <<~MSG
                  hello
                MSG
                puts text
              end
            end
            "#,
        )
        .config("Metrics/MethodLength:\n  Max: 4\n")
        .locations(&[(1, 1, 8, 3)])
        .run();
    }

    #[test]
    fn module_length() {
        CopCase::annotated(
            "Metrics/ModuleLength",
            r#"
            module Foo
            ^^^^^^^^^^ Module has too many lines. [2/1]
              puts 1
              puts 2
            end
            "#,
        )
        .config("Metrics/ModuleLength:\n  Max: 1\n")
        .run();
        CopCase::new("Metrics/ModuleLength", "module Foo\nend\n", Vec::new())
            .config("Metrics/ModuleLength:\n  Max: 1\n")
            .run();
    }

    #[test]
    fn parameter_lists() {
        expect_offense(
            "Metrics/ParameterLists",
            r#"
            def foo(a, b, c, d, e, f)
                   ^^^^^^^^^^^^^^^^^^ Avoid parameter lists longer than 5 parameters. [6/5]
            end
            "#,
        );
        expect_no_offenses("Metrics/ParameterLists", "def foo(a, b, c, d, e)\nend\n");
    }

    /// 明示的なブロック引数は数えない。暗黙化すれば済む話なので、数えると本家が望まない
    /// 変更へ誘導してしまう。`**options` は対象外ではないので数える。
    #[test]
    fn parameter_lists_excludes_the_explicit_block_argument() {
        expect_no_offenses(
            "Metrics/ParameterLists",
            "def foo(a, b, c, d, e, &block)\nend\n",
        );
        expect_offense(
            "Metrics/ParameterLists",
            r#"
            def foo(a, b, c, d, e, **options, &block)
                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Avoid parameter lists longer than 5 parameters. [6/5]
            end
            "#,
        );
    }

    /// `MaxOptionalParameters` (既定 3) は Max とは別の検査で、`def` ノード全体を指す。
    /// キーワード引数の既定値は省略可能引数には数えない。
    #[test]
    fn parameter_lists_reports_too_many_optional_parameters() {
        CopCase::annotated(
            "Metrics/ParameterLists",
            r#"
            def foo(a = 1, b = 2, c = 3, d = 4)
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Method has too many optional parameters. [4/3]
            end
            "#,
        )
        .locations(&[(1, 1, 2, 3)])
        .run();
        expect_no_offenses(
            "Metrics/ParameterLists",
            "def foo(a: 1, b: 2, c: 3, d: 4)\nend\n",
        );
    }

    /// tree-sitter-ruby は既定値が `nil`/`true`/`false` の並びを 1 個の `optional_parameter`
    /// (多重代入の連鎖) に畳んでしまう。畳まれた分を数え直さないと丸ごと見落とす。
    #[test]
    fn parameter_lists_unfolds_keyword_literal_defaults() {
        CopCase::annotated(
            "Metrics/ParameterLists",
            r#"
            def tag(name = nil, options = nil, open = false, escape = true)
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Method has too many optional parameters. [4/3]
            end
            "#,
        )
        .locations(&[(1, 1, 2, 3)])
        .run();
    }

    /// ブロックの引数リストも `on_args` に届くので数えられる。ただし lambda / proc は
    /// アリティが呼び出し側の都合なので免除。
    #[test]
    fn parameter_lists_covers_blocks_but_not_lambdas_or_procs() {
        expect_offense(
            "Metrics/ParameterLists",
            r#"
            each do |a, b, c, d, e, f|
                    ^^^^^^^^^^^^^^^^^^ Avoid parameter lists longer than 5 parameters. [6/5]
            end
            "#,
        );
        for source in [
            "lambda { |a, b, c, d, e, f| 1 }\n",
            "->(a, b, c, d, e, f) { 1 }\n",
            "proc { |a, b, c, d, e, f| 1 }\n",
            "Proc.new { |a, b, c, d, e, f| 1 }\n",
        ] {
            expect_no_offenses("Metrics/ParameterLists", source);
        }
    }

    /// `Struct.new`/`Data.define` の `initialize` はメンバ一覧の写しなので数えない。本家は
    /// ブロックの *直接の* 子である `def` だけを免除するため、文が 2 つ以上あると外れる。
    #[test]
    fn parameter_lists_exempts_struct_and_data_initialize() {
        expect_no_offenses(
            "Metrics/ParameterLists",
            "Struct.new(:a) do\n  def initialize(a:, b:, c:, d:, e:, f:)\n  end\nend\n",
        );
        expect_no_offenses(
            "Metrics/ParameterLists",
            "Data.define(:a) do\n  def initialize(a:, b:, c:, d:, e:, f:)\n  end\nend\n",
        );
        expect_offense(
            "Metrics/ParameterLists",
            r#"
            Struct.new(:a) do
              def initialize(a:, b:, c:, d:, e:, f:)
                            ^^^^^^^^^^^^^^^^^^^^^^^^ Avoid parameter lists longer than 5 parameters. [6/5]
              end
              def other(a:, b:, c:, d:, e:, f:)
                       ^^^^^^^^^^^^^^^^^^^^^^^^ Avoid parameter lists longer than 5 parameters. [6/5]
              end
            end
            "#,
        );
    }
}

mod naming {
    use super::*;

    // 陽性ケースは sonicop と本家で検出対象・レンジ・文言のすべてが食い違うため、
    // 本家の実出力を期待値にした `conformance::ascii_identifiers_matches_rubocop`
    // 側に置いている。ここでは乖離していない陰性ケースだけを見る。
    #[test]
    fn ascii_identifiers() {
        expect_no_offenses("Naming/AsciiIdentifiers", "@foo = 1\n");
        expect_no_offenses("Naming/AsciiIdentifiers", "foo = 1\nCONST = 2\n");
    }

    #[test]
    fn constant_name() {
        expect_offense(
            "Naming/ConstantName",
            r#"
            Foo = 1
            ^^^ Use SCREAMING_SNAKE_CASE for constants.
            "#,
        );
        expect_no_offenses("Naming/ConstantName", "FOO = 1\n");
    }

    /// 本家 `allowed_assignment?` が見逃すのは「クラスかもしれない値」だけで、
    /// リテラルを受け手にした呼び出しはそこに入らない。既定を「報告しない」側に
    /// 倒して `{}.freeze` を許していたのが、Rails コーパスで 85 件の取りこぼしだった。
    #[test]
    fn constant_name_reports_a_call_on_a_literal_receiver() {
        expect_offense(
            "Naming/ConstantName",
            r#"
            Foo = {}.freeze
            ^^^ Use SCREAMING_SNAKE_CASE for constants.
            "#,
        );
        // 受け手がリテラルでなければ、返る値がクラスかどうかは分からない。
        expect_no_offenses("Naming/ConstantName", "Foo = [1].freeze.dup\n");
        expect_no_offenses("Naming/ConstantName", "Foo = Class.new\n");
        expect_no_offenses("Naming/ConstantName", "Foo = Struct.new(:a)\n");
        expect_no_offenses("Naming/ConstantName", "Foo = something\n");
        expect_no_offenses("Naming/ConstantName", "Foo = Other\n");
    }

    /// 裸の識別子は、その名前が先に束縛されていればローカル変数の読み出しで、
    /// そうでなければレシーバ無しのメソッド呼び出しになる。本家はこの型の違いで
    /// 判定を変えるので、スコープを追わないと `Routes = routes` を落とす。
    #[test]
    fn constant_name_separates_a_local_read_from_a_method_call() {
        expect_offense(
            "Naming/ConstantName",
            r#"
            stub do |routes|
              Routes = routes
              ^^^^^^ Use SCREAMING_SNAKE_CASE for constants.
              Paths = paths
            end
            "#,
        );
    }

    /// 右辺が読めるのは or_asgn 越しの `||=` だけ。ほかの演算子代入では casgn に
    /// 式が残らず値は不明になるが、不明は「許す」ではないので綴りが問われる。
    #[test]
    fn constant_name_reads_the_value_only_through_or_assign() {
        expect_offense(
            "Naming/ConstantName",
            r#"
            Foo ||= Other
            Bar ||= 1
            ^^^ Use SCREAMING_SNAKE_CASE for constants.
            Baz += Other
            ^^^ Use SCREAMING_SNAKE_CASE for constants.
            "#,
        );
    }

    /// 多重代入の各ターゲットも式を持たない casgn なので、右辺では免れない。
    #[test]
    fn constant_name_reports_every_multiple_assignment_target() {
        expect_offense(
            "Naming/ConstantName",
            r#"
            Qux, Quux = Other, Another
            ^^^ Use SCREAMING_SNAKE_CASE for constants.
                 ^^^^ Use SCREAMING_SNAKE_CASE for constants.
            "#,
        );
    }

    #[test]
    fn method_name() {
        expect_offense(
            "Naming/MethodName",
            r#"
            def fooBar
                ^^^^^^ Use snake_case for method names.
            end
            "#,
        );
        expect_no_offenses("Naming/MethodName", "def foo_bar\nend\n");
    }

    /// 本家 `operator_method?` が見る一覧には `=~` も単項の `-@` `~@` `!` も入って
    /// いるので、演算子の定義はどの綴り規約でも報告されない。
    #[test]
    fn method_name_leaves_operator_definitions_alone() {
        for source in [
            "def =~(other)\nend\n",
            "def -@\nend\n",
            "def ~@\nend\n",
            "def !\nend\n",
        ] {
            expect_no_offenses("Naming/MethodName", source);
        }
    }

    /// `def` 以外にもメソッド名を決める書き方があり、本家はそのすべてを見る。
    /// レンジはセレクタの 1 文字後ろから始まるので、括弧の有無に関わらず
    /// 最初の引数を指す。
    #[test]
    fn method_name_checks_the_other_ways_a_method_gets_named() {
        expect_offense(
            "Naming/MethodName",
            r#"
            class C
              alias :fooBar :baz
                    ^^^^^^^ Use snake_case for method names.
              alias_method :barBaz, :baz
                           ^^^^^^^ Use snake_case for method names.
              attr_accessor :bazQux, :other
                            ^^^^^^^^^^^^^^^ Use snake_case for method names.
              define_method :quxQuux do
                            ^^^^^^^^ Use snake_case for method names.
              end
            end
            "#,
        );
        expect_offense(
            "Naming/MethodName",
            r#"
            Corge = Struct.new(:corgeGrault)
                               ^^^^^^^^^^^^ Use snake_case for method names.
            "#,
        );
    }

    /// 既定の設定が `ForbiddenIdentifiers` に `__id__` と `__send__` を積んでいるので、
    /// 綴りが snake_case でも別の文面で報告される。
    #[test]
    fn method_name_reports_the_forbidden_identifiers_of_the_default_config() {
        expect_offense(
            "Naming/MethodName",
            r#"
            def __send__
                ^^^^^^^^ `__send__` is forbidden, use another method name instead.
            end
            "#,
        );
    }

    #[test]
    fn variable_name() {
        expect_offense(
            "Naming/VariableName",
            r#"
            def foo(barBaz)
                    ^^^^^^ Use snake_case for variable names.
            end
            "#,
        );
        expect_no_offenses("Naming/VariableName", "def foo(bar_baz)\nend\n");
    }

    /// 本家は `on_lvar` も見るので、綴りの悪いローカル変数は代入だけでなく
    /// 読み出しのたびに報告される。同じ綴りでもメソッド呼び出しは対象外。
    #[test]
    fn variable_name_reports_reads_of_a_local_but_not_method_calls() {
        expect_offense(
            "Naming/VariableName",
            r#"
            fooBar = 1
            ^^^^^^ Use snake_case for variable names.
            puts fooBar
                 ^^^^^^ Use snake_case for variable names.
            puts bazQux
            "#,
        );
    }

    /// インスタンス変数とクラス変数は代入だけが対象。グローバル変数は
    /// `on_gvasgn` が ForbiddenIdentifiers しか見ないので綴りを問われない。
    #[test]
    fn variable_name_checks_instance_and_class_variables_but_not_globals() {
        expect_offense(
            "Naming/VariableName",
            r#"
            @fooBar = 1
            ^^^^^^^ Use snake_case for variable names.
            @@bazQux = 1
            ^^^^^^^^ Use snake_case for variable names.
            $quxQuux = 1
            "#,
        );
    }

    /// ブロックローカル (`shadowarg`) とパターンマッチの束縛 (`match_var`) には
    /// 本家にハンドラが無いので、束縛そのものは報告されない。それでも名前は
    /// スコープに入るため、後続の読み出しはローカル変数として報告される。
    #[test]
    fn variable_name_skips_bindings_without_a_handler_but_still_scopes_them() {
        expect_offense(
            "Naming/VariableName",
            r#"
            [1].each do |aA; shadowB|
                         ^^ Use snake_case for variable names.
              shadowB = aA
              ^^^^^^^ Use snake_case for variable names.
                        ^^ Use snake_case for variable names.
            end
            "#,
        );
        expect_offense(
            "Naming/VariableName",
            r#"
            case value
            in [pX]
              pX
              ^^ Use snake_case for variable names.
            end
            "#,
        );
    }
}

mod security {
    use super::*;

    #[test]
    fn eval() {
        expect_offense(
            "Security/Eval",
            r#"
            eval(code)
            ^^^^ The use of `eval` is a serious security risk.
            "#,
        );
        expect_no_offenses("Security/Eval", "eval('2 + 2')\n");
    }

    // 本家 `Cop::Base#default_severity` は `lint? ? :warning : :convention` で、
    // Security/Eval は config/default.yml に `Severity:` を持たないため convention。
    #[test]
    fn eval_reports_convention_severity_like_rubocop() {
        CopCase::new("Security/Eval", "eval(code)\n", Vec::new())
            .without_offense_check()
            .severity(Severity::Convention)
            .run();
    }
}

mod style {
    use super::*;

    #[test]
    fn frozen_string_literal_comment_accepts_the_magic_comment() {
        expect_no_offenses(
            "Style/FrozenStringLiteralComment",
            "# frozen_string_literal: true\n\nx = 1\n",
        );
    }

    #[test]
    fn hash_syntax() {
        expect_offense(
            "Style/HashSyntax",
            r#"
            puts({ :a => 1 })
                   ^^^^^ Use the new Ruby 1.9 hash syntax.
            "#,
        );
        expect_no_offenses("Style/HashSyntax", "puts({ a: 1 })\n");
        expect_correction(
            "Style/HashSyntax",
            "puts({ :a => 1 })\n",
            "puts({ a: 1 })\n",
        );
    }

    #[test]
    fn numeric_literals() {
        expect_offense(
            "Style/NumericLiterals",
            r#"
            puts 12345
                 ^^^^^ Use underscores(_) as thousands separator and separate every 3 digits with them.
            "#,
        );
        expect_no_offenses("Style/NumericLiterals", "puts 1234\n");
        expect_correction("Style/NumericLiterals", "puts 12345\n", "puts 12_345\n");
    }

    /// 桁区切りの検査は整数リテラルだけの話ではない。小数の整数部も対象で、
    /// 既に `_` の入った数も区切り方が狂っていれば咎める。逆に末尾だけ短い
    /// `10_000_00` はセント表記として許され、`r` / `i` 付きは別のリテラル
    /// (rational / complex) なので対象外。符号は数の一部として範囲に入る。
    ///
    /// 期待値は本家 1.89.0 の `--only Style/NumericLiterals --format json` と
    /// `-a` の実測。
    #[test]
    fn numeric_literals_checks_floats_regrouping_and_signed_literals() {
        const MSG: &str =
            "Use underscores(_) as thousands separator and separate every 3 digits with them.";
        let source = concat!(
            "a = 1234567890.50\n",
            "b = 2018_02_12_164506\n",
            "c = 18_00_00\n",
            "d = 10_000_00\n",
            "e = -9223372036854775808\n",
            "f = 1_000_000\n",
            "g = 1000000r\n",
            "h = 1000000i\n",
            "i = 0xFFFFF\n",
            "j = 1_0000\n",
        );
        CopCase::new(
            "Style/NumericLiterals",
            source,
            vec![
                Annotation::new(1, 5, 13, MSG),
                Annotation::new(2, 5, 17, MSG),
                Annotation::new(3, 5, 8, MSG),
                Annotation::new(5, 5, 20, MSG),
                Annotation::new(10, 5, 6, MSG),
            ],
        )
        .corrected(concat!(
            "a = 1_234_567_890.50\n",
            "b = 20_180_212_164_506\n",
            "c = 180_000\n",
            "d = 10_000_00\n",
            "e = -9_223_372_036_854_775_808\n",
            "f = 1_000_000\n",
            "g = 1000000r\n",
            "h = 1000000i\n",
            "i = 0xFFFFF\n",
            "j = 10_000\n",
        ))
        .run();
    }

    #[test]
    fn semicolon_accepts_separate_lines() {
        expect_no_offenses("Style/Semicolon", "puts 1\nputs 2\n");
    }

    /// 1 行メソッドの `;` は本家では offense にならない。本家はトークン列を見て
    /// 「行頭・行末・波括弧に接する」セミコロンだけを 1 行 1 件報告し、`def foo; bar; end` は
    /// 式が 1 つなので何も出さない。Rails では 641 件を誤検出していた形。
    #[test]
    fn semicolon_accepts_a_single_line_method_definition() {
        expect_no_offenses("Style/Semicolon", "def user; \"David\"; end\n");
        expect_no_offenses("Style/Semicolon", "class << self; attr_accessor :x; end\n");
    }

    /// 逆に、式が 2 つ以上終わる行では行内の `;` を残らず報告する。
    #[test]
    fn semicolon_reports_every_separator_when_a_line_holds_two_expressions() {
        expect_offense(
            "Style/Semicolon",
            r#"
            def clear; @a = 1; @b = 2; end
                     ^ Do not use semicolons to terminate expressions.
                             ^ Do not use semicolons to terminate expressions.
                                     ^ Do not use semicolons to terminate expressions.
            "#,
        );
    }

    /// 行末のセミコロンは式が 1 つでも報告する。
    #[test]
    fn semicolon_reports_a_line_terminator() {
        expect_offense(
            "Style/Semicolon",
            r#"
            x = 1;
                 ^ Do not use semicolons to terminate expressions.
            "#,
        );
    }

    #[test]
    fn string_literals() {
        expect_offense(
            "Style/StringLiterals",
            r#"
            puts "hi"
                 ^^^^ Prefer single-quoted strings when you don't need string interpolation or special symbols.
            "#,
        );
        expect_no_offenses("Style/StringLiterals", "puts 'hi'\n");
        expect_correction("Style/StringLiterals", "puts \"hi\"\n", "puts 'hi'\n");
    }

    // 本家の判定は「値」ではなく「ソース」に対して行われる。`\"` は単引用符では
    // ただの `"` になるので、エスケープがあっても二重引用符は要らない。
    //
    // 実測 (rubocop 1.89.0): 3 行とも offense。
    #[test]
    fn string_literals_reports_escapes_that_single_quotes_can_drop() {
        expect_offense(
            "Style/StringLiterals",
            r##"
            a = "{\"k\":\"v\"}"
                ^^^^^^^^^^^^^^^ Prefer single-quoted strings [...]
            b = "\\x34"
                ^^^^^^^ Prefer single-quoted strings [...]
            c = "\\\"x"
                ^^^^^^^ Prefer single-quoted strings [...]
            "##,
        );
        expect_correction(
            "Style/StringLiterals",
            "a = \"{\\\"k\\\":\\\"v\\\"}\"\nb = \"\\\\x34\"\nc = \"\\\\\\\"x\"\n",
            "a = '{\"k\":\"v\"}'\nb = '\\\\x34'\nc = '\\\"x'\n",
        );
    }

    // 逆に、バックスラッシュの連なりが奇数個で終わる (= 何かをエスケープしている)
    // 場合は単引用符では書けないので offense にならない。`'` を含む場合も同じ。
    #[test]
    fn string_literals_accepts_escapes_that_need_double_quotes() {
        expect_no_offenses(
            "Style/StringLiterals",
            r##"
            a = "a\nb"
            b = "\e[0m"
            c = "it's"
            d = "\\\y"
            "##,
        );
    }

    // `#$0` / `#@ivar` は `#{}` と同じ補間で、本家では dstr になり `on_str` が
    // 呼ばれない。字面に `#{` が無いため素通ししていた false positive の回帰。
    #[test]
    fn string_literals_ignores_shorthand_interpolation() {
        expect_no_offenses(
            "Style/StringLiterals",
            r##"
            a = "x_#$0"
            b = "y_#@ivar"
            c = "z_#{w}"
            "##,
        );
    }

    // 値が複数行にまたがるリテラルは本家では dstr になり、行ごとの str 子ノードは
    // 引用符を持たないので検査対象から外れる。
    #[test]
    fn string_literals_ignores_a_value_that_spans_lines() {
        expect_no_offenses("Style/StringLiterals", "a = \"multi\nline\"\n");
    }

    // 補間の中の文字列は Style/StringLiteralsInInterpolation の担当だが、
    // それが効くのは dstr / dsym / regexp の中だけ。バッククォート (xstr) の
    // 補間はこの cop が見る。
    #[test]
    fn string_literals_checks_interpolation_inside_a_command_literal() {
        expect_offense(
            "Style/StringLiterals",
            r#"
            a = `ls #{"foo"}`
                      ^^^^^ Prefer single-quoted strings [...]
            "#,
        );
        expect_no_offenses(
            "Style/StringLiterals",
            r##"
            a = "o#{"i"}"
            b = /r#{"i"}/
            c = :"s#{"i"}"
            "##,
        );
    }

    // `%` リテラルと文字リテラルは引用符を差し替えようがないので本家も見ない。
    // 引用符付きハッシュキーは symbol なので同様。
    #[test]
    fn string_literals_ignores_literals_without_swappable_quotes() {
        expect_no_offenses(
            "Style/StringLiterals",
            r##"
            a = %q(x)
            b = %(y)
            c = %w[d e]
            d = ?f
            e = { "k": 1 }
            "##,
        );
    }

    // 値に生の制御文字が入っていると単引用符では書けないので、本家は
    // `String#inspect` に切り替えて二重引用符のままエスケープする。
    #[test]
    fn string_literals_corrects_a_raw_control_character_by_escaping_it() {
        expect_correction(
            "Style/StringLiterals",
            "a = \"tab\there\"\n",
            "a = \"tab\\there\"\n",
        );
    }

    // double_quotes 側の判定は `"` / `\` の後続 1 文字 / `#{`・`#@`・`#$` の
    // 走査で、単引用符側のような連なりの数え上げはしない。
    #[test]
    fn string_literals_double_quotes_style_keeps_meaningful_single_quotes() {
        CopCase::annotated(
            "Style/StringLiterals",
            r#"
            a = 'plain'
                ^^^^^^^ Prefer double-quoted strings [...]
            "#,
        )
        .config("Style/StringLiterals:\n  EnforcedStyle: double_quotes\n")
        .run();
        CopCase::new(
            "Style/StringLiterals",
            "a = 'a\\nb'\nb = 'say \"hi\"'\nc = '#{x}'\nd = '#@y'\n",
            Vec::new(),
        )
        .config("Style/StringLiterals:\n  EnforcedStyle: double_quotes\n")
        .run();
        CopCase::new("Style/StringLiterals", "a = 'tab\there'\n", Vec::new())
            .config("Style/StringLiterals:\n  EnforcedStyle: double_quotes\n")
            .without_offense_check()
            .corrected("a = \"tab\\there\"\n")
            .run();
    }
}

/// 他チームが並行して直している確定バグの回帰。
mod regressions {
    use super::*;

    // 「`return` で終わらないメソッドが先に来ると後続のメソッドを走査しなくなる」
    // 取り違えの回帰。ファイル内の後続メソッドが全スキップされる false negative。
    #[test]
    fn redundant_return_detects_a_return_after_a_method_without_one() {
        expect_offense(
            "Style/RedundantReturn",
            r#"
            def first;  1;        end
            def second; return 2; end
                        ^^^^^^ Redundant `return` detected.
            "#,
        );
    }

    #[test]
    fn redundant_return_detects_a_return_in_the_first_method() {
        expect_offense(
            "Style/RedundantReturn",
            r#"
            def second; return 2; end
                        ^^^^^^ Redundant `return` detected.
            def first;  1;        end
            "#,
        );
    }

    #[test]
    fn redundant_return_ignores_a_method_body_without_return() {
        expect_no_offenses("Style/RedundantReturn", "def foo\n  1\nend\n");
    }

    #[test]
    fn redundant_return_corrects_by_dropping_the_keyword() {
        expect_correction(
            "Style/RedundantReturn",
            "def foo\n  return 1\nend\n",
            "def foo\n  1\nend\n",
        );
    }

    // `Offense::location` が `end - 1` をそのままバイト添字にするため、レンジが
    // 多バイト文字の直後で終わると char boundary を割ってプロセスごと落ちる。
    // JSON formatter も同じ経路を通るので CLI でも再現する。
    //
    // 本家が実際に検出する入力 (ローカル変数) を使う。位置 `1:1-1:1` は本家と
    // 一致しているのでここで固定する。`locations` の検証が `location()` を通るため、
    // 文字境界を割る実装に戻れば必ず落ちる。乖離しているメッセージと
    // `location.length` の単位は下の conformance 側で扱う。
    //
    // 実測 (rubocop 1.89.0): `あ = 1` → line 1 col 1 len 1
    #[test]
    fn offense_location_survives_a_range_ending_on_a_multibyte_character() {
        CopCase::new("Naming/AsciiIdentifiers", "あ = 1\n", Vec::new())
            .without_offense_check()
            .locations(&[(1, 1, 1, 1)])
            .run();
    }
}

/// 長さ系 cop が「何行と数えるか」の回帰。`CodeLengthCalculator` と
/// `CodeLength#check_code_length` の分岐ごとに 1 ケース置く。期待値はすべて
/// rubocop 1.89.0 の `--only <cop> -f json` 実測。
mod metrics_length_counting {
    use super::*;

    /// `CLASSLIKE_TYPES` は `class` と `module` だけなので、`class << self` は
    /// クラスの行範囲ではなくメソッドと同じく body で数えられる。行範囲で数えると
    /// 閉じ `end` の分だけ 1 行多い 4 になる。
    ///
    /// 実測: line 1 col 1 last 6,3 len 53 / `Class has too many lines. [3/1]`
    #[test]
    fn singleton_class_is_measured_over_its_body() {
        CopCase::annotated(
            "Metrics/ClassLength",
            r#"
            class << self
            ^^^^^^^^^^^^^ Class has too many lines. [3/1]
              # a comment
              def one
                1
              end
            end
            "#,
        )
        .config("Metrics/ClassLength:\n  Max: 1\n")
        .locations(&[(1, 1, 6, 3)])
        .lengths(&[53])
        .run();
    }

    /// 行末コメントは RuboCop の AST に存在しないので、body の最終行を
    /// コメントの行まで伸ばしてはいけない。ヒアドキュメントを含む body は
    /// 行範囲で数えられるため、伸びた 1 行がそのまま件数に出る。
    ///
    /// 実測: 1:1 `[5/1]` / 2:3 `[4/1]`
    #[test]
    fn a_trailing_comment_does_not_extend_a_body_holding_a_heredoc() {
        CopCase::annotated(
            "Metrics/BlockLength",
            r#"
            outer do
            ^^^^^^^^ Block has too many lines. [5/1]
              inner do
              ^^^^^^^^ Block has too many lines. [4/1]
                write <<~RUBY
                  a
                  b
                RUBY
              end # inner
            end
            "#,
        )
        .config("Metrics/BlockLength:\n  Max: 1\n")
        .locations(&[(1, 1, 8, 3), (2, 3, 7, 5)])
        .run();
    }

    /// `CountComments: true` でも同じ。body は `bar` の 1 行きりで、後続の
    /// コメント行は body の外にある。
    ///
    /// 実測: offense なし
    #[test]
    fn a_trailing_comment_is_outside_the_body_even_when_comments_count() {
        CopCase::new(
            "Metrics/MethodLength",
            "def foo\n  bar\n  # trailing\nend\n",
            Vec::new(),
        )
        .config("Metrics/MethodLength:\n  Max: 1\n  CountComments: true\n")
        .run();
    }

    /// 中身が class か module 1 つきりの名前空間は、`end` がどれだけ離れていても
    /// 0 行と数えられる。`Max: 0` でないと差が出ない。
    ///
    /// 実測: 3:3 `[3/0]` のみ (外側の `Outer` には出ない)
    #[test]
    fn a_namespace_module_counts_as_zero_lines() {
        CopCase::annotated(
            "Metrics/ModuleLength",
            r#"
            module Outer
              # a comment
              module Inner
              ^^^^^^^^^^^^ Module has too many lines. [3/0]
                def a
                  1
                end
              end
            end
            "#,
        )
        .config("Metrics/ModuleLength:\n  Max: 0\n")
        .locations(&[(3, 3, 7, 5)])
        .run();
    }

    /// 1 行に収まる構文は、body がヒアドキュメントで下へ伸びていても
    /// `node.line_count <= max_length` で計算前に打ち切られる。`Max: 0` にすると
    /// 打ち切りが外れて 4 行として報告される。
    ///
    /// 実測: Max 1 → offense なし / Max 0 → 1:1 last 1,17 len 17 `[4/0]`
    #[test]
    fn a_single_line_construct_is_skipped_before_counting() {
        let source = "foo { bar(<<~X) }\n  a\n  b\nX\n";
        CopCase::new("Metrics/BlockLength", source, Vec::new())
            .config("Metrics/BlockLength:\n  Max: 1\n")
            .run();
        CopCase::new("Metrics/BlockLength", source, Vec::new())
            .config("Metrics/BlockLength:\n  Max: 0\n")
            .without_offense_check()
            .locations(&[(1, 1, 1, 17)])
            .lengths(&[17])
            .run();
    }
}

/// 定数へ代入されたクラス/モジュール定義を長さ系 cop がどう拾うかの回帰。
/// `Metrics/BlockLength` は `class_constructor?` で降りるので、ここで拾えないと
/// どの cop からも見えなくなる。期待値は rubocop 1.89.0 の実測。
mod metrics_constant_definitions {
    use super::*;

    /// `on_casgn` は block を `check_code_length` に渡すので、offense は定数ではなく
    /// `Class.new` から始まる。
    ///
    /// 実測: line 1 col 7 last 5,3 / `Class has too many lines. [3/1]`
    #[test]
    fn a_constant_assigned_a_class_new_block_is_a_class() {
        CopCase::annotated(
            "Metrics/ClassLength",
            r#"
            Foo = Class.new do
                  ^^^^^^^^^^^^ Class has too many lines. [3/1]
              def a
                1
              end
            end
            "#,
        )
        .config("Metrics/ClassLength:\n  Max: 1\n")
        .locations(&[(1, 7, 5, 3)])
        .run();
    }

    /// `class_definition?` は定数に条件を付けないので名前空間付きでも拾い、
    /// `Struct.new` の引数も許す。
    ///
    /// 実測: line 1 col 8 last 5,3 / `Class has too many lines. [3/1]`
    #[test]
    fn a_namespaced_constant_assigned_a_struct_new_block_is_a_class() {
        CopCase::annotated(
            "Metrics/ClassLength",
            r#"
            A::B = Struct.new(:x) do
                   ^^^^^^^^^^^^^^^^^ Class has too many lines. [3/1]
              def a
                1
              end
            end
            "#,
        )
        .config("Metrics/ClassLength:\n  Max: 1\n")
        .locations(&[(1, 8, 5, 3)])
        .run();
    }

    /// `Metrics/ModuleLength` は代入そのものを `check_code_length` に渡すため、
    /// offense は定数名に載る。パターンは `Metrics/ClassLength` より厳しく、
    /// 名前空間付きの定数 (`C::D`) は対象外。
    ///
    /// 実測: line 1 col 1 last 1,3 len 3 / `Module has too many lines. [3/1]` の 1 件だけ
    #[test]
    fn a_constant_assigned_a_module_new_block_is_reported_on_the_constant() {
        CopCase::annotated(
            "Metrics/ModuleLength",
            r#"
            Baz = Module.new do
            ^^^ Module has too many lines. [3/1]
              def c
                3
              end
            end

            C::D = Module.new do
              def d
                4
              end
            end
            "#,
        )
        .config("Metrics/ModuleLength:\n  Max: 1\n")
        .locations(&[(1, 1, 1, 3)])
        .lengths(&[3])
        .run();
    }

    /// 名前空間付きの定数は `#global_const?` に落ちるのでクラス定義ではなく、
    /// block として数えられる。
    ///
    /// 実測: Metrics/BlockLength line 1 col 5 / `Block has too many lines. [3/1]`
    #[test]
    fn a_namespaced_constructor_stays_a_block() {
        CopCase::annotated(
            "Metrics/BlockLength",
            r#"
            G = Foo::Class.new do
                ^^^^^^^^^^^^^^^^^ Block has too many lines. [3/1]
              def e
                5
              end
            end
            "#,
        )
        .config("Metrics/BlockLength:\n  Max: 1\n")
        .locations(&[(1, 5, 5, 3)])
        .run();
        CopCase::new(
            "Metrics/ClassLength",
            "G = Foo::Class.new do\n  def e\n    5\n  end\nend\n",
            Vec::new(),
        )
        .config("Metrics/ClassLength:\n  Max: 1\n")
        .run();
    }
}

/// 構文エラーの扱い。
///
/// 本家の `Cop::Commissioner#investigate` は `ProcessedSource#valid_syntax?` が
/// false のとき `on_other_file` しか呼ばず、それを実装している cop は
/// `Lint/Syntax` だけなので、構文エラーのファイルからは `Lint/Syntax` の
/// offense しか出ない。
///
/// 加えて本家は parser gem を `TargetRubyVersion` で動かすため、その版に無い
/// 構文はすべて構文エラーになる。tree-sitter は版に関係なく最新構文を受理する
/// ので、版ごとのゲートは手で持つしかない。
///
/// 期待値はすべて rubocop 1.89.0 の `--only Lint/Syntax --format json` 実測。
mod syntax {
    use super::*;

    const HINT: &str =
        "(Using Ruby 2.7 parser; configure using `TargetRubyVersion` parameter, under `AllCops`)";

    fn unexpected(line: usize, column: usize, length: usize, token: &str) -> Annotation {
        Annotation::new(
            line,
            column,
            length,
            format!("unexpected token {token}\n{HINT}"),
        )
    }

    fn at_2_7(source: &str, expected: Vec<Annotation>) -> CopCase {
        CopCase::new("Lint/Syntax", source, expected).target_ruby("2.7")
    }

    fn accepted(source: &str, version: &str) -> CopCase {
        CopCase::new("Lint/Syntax", source, Vec::new()).target_ruby(version)
    }

    /// 実測: `def type = :brew` → 1:10 tEQL / `def other = :y` → 3:11 tEQL
    #[test]
    fn endless_method_definition_needs_ruby_3_0() {
        at_2_7(
            "def type = :brew\nx = 1\ndef other = :y\n",
            vec![unexpected(1, 10, 1, "tEQL"), unexpected(3, 11, 1, "tEQL")],
        )
        .run();
        accepted("def type = :brew\n", "3.0").run();
    }

    /// 通常のメソッドの `=` は setter 名や省略可能引数の一部なので、endless
    /// 定義と取り違えてはいけない。
    #[test]
    fn a_setter_or_default_argument_is_not_an_endless_definition() {
        accepted("def foo=(value)\n  @foo = value\nend\n", "2.7").run();
        accepted("def foo a = 1\n  a\nend\n", "2.7").run();
    }

    /// 値を省略したラベルの**直後のトークン**が報告される。本家は 1 つ報告した
    /// あと構文ごと読み飛ばすので、同じリテラル内の 2 つ目以降は出ない。
    ///
    /// 実測: `foo(a:, b:)` → 1:7 tCOMMA (1 件) / `foo(a:)` → 1:7 tRPAREN
    #[test]
    fn hash_value_omission_needs_ruby_3_1() {
        at_2_7("foo(a:, b:)\n", vec![unexpected(1, 7, 1, "tCOMMA")]).run();
        at_2_7("foo(a:)\n", vec![unexpected(1, 7, 1, "tRPAREN")]).run();
        at_2_7("h = {a:, b: 1}\n", vec![unexpected(1, 8, 1, "tCOMMA")]).run();
        at_2_7(
            "foo(a: 1, b: {c:, d:})\n",
            vec![unexpected(1, 17, 1, "tCOMMA")],
        )
        .run();
        accepted("foo(a:, b:)\n", "3.1").run();
    }

    /// 別々の構文なら回復をまたいでそれぞれ報告される。
    ///
    /// 実測: `foo(a:)\nbar(b:)` → 1:7 tRPAREN と 2:7 tRPAREN
    #[test]
    fn each_construct_reports_its_own_omission() {
        at_2_7(
            "foo(a:)\nbar(b:)\n",
            vec![
                unexpected(1, 7, 1, "tRPAREN"),
                unexpected(2, 7, 1, "tRPAREN"),
            ],
        )
        .run();
    }

    /// 実測: `def foo(&)` → 1:10 tRPAREN / `bar(&)` → 2:8 tRPAREN
    #[test]
    fn anonymous_block_forwarding_needs_ruby_3_1() {
        at_2_7(
            "def foo(&)\n  bar(&)\nend\n",
            vec![
                unexpected(1, 10, 1, "tRPAREN"),
                unexpected(2, 8, 1, "tRPAREN"),
            ],
        )
        .run();
        accepted("def foo(&)\n  bar(&)\nend\n", "3.1").run();
    }

    /// 名前のない `*` / `**` の**受け取り**は昔から書けるので、渡す側だけが
    /// Ruby 3.2 のゲートに掛かる。実測: `bar(*)` → 2:8 tRPAREN
    #[test]
    fn anonymous_rest_forwarding_needs_ruby_3_2() {
        at_2_7(
            "def foo(*)\n  bar(*)\nend\n",
            vec![unexpected(2, 8, 1, "tRPAREN")],
        )
        .run();
        accepted("def foo(*)\n  bar(1)\nend\n", "2.7").run();
        accepted("def foo(*)\n  bar(*)\nend\n", "3.2").run();
    }

    /// 実測: `1 => x` → 1:3 tASSOC (2 文字)。`1 in Integer` は 2.7 で通る。
    #[test]
    fn rightward_assignment_needs_ruby_3_0() {
        at_2_7("1 => x\n", vec![unexpected(1, 3, 2, "tASSOC")]).run();
        accepted("1 in Integer\n", "2.7").run();
        accepted("1 => x\n", "3.0").run();
    }

    /// 壊れたトークンだけを指し、parser gem のトークン名で呼ぶ。
    /// 実測: `x = )` → 1:5 tRPAREN (1 文字) / `x = 1))` → 1:6 tRPAREN
    #[test]
    fn a_broken_token_is_named_and_pointed_at() {
        at_2_7("x = )\n", vec![unexpected(1, 5, 1, "tRPAREN")]).run();
        at_2_7("x = 1))\n", vec![unexpected(1, 6, 1, "tRPAREN")]).run();
        at_2_7("1+1=2\n", vec![unexpected(1, 4, 1, "tEQL")]).run();
    }

    /// 閉じ損ねた構文は、トークンではなく入力の終わりで報告される。本家は
    /// 長さ 0 のレンジを最後の 1 文字へ広げる (`lint/syntax.rb` の
    /// `diagnostic_location`)。実測: `f {` → 1:4-2:1 (1 文字) $end
    #[test]
    fn an_unclosed_construct_is_reported_at_the_end_of_input() {
        // 注記は行内のカラム差でしか長さを表せないので、行をまたぐレンジは
        // `locations` と `lengths` で固定する。本家の JSON も長さ 1 を出す。
        at_2_7(
            "f {\n",
            vec![Annotation::new(
                1,
                4,
                0,
                format!("unexpected token $end\n{HINT}"),
            )],
        )
        .locations(&[(1, 4, 2, 1)])
        .lengths(&[1])
        .run();
        at_2_7(
            "def a\n",
            vec![Annotation::new(
                1,
                6,
                0,
                format!("unexpected token $end\n{HINT}"),
            )],
        )
        .locations(&[(1, 6, 2, 1)])
        .lengths(&[1])
        .run();
    }

    /// 構文エラーのファイルには他の cop が一切走らない。
    #[test]
    fn no_other_cop_inspects_a_file_that_does_not_parse() {
        let cops = [
            "Lint/Syntax",
            "Style/StringLiterals",
            "Layout/TrailingWhitespace",
        ];
        let broken = CopCase::new("Lint/Syntax", "x = \"a\"  \ny = )\n", Vec::new())
            .cops(&cops)
            .target_ruby("2.7")
            .without_offense_check()
            .inspect();
        assert_eq!(
            broken
                .offenses
                .iter()
                .map(|offense| offense.cop_name)
                .collect::<Vec<_>>(),
            vec!["Lint/Syntax"],
            "構文エラーのファイルから Lint/Syntax 以外が出ている"
        );

        // 同じソースが構文的に通れば、両方の cop が普段どおり報告する。
        let sound = CopCase::new("Lint/Syntax", "x = \"a\"  \ny = 1\n", Vec::new())
            .cops(&cops)
            .target_ruby("2.7")
            .without_offense_check()
            .inspect();
        let mut names = sound
            .offenses
            .iter()
            .map(|offense| offense.cop_name)
            .collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["Layout/TrailingWhitespace", "Style/StringLiterals"]
        );
    }

    /// 版ゲートで落ちたファイルも同じ扱いになる。tree-sitter は受理するので、
    /// ゲートが効いていなければ他の cop がそのまま走ってしまう。
    #[test]
    fn version_gated_syntax_also_stops_the_other_cops() {
        let report = CopCase::new("Lint/Syntax", "def type = \"brew\"  \n", Vec::new())
            .cops(&[
                "Lint/Syntax",
                "Style/StringLiterals",
                "Layout/TrailingWhitespace",
            ])
            .target_ruby("2.7")
            .without_offense_check()
            .inspect();
        assert_eq!(
            report
                .offenses
                .iter()
                .map(|offense| offense.cop_name)
                .collect::<Vec<_>>(),
            vec!["Lint/Syntax"]
        );
    }
}

/// `Style/StringLiterals` が本家の `on_dstr` / `on_str` の切り分けに従っていることの回帰。
///
/// 本家は `str` と `dstr` を lexer の行分割で区別する。1 リテラルが複数行の
/// `str` に割れたときだけ `dstr` になり、`on_str` は引用符を持たない子ノードを
/// 読み飛ばす。`dstr` 自体は `ConsistentQuotesInMultiline` が真のときだけ
/// `on_dstr` が見る。
///
/// 期待値はすべて rubocop 1.89.0 の `--only Style/StringLiterals --format json`
/// 実測から取っている。
mod string_literals_multiline {
    use super::*;

    const SINGLE: &str = "Prefer single-quoted strings [...]";
    const DOUBLE: &str = "Prefer double-quoted strings [...]";
    const INCONSISTENT: &str = "Inconsistent quote style.";
    const CONSISTENT: &str = "Style/StringLiterals:\n  ConsistentQuotesInMultiline: true\n";
    const CONSISTENT_DOUBLE: &str = concat!(
        "Style/StringLiterals:\n",
        "  ConsistentQuotesInMultiline: true\n",
        "  EnforcedStyle: double_quotes\n",
    );

    // 改行が閉じ引用符の直前にしか無いリテラルは行が 1 本しかないので本家では
    // `str` のままで、`on_str` の検査対象に残る。改行を含むだけで一律に読み飛ばして
    // いた false negative の回帰。既定設定で出る差分なので影響が大きい。
    //
    // 実測: `a = "one\n"` → 1:5-2:1 len 6 correctable / `b` (2 行に割れる) は検出なし。
    #[test]
    fn a_line_break_that_only_ends_the_literal_leaves_it_a_str() {
        CopCase::annotated(
            "Style/StringLiterals",
            "a = \"one\n    ^^^^ Prefer single-quoted strings [...]\n\"\nb = \"two\nthree\"\n",
        )
        .locations(&[(1, 5, 2, 1)])
        .lengths(&[6])
        .correctable(true)
        .run();
    }

    // 本家の autocorrect は値から書き直すので、生の改行はエスケープに畳まれる。
    #[test]
    fn such_a_literal_is_corrected_into_an_escape() {
        expect_correction(
            "Style/StringLiterals",
            "a = \"one\n\"\n",
            "a = \"one\\n\"\n",
        );
    }

    // 隣接リテラルの連結は本家では `dstr`。既定 (`ConsistentQuotesInMultiline: false`)
    // では `on_dstr` が即 return するため、子リテラルが個別に検査される。
    #[test]
    fn adjacent_literals_are_checked_one_by_one_by_default() {
        CopCase::annotated(
            "Style/StringLiterals",
            r#"
            a = "x" "y"
                ^^^ Prefer single-quoted strings [...]
                    ^^^ Prefer single-quoted strings [...]
            "#,
        )
        .run();
    }

    // `ConsistentQuotesInMultiline` が真なら `on_dstr` が連結全体を 1 件として
    // 報告し、子リテラルは `part_of_ignored_node?` で落ちる。`dstr` は
    // `StringLiteralCorrector` が即 return するので correctable にならない。
    //
    // 実測: `a = "x" "y"` → 1:5-1:11 len 7 correctable=false。
    #[test]
    fn adjacent_literals_are_judged_as_one_when_consistency_is_required() {
        CopCase::new(
            "Style/StringLiterals",
            "a = \"x\" \"y\"\n",
            vec![Annotation::new(1, 5, 7, SINGLE)],
        )
        .config(CONSISTENT)
        .locations(&[(1, 5, 1, 11)])
        .lengths(&[7])
        .correctable(false)
        .run();
    }

    // 引用符が混ざる連結は専用メッセージになる。
    #[test]
    fn mixed_quotes_report_inconsistency() {
        CopCase::new(
            "Style/StringLiterals",
            "b = \"x\" 'y'\n",
            vec![Annotation::new(1, 5, 7, INCONSISTENT)],
        )
        .config(CONSISTENT)
        .locations(&[(1, 5, 1, 11)])
        .run();
        // `%q(` も 1 つの引用符として数えるので `"` と混ざれば不一致になる。
        CopCase::new(
            "Style/StringLiterals",
            "d = %q(x) \"y\"\n",
            vec![Annotation::new(1, 5, 9, INCONSISTENT)],
        )
        .config(CONSISTENT)
        .locations(&[(1, 5, 1, 13)])
        .run();
    }

    // `accept_child_double_quotes?`: 単引用符では書けない子が 1 つでもあれば
    // 連結全体が許される。
    #[test]
    fn a_child_that_needs_double_quotes_excuses_the_whole_chain() {
        CopCase::new("Style/StringLiterals", "c = \"it's\" \"y\"\n", Vec::new())
            .config(CONSISTENT)
            .run();
    }

    // Ruby は値の後ろでは `%` を剰余演算子としてしか読まないので `"x" %q(y)` は
    // `send` であって連結ではない。tree-sitter は連結として畳んでしまうため、
    // 先頭リテラルが素の `str` として検査され続けることを固定する。
    //
    // 実測: `e = "x" %q(y)` → 1:5-1:7 len 3 correctable=true (連結の報告は無い)。
    #[test]
    fn a_percent_literal_after_a_string_is_the_modulo_operator() {
        CopCase::new(
            "Style/StringLiterals",
            "e = \"x\" %q(y)\n",
            vec![Annotation::new(1, 5, 3, SINGLE)],
        )
        .config(CONSISTENT)
        .locations(&[(1, 5, 1, 7)])
        .correctable(true)
        .run();
    }

    // 値が複数行に割れる 1 リテラルは子が引用符を持たないので、本家は親の
    // 引用符を 1 つだけ読む。連結の中に入っている場合、外側は「子に `dstr` が
    // ある」ため許され、内側だけが報告される。
    //
    // 実測: `a = "x\ny"` → 1:5-2:2 len 5 / `b = "x\ny" "z"` → 3:5-4:2 len 5 のみ。
    #[test]
    fn a_multiline_literal_is_judged_as_one_and_nested_chains_report_only_it() {
        CopCase::new(
            "Style/StringLiterals",
            "a = \"x\ny\"\nb = \"x\ny\" \"z\"\n",
            vec![
                Annotation::new(1, 5, 2, SINGLE),
                Annotation::new(3, 5, 2, SINGLE),
            ],
        )
        .config(CONSISTENT)
        .locations(&[(1, 5, 2, 2), (3, 5, 4, 2)])
        .lengths(&[5, 5])
        .correctable(false)
        .run();
    }

    // `EnforcedStyle: double_quotes` 側の分岐 (`unexpected_single_quotes?`) は
    // 子が「全部」単引用符で書き直せることを求める。
    //
    // 実測: `a = 'x' 'y'` → 1:5-1:11 len 7 / `b = 'x\ny'` → 2:5-3:2 len 5。
    #[test]
    fn single_quoted_multiline_literals_are_reported_under_double_quotes() {
        CopCase::new(
            "Style/StringLiterals",
            "a = 'x' 'y'\nb = 'x\ny'\n",
            vec![
                Annotation::new(1, 5, 7, DOUBLE),
                Annotation::new(2, 5, 2, DOUBLE),
            ],
        )
        .config(CONSISTENT_DOUBLE)
        .locations(&[(1, 5, 1, 11), (2, 5, 3, 2)])
        .lengths(&[7, 5])
        .correctable(false)
        .run();
    }

    // 補間を含む部分は本家では `dstr` なので `accept_child_double_quotes?` が
    // 真になり、連結全体が許される。そのうえで `ignore_node` は行われるため、
    // 素の `"y"` も報告されない。既定設定なら `"y"` は 1:13 で報告される。
    #[test]
    fn an_interpolated_part_excuses_the_whole_chain() {
        CopCase::new("Style/StringLiterals", "a = \"x#{b}\" \"y\"\n", Vec::new())
            .config(CONSISTENT)
            .run();
        CopCase::annotated(
            "Style/StringLiterals",
            r#"
            a = "x#{b}" "y"
                        ^^^ Prefer single-quoted strings [...]
            "#,
        )
        .run();
    }

    // `\` による行継続は double quote の中では 1 行のままなので `str`。
    // 単引用符の中ではバックスラッシュが改行を食わないので `dstr` になる。
    // どちらも offense にならないことを固定する (既定設定)。
    #[test]
    fn a_backslash_continuation_inside_a_literal_is_not_reported() {
        expect_no_offenses("Style/StringLiterals", "a = \"x\\\ny\"\nb = 'x\\\ny'\n");
    }
}

/// `Layout/SpaceAroundOperators` / `Layout/SpaceAfterComma` / `Layout/SpaceInsideParens`。
///
/// 本家はレキサのトークン列を歩くのに対し sonicop は tree-sitter の木を歩くので、
/// 「本家がどの構文を演算子として拾うか」と「Ruby のレキサと tree-sitter で読みが
/// 割れる構文」の 2 つが取りこぼしの源になる。ここではその両方を固定する。
///
/// 期待値はすべて本家 1.89.0 の `--only <cop> --format json` 実測。
mod layout_spacing {
    use super::*;

    const AROUND: &str = "Layout/SpaceAroundOperators";
    const COMMA: &str = "Layout/SpaceAfterComma";
    const PARENS: &str = "Layout/SpaceInsideParens";

    /// 本家は `on_pair` / `on_if` / `on_class` / `on_sclass` / `on_resbody` という
    /// 別々のハンドラで二項演算子以外の演算子も拾う。tree-sitter では `pair` /
    /// `conditional` / `superclass` / `singleton_class` / `exception_variable` が
    /// それぞれに対応する。
    #[test]
    fn every_handler_of_the_upstream_cop_has_a_node_kind() {
        CopCase::new(
            AROUND,
            concat!(
                "h = {1=>2, 3 =>4}\n",
                "a = 1\n",
                "b = 2\n",
                "c = 3\n",
                "t = a ? b:c\n",
                "class Foo<Bar\n",
                "end\n",
                "class<<self\n",
                "end\n",
                "begin\n",
                "rescue E=>e\n",
                "end\n",
            ),
            vec![
                Annotation::new(1, 7, 2, "Surrounding space missing for operator `=>`."),
                Annotation::new(1, 14, 2, "Surrounding space missing for operator `=>`."),
                Annotation::new(5, 10, 1, "Surrounding space missing for operator `:`."),
                Annotation::new(6, 10, 1, "Surrounding space missing for operator `<`."),
                Annotation::new(8, 6, 2, "Surrounding space missing for operator `<<`."),
                Annotation::new(11, 9, 2, "Surrounding space missing for operator `=>`."),
            ],
        )
        .run();
    }

    /// パターンマッチの `=>` は `on_match_pattern` が 3.0 以上でだけ動く。`|` は
    /// `on_match_alt`、`Integer => n` は `on_match_as`。
    #[test]
    fn pattern_matching_operators_need_ruby_three() {
        let source = concat!(
            "v = 5\n",
            "v => Integer\n",
            "case v\n",
            "in 1|2 then 1\n",
            "in Integer=>n then n\n",
            "end\n",
        );
        CopCase::new(
            AROUND,
            source,
            vec![
                Annotation::new(4, 5, 1, "Surrounding space missing for operator `|`."),
                Annotation::new(5, 11, 2, "Surrounding space missing for operator `=>`."),
            ],
        )
        .target_ruby("3.0")
        .run();
    }

    /// `range_with_surrounding_space` は演算子の右側だけ改行を飲む。行末の演算子は
    /// 前に空白があれば許され、無ければ報告される。行の残りがコメントなら
    /// `comment_at_line` で免除される。空行を挟むと右側は空白で終わらなくなる。
    #[test]
    fn a_line_break_counts_as_space_only_on_the_right() {
        CopCase::new(
            AROUND,
            concat!(
                "a = \"x\"+\n",
                "  \"y\"\n",
                "b = \"x\" +\n",
                "  \"y\"\n",
                "c = 1 + # note\n",
                "  2\n",
                "d = 1 +\n",
                "  2\n",
                "e = 1+\n",
                "\n",
                "  2\n",
            ),
            vec![
                Annotation::new(1, 8, 1, "Surrounding space missing for operator `+`."),
                Annotation::new(9, 6, 1, "Surrounding space missing for operator `+`."),
            ],
        )
        .run();
    }

    /// `rational_literal?` は `1/48r` を 1 個のリテラルとみなして send ごと飛ばす。
    /// 受け手が整数でなければ `EnforcedStyleForRationalLiterals` が効き、既定の
    /// `no_space` では空白のある `/` が報告される。`**` も同じ形。
    #[test]
    fn rational_and_exponent_operators_have_their_own_styles() {
        CopCase::new(
            AROUND,
            concat!(
                "a = 1 / 48r\n",
                "b = 2/3r\n",
                "c = x / 48r\n",
                "d = x/3r\n",
                "e = 2**3\n",
                "f = 2 ** 3\n",
                "g = 2 **\n",
                "  3\n",
            ),
            vec![
                Annotation::new(3, 7, 1, "Space around operator `/` detected."),
                Annotation::new(6, 7, 2, "Space around operator `**` detected."),
                Annotation::new(7, 7, 2, "Space around operator `**` detected."),
            ],
        )
        .run();
    }

    #[test]
    fn the_exponent_and_rational_styles_can_be_inverted() {
        CopCase::new(
            AROUND,
            "a = 2**3\nb = 2 ** 3\nc = x/3r\nd = x / 3r\n",
            vec![
                Annotation::new(1, 6, 2, "Surrounding space missing for operator `**`."),
                Annotation::new(3, 6, 1, "Surrounding space missing for operator `/`."),
            ],
        )
        .config(concat!(
            "Layout/SpaceAroundOperators:\n",
            "  EnforcedStyleForExponentOperator: space\n",
            "  EnforcedStyleForRationalLiterals: space\n",
        ))
        .run();
    }

    /// 省略可能引数の既定値の `=` は `Layout/SpaceAroundEqualsInParameterDefault` の
    /// 担当で、この cop は触らない。tree-sitter は `def f(a=nil, b=nil)` を 1 個の
    /// 省略可能引数と多重代入として読むので、その代入を演算子として数えないこと。
    #[test]
    fn parameter_defaults_belong_to_another_cop() {
        expect_no_offenses(
            AROUND,
            "def foo(x=nil, y=nil, z=nil)\n  x\nend\ndef bar(a=1, b=2)\n  a\nend\n",
        );
    }

    /// `=~` は 1 個のトークン。tree-sitter は `a[0] =~ /x/` を「`a[0]` に `~ /x/` を
    /// 代入」と読むので、`=` の直後が `~` なら `=~` として扱い直す。ただし左辺が
    /// 正規表現リテラルの `=~` は本家では `match_with_lvasgn` になりハンドラが無い。
    #[test]
    fn a_match_operator_is_a_single_token() {
        CopCase::new(
            AROUND,
            concat!(
                "a = \"s\"\n",
                "b = /x/=~a\n",
                "c = /x/ =~a\n",
                "d = a=~/x/\n",
                "e = /x/!~a\n",
                "f = %r{x}=~a\n",
                "g = a[0] =~ /x/ && !a[1]\n",
                "h = a[0]=~/x/\n",
                "i = a[0] = ~a\n",
            ),
            vec![
                Annotation::new(4, 6, 2, "Surrounding space missing for operator `=~`."),
                Annotation::new(5, 8, 2, "Surrounding space missing for operator `!~`."),
                Annotation::new(8, 9, 2, "Surrounding space missing for operator `=~`."),
            ],
        )
        .run();
    }

    /// `a&b` は Ruby のレキサでは二項演算子、`a &b` はブロック渡し。tree-sitter は
    /// どちらもブロック渡しに読むので、空白の有無で区別し直す。
    #[test]
    fn an_ampersand_without_a_leading_space_is_the_binary_operator() {
        CopCase::new(
            AROUND,
            concat!(
                "z = [1]\n",
                "w = z&z\n",
                "v = z & z\n",
                "u = z.map(&:to_s)\n",
                "t = z.each &:to_s\n",
                "s = z.first&.to_s\n",
            ),
            vec![Annotation::new(
                2,
                6,
                1,
                "Surrounding space missing for operator `&`.",
            )],
        )
        .run();
    }

    /// 余分な空白は「隣の行と揃っている」ときだけ許される。揃っていないものは
    /// `Operator ... should be surrounded by a single space.` になる。
    #[test]
    fn padding_is_allowed_only_where_it_lines_up_with_a_neighbour() {
        CopCase::new(
            AROUND,
            concat!(
                "h = {\n",
                "  1 =>  2,\n",
                "  11 => 3\n",
                "}\n",
                "g = {\n",
                "  \"aaa\"   => 1,\n",
                "  \"b\" => 2\n",
                "}\n",
                "x   = 1\n",
                "yyy = 2\n",
            ),
            vec![Annotation::new(
                6,
                11,
                2,
                "Operator `=>` should be surrounded by a single space.",
            )],
        )
        .run();
    }

    /// `AllowForAlignment: false` でも `excess_leading_space?` は先に
    /// `allow_for_alignment?` を見て抜けるので、左側の余白は報告されない。
    /// 報告されるのは右側の余白だけ。
    #[test]
    fn disallowing_alignment_only_reaches_the_trailing_padding() {
        CopCase::new(
            AROUND,
            "h = {\n  1 =>  2,\n  11 => 3\n}\nx   = 1\nyyy = 2\n",
            vec![Annotation::new(
                2,
                5,
                2,
                "Operator `=>` should be surrounded by a single space.",
            )],
        )
        .config("Layout/SpaceAroundOperators:\n  AllowForAlignment: false\n")
        .run();
    }

    /// `Layout/HashAlignment` が table なら、1 行 1 要素で書かれたハッシュの
    /// ロケットは揃えるためのものとして丸ごと見逃される。
    #[test]
    fn a_table_style_hash_keeps_its_padded_rockets() {
        CopCase::new(
            AROUND,
            concat!(
                "h = {\n",
                "  \"aaa\"   => 1,\n",
                "  \"b\" => 2\n",
                "}\n",
                "g = { \"aaa\"   => 1, \"b\" => 2 }\n",
            ),
            vec![Annotation::new(
                5,
                15,
                2,
                "Operator `=>` should be surrounded by a single space.",
            )],
        )
        .config("Layout/HashAlignment:\n  EnforcedHashRocketStyle: table\n")
        .run();
    }

    /// 文字列補間やヒアドキュメントの中身も本家では普通にトークン化される。
    #[test]
    fn operators_inside_interpolation_are_still_operators() {
        CopCase::new(
            AROUND,
            "n = 1\ns = <<~TEXT\n  line #{n-1} here\nTEXT\n",
            vec![Annotation::new(
                3,
                11,
                1,
                "Surrounding space missing for operator `-`.",
            )],
        )
        .run();
    }

    #[test]
    fn space_around_operators_autocorrects_like_upstream() {
        expect_correction(
            AROUND,
            concat!(
                "a=1\n",
                "h = {1=>2}\n",
                "b = \"x\"+\n",
                "  \"y\"\n",
                "c = 2 ** 3\n",
                "d = x / 3r\n",
                "e = {\n",
                "  \"aaa\"   => 1,\n",
                "  \"b\" => 2\n",
                "}\n",
            ),
            concat!(
                "a = 1\n",
                "h = {1 => 2}\n",
                "b = \"x\" +\n",
                "  \"y\"\n",
                "c = 2**3\n",
                "d = x/3r\n",
                "e = {\n",
                "  \"aaa\" => 1,\n",
                "  \"b\" => 2\n",
                "}\n",
            ),
        );
    }

    /// 空の括弧の中の空白も本家は報告する。`(` の側から 1 件だけ出るので、
    /// `)` の側では二重に数えない。行末やコメントが続く場合は「同じ行の次の
    /// トークン」が無いので対象外。
    #[test]
    fn empty_parentheses_are_reported_once_from_the_opening_side() {
        CopCase::new(
            PARENS,
            concat!(
                "a = foo( )\n",
                "b = foo(  )\n",
                "c = foo( 3 )\n",
                "d = foo( # note\n",
                ")\n",
                "e = foo(\n",
                "  3\n",
                ")\n",
                "f = ( 1 )\n",
                "g = %w( a b )\n",
                "h = %i( a )\n",
            ),
            vec![
                Annotation::new(1, 9, 1, "Space inside parentheses detected."),
                Annotation::new(2, 9, 2, "Space inside parentheses detected."),
                Annotation::new(3, 9, 1, "Space inside parentheses detected."),
                Annotation::new(3, 11, 1, "Space inside parentheses detected."),
                Annotation::new(9, 6, 1, "Space inside parentheses detected."),
                Annotation::new(9, 8, 1, "Space inside parentheses detected."),
            ],
        )
        .run();
    }

    /// `)` `]` `|` の前は空白を求めない。`}` は
    /// `Layout/SpaceInsideHashLiteralBraces` が `no_space` のときだけ免除される。
    /// 文字列リテラルやヒアドキュメントの終端記号の中のコンマはトークンでは
    /// ないが、補間の中のコンマはトークンなので報告される。
    #[test]
    fn only_a_comma_the_parser_saw_is_a_comma() {
        let source = concat!(
            "a = {x: 1,}\n",
            "b = [1,]\n",
            "c = foo(1,)\n",
            "d = [1,2]\n",
            "e = \"#{d[0,1]}\"\n",
            "f = <<-'},'\n",
            "body\n",
            "},\n",
            "g = %w[a,b]\n",
            "h = :\"a,b\"\n",
            "i = [1,# note\n",
            "     2]\n",
        );
        CopCase::new(
            COMMA,
            source,
            vec![
                Annotation::new(1, 10, 1, "Space missing after comma."),
                Annotation::new(4, 7, 1, "Space missing after comma."),
                Annotation::new(5, 11, 1, "Space missing after comma."),
                Annotation::new(11, 7, 1, "Space missing after comma."),
            ],
        )
        .run();
        CopCase::new(
            COMMA,
            "a = {x: 1,}\nb = [1,2]\n",
            vec![Annotation::new(2, 7, 1, "Space missing after comma.")],
        )
        .config("Layout/SpaceInsideHashLiteralBraces:\n  EnforcedStyle: no_space\n")
        .run();
    }

    #[test]
    fn parens_and_commas_autocorrect_like_upstream() {
        expect_correction(
            PARENS,
            "a = foo( )\nb = foo( 3 )\n",
            "a = foo()\nb = foo(3)\n",
        );
        expect_correction(
            COMMA,
            "c = {x: 1,}\nd = [1,2]\n",
            "c = {x: 1, }\nd = [1, 2]\n",
        );
    }
}

/// 名前の綴りを見る cop 群。リテラルの実体値と、tree-sitter が読み違える
/// カンマ区切りリストの扱いが対象。
mod naming_literals {
    use super::*;

    /// `sym` / `str` の値はエスケープを解いた後の文字列なので、`:"a\000"` は NUL を含む
    /// 名前になる。エスケープを字面のまま読むと `a000` になって snake_case を通ってしまう。
    #[test]
    fn method_name_reads_the_value_a_literal_stands_for() {
        expect_offense(
            "Naming/MethodName",
            r#"
            Data.define(:"a\000")
                        ^^^^^^^^ Use snake_case for method names.
            define_method("\u{3042}") {}
                          ^^^^^^^^^^^ Use snake_case for method names.
            define_method(:"\t") { :tab }
                          ^^^^^^ Use snake_case for method names.
            define_method("a \"b\" c") {}
                          ^^^^^^^^^^^^ Use snake_case for method names.
            define_method('a\nb') {}
                          ^^^^^^^ Use snake_case for method names.
            "#,
        );
    }

    /// 単一引用符の中の `\n` はエスケープではなく 2 文字。解いてはいけない。
    #[test]
    fn method_name_keeps_a_single_quoted_backslash_literal() {
        expect_no_offenses("Naming/MethodName", "define_method('a_b') {}\n");
    }

    /// `rescue => Foo` は本家では `casgn` なので、値を持たない定数代入として報告される。
    #[test]
    fn constant_name_reports_a_constant_a_rescue_clause_binds() {
        expect_offense(
            "Naming/ConstantName",
            r#"
            begin
              raise 'x'
            rescue => CapturedError
                      ^^^^^^^^^^^^^ Use SCREAMING_SNAKE_CASE for constants.
            end
            "#,
        );
        expect_no_offenses(
            "Naming/ConstantName",
            "begin\n  raise 'x'\nrescue => CAPTURED\nend\n",
        );
    }

    /// tree-sitter は `foo(A, b = 1)` を「`A` を巻き込んだ多重代入」として読むが、Ruby は
    /// カンマごとにリストを閉じるので `A` は引数、代入されるのは `b` だけ。仮引数の既定値
    /// (`def m(a = A, b = 2)`) と `__FILE__` を挟む形も同じ読み違えをする。
    #[test]
    fn a_comma_list_is_not_a_multiple_assignment() {
        expect_no_offenses(
            "Naming/ConstantName",
            concat!(
                "assert_kind_of(Integer, a = Object.new)\n",
                "puts a\n",
                "def def_class(superklass = Object, methodname = 'result')\n",
                "  [superklass, methodname]\n",
                "end\n",
            ),
        );
        expect_no_offenses(
            "Naming/VariableName",
            concat!(
                "m.module_eval \"A = 1\", __FILE__, line = __LINE__\n",
                "puts line\n",
            ),
        );
    }
}

/// `VariableForce` を土台にする 2 cop。スコープの入れ子、評価順、ブロックによる捕捉が
/// 結果を決めるので、その 3 つを崩さないための回帰テストを置く。
mod local_variable_analysis {
    use super::*;

    const UNUSED_ARGUMENT: &str = "Lint/UnusedBlockArgument";
    const USELESS: &str = "Lint/UselessAssignment";

    /// 内側のブロックが同じ名前を再束縛したら、外側の引数は読まれていない。名前で本文を
    /// 探すだけの実装はここで内側の束縛を外側の参照と取り違える。
    #[test]
    fn an_inner_binding_does_not_reference_the_outer_argument() {
        expect_offense(
            UNUSED_ARGUMENT,
            r#"
            x = ->(a) { ->(a) { 1 } }
                   ^ Unused block argument - `a`. If it's necessary, use `_` or `_a` as an argument name to indicate that it won't be used. Also consider using a proc without arguments instead of a lambda if you want it to accept any arguments but don't care about them.
                           ^ Unused block argument - `a`. If it's necessary, use `_` or `_a` as an argument name to indicate that it won't be used. Also consider using a proc without arguments instead of a lambda if you want it to accept any arguments but don't care about them.
            puts x
            "#,
        );
    }

    /// メソッド名は変数の読みではない。`Etc.group` の `group` をブロック引数 `group` の
    /// 参照と数えると、この offense が消える。
    #[test]
    fn a_method_of_the_same_name_is_not_a_reference() {
        expect_offense(
            UNUSED_ARGUMENT,
            r#"
            Etc.group do |group|
                          ^^^^^ Unused block argument - `group`. You can omit the argument if you don't care about it.
              Etc.group do |group2|
              end
            end
            "#,
        );
    }

    /// `binding` が引数として束縛されていれば変数の読みで、スコープを渡す `binding` 呼び出し
    /// ではない。呼び出しと取り違えると同じブロックの引数が全部「参照済み」になる。
    #[test]
    fn a_binding_parameter_is_not_a_binding_call() {
        expect_offense(
            UNUSED_ARGUMENT,
            r#"
            set_trace_func ->(event, file, line, id, binding, klass) do
                                     ^^^^ Unused block argument - `file`. If it's necessary, use `_` or `_file` as an argument name to indicate that it won't be used.
                                           ^^^^ Unused block argument - `line`. If it's necessary, use `_` or `_line` as an argument name to indicate that it won't be used.
                                                 ^^ Unused block argument - `id`. If it's necessary, use `_` or `_id` as an argument name to indicate that it won't be used.
                                                              ^^^^^ Unused block argument - `klass`. If it's necessary, use `_` or `_klass` as an argument name to indicate that it won't be used.
              stf_b = binding if event == 'raise'
              stf_b
            end
            "#,
        );
    }

    /// `{ |f| ; }` の本体は文を 1 つも持たないので、本家の `empty_block?` では body が nil。
    /// `IgnoreEmptyBlocks` の既定で見逃す側になる。
    #[test]
    fn a_block_holding_only_a_separator_is_empty() {
        expect_no_offenses(UNUSED_ARGUMENT, "File.open('f', 'w') { |f|\n  ;\n}\n");
    }

    /// ヒアドキュメントの本体は tree-sitter では文の側にぶら下がるが、補間が読む変数は
    /// `<<EOF` を書いたブロックのもの。
    #[test]
    fn a_heredoc_body_reads_the_block_it_was_opened_in() {
        expect_no_offenses(
            UNUSED_ARGUMENT,
            concat!(
                "have_func_decl = proc do |name, headers|\n",
                "  %w[int void].all? { |ret| try_compile(<<EOF) }\n",
                "#{headers} #{ret} #{name}(void);\n",
                "EOF\n",
                "end\n",
                "puts have_func_decl\n",
            ),
        );
    }

    /// 分岐の中の読みは、分岐の外の代入を使ったことにならない。
    #[test]
    fn a_read_inside_a_branch_does_not_use_an_assignment_outside_it() {
        expect_offense(
            USELESS,
            r#"
            def m(flag)
              x = 1
              ^ Useless assignment to variable - `x`.
              if flag
                x = 2
                puts x
              end
            end
            "#,
        );
    }

    /// ブロックが捕まえた変数はいつ読まれるか分からないので、上書きされていない代入は
    /// 使われたものとして扱う。
    #[test]
    fn an_assignment_captured_by_a_block_counts_as_used() {
        expect_no_offenses(
            USELESS,
            "def m\n  result = compute\n  [1].each { result = 2 }\n  result\nend\n",
        );
    }

    /// ループの条件が読む変数への代入は、次の周回で読まれるので死んでいない。
    #[test]
    fn an_assignment_a_loop_reads_again_is_not_dead() {
        expect_no_offenses(
            USELESS,
            "def m\n  i = 0\n  while i < 10\n    i += 1\n  end\nend\n",
        );
    }

    /// `for` の変数は本家では代入で、読まれなければ報告される。ループ走査でこれを参照と
    /// 数えてしまうと消える。
    #[test]
    fn an_unread_for_variable_is_reported() {
        expect_offense(
            USELESS,
            r#"
            def m
              for dummy in 0..3
                  ^^^^^ Useless assignment to variable - `dummy`.
                puts 1
              end
            end
            "#,
        );
    }

    /// スコープの戻り値になっている演算代入だけ、演算子を提案する文面が足される。
    #[test]
    fn an_operator_assignment_in_return_position_names_the_operator() {
        expect_offense(
            USELESS,
            r#"
            def m
              x = 0
              x ^= 1
              ^ Useless assignment to variable - `x`. Use `^` instead of `^=`.
            end
            "#,
        );
    }

    /// `rescue => ex` は代入なので、使われなければ報告される。
    #[test]
    fn an_unread_exception_variable_is_reported() {
        expect_offense(
            USELESS,
            r#"
            def m
              begin
                do_something
              rescue StandardError => ex
                                      ^^ Useless assignment to variable - `ex`.
                :handled
              end
            end
            "#,
        );
    }

    /// 名前付きキャプチャは `match_with_lvasgn` として変数を作る。報告位置は変数名ではなく
    /// 正規表現リテラル。
    #[test]
    fn a_regexp_named_capture_declares_a_variable() {
        expect_offense(
            USELESS,
            r#"
            def m(text)
              /(?<year>\d+)/ =~ text
              ^^^^^^^^^^^^^^ Useless assignment to variable - `year`.
              puts 1
            end
            "#,
        );
        // 補間を含む正規表現はリテラルとして畳めないので、本家では変数を作らない。
        expect_no_offenses(
            USELESS,
            "def m(text, part)\n  /(?<year>#{part})/ =~ text\n  puts 1\nend\n",
        );
    }

    /// 似た名前がスコープにあれば綴り間違いとして提案する。順位付けは Ruby の
    /// `DidYouMean::SpellChecker` そのもの。
    #[test]
    fn a_similar_name_in_the_scope_is_suggested() {
        expect_offense(
            USELESS,
            r#"
            def m
              stretch_depth = 5
              stretch_tree = 1
              ^^^^^^^^^^^^ Useless assignment to variable - `stretch_tree`. Did you mean `stretch_depth`?
              puts stretch_depth
            end
            "#,
        );
    }

    /// 多重代入の要素は消せないので、`_` 前置を勧める文面になる。
    #[test]
    fn a_multiple_assignment_target_suggests_an_underscore() {
        expect_offense(
            USELESS,
            r#"
            def m
              a, b = 1, 2
                 ^ Useless assignment to variable - `b`. Use `_` or `_b` as a variable name to indicate that it won't be used.
              puts a
            end
            "#,
        );
    }

    /// 引数なしの `super` はメソッドの引数を全部渡すので、全部読まれている。
    /// `def n(config = nil, options = nil)` は tree-sitter がカンマリストを読み違える形でもある。
    #[test]
    fn a_bare_super_reads_every_method_argument() {
        expect_no_offenses(
            USELESS,
            "class Foo\n  def initialize(config = nil, options = nil)\n    super\n  end\nend\n",
        );
    }

    /// 後置 `if` の条件でした代入は、キーワードの左にある読みからは見えない。逆に条件の
    /// 代入自体は使われている。
    #[test]
    fn an_assignment_in_a_modifier_condition_is_used_by_the_body() {
        expect_no_offenses(USELESS, "def m\n  v = 1\n  \"#{v}\" if v &&= v.to_s\nend\n");
    }

    /// `DirectiveComment::DIRECTIVE_COMMENT_REGEXP` は cop 名の並びまでしか読まず、後ろに
    /// 続く散文は無視する。行全体を 1 つの名前として読むと、この disable が効かなくなる。
    #[test]
    fn a_directive_may_be_followed_by_prose() {
        expect_no_offenses(
            USELESS,
            concat!(
                "def m(block)\n",
                "  count = 1\n",
                "  handle = nil # rubocop:disable Lint/UselessAssignment avoid holding it\n",
                "  [count, block]\n",
                "end\n",
            ),
        );
    }

    /// ループ走査は代入ノードを `Array#include?` で探すので、比較は同一性ではなく構造。
    /// 同じ綴りの代入はループの外のものまで「ループ内の代入」と数えられ、条件分岐の中に
    /// あればまとめて参照済みになる。
    #[test]
    fn a_loop_marks_every_assignment_written_the_same_way() {
        expect_no_offenses(
            USELESS,
            concat!(
                "if true\n",
                "  def m\n",
                "    for i in 1..10\n",
                "      puts 1\n",
                "    end\n",
                "    for i in 1..10\n",
                "      puts i\n",
                "    end\n",
                "  end\n",
                "end\n",
            ),
        );
        // 分岐の外なら、参照されるのは最後の 1 件だけ。
        expect_offense(
            USELESS,
            r#"
            def m
              for i in 1..10
                  ^ Useless assignment to variable - `i`.
                puts 1
              end
              for i in 1..10
                puts i
              end
            end
            "#,
        );
        // 綴りが違えば別の代入なので、分岐の中でも最初の 1 件は残る。
        expect_offense(
            USELESS,
            r#"
            if true
              def m
                i = 1
                ^ Useless assignment to variable - `i`.
                for i in 1..10
                  puts i
                end
              end
            end
            "#,
        );
    }

    /// `for a, b in list` の要素は `masgn` ではなく `for_assignment` なので、`_` 前置の案内は
    /// 付かない。
    #[test]
    fn a_for_loop_target_is_not_a_multiple_assignment() {
        expect_offense(
            USELESS,
            r#"
            def m
              for i, j in { 1 => 10 }
                     ^ Useless assignment to variable - `j`.
                puts i
              end
            end
            "#,
        );
    }

    /// ヒアドキュメントの本体は開いた文の一部なので、スコープの戻り値はその手前の文。
    /// 本体を戻り値と見ると演算子の案内が消える。
    #[test]
    fn a_trailing_heredoc_body_is_not_the_return_value() {
        expect_offense(
            USELESS,
            r#"
            def m(name)
              code = "a"
              return code unless name

              code += <<~TEXT
              ^^^^ Useless assignment to variable - `code`. Use `+` instead of `+=`.
                body
              TEXT
            end
            "#,
        );
    }

    /// `/\c#{str}/` の `#` はエスケープの引数で、補間ではない。補間として読むと `str` が
    /// 使われたことになってしまう。
    #[test]
    fn an_escape_can_swallow_the_hash_of_an_interpolation() {
        expect_offense(
            USELESS,
            r#"
            def m
              str = "J"
              ^^^ Useless assignment to variable - `str`.
              /\c#{str}/.to_s
            end
            "#,
        );
    }

    /// `begin` の本体は raise で途中終了しうるので、後ろの読みが手前の代入を使ったことに
    /// ならない。本体全体で 1 つの分岐として扱わないと、文ごとに別の分岐になって
    /// 手前の代入まで参照済みになる。
    #[test]
    fn the_guarded_body_of_a_begin_is_one_branch() {
        expect_offense(
            USELESS,
            r#"
            def m(source)
              a = source.read(1)
              ^ Useless assignment to variable - `a`.
              b = source.read(2)
              ^ Useless assignment to variable - `b`.
              a, b = source.read(3), 4
              [a, b]
            ensure
              source.close
            end
            "#,
        );
    }

    /// `"%3d"%[1]` の `%` は演算子で、リテラルの始まりではない。文字列の連結として読むと
    /// 中に書かれた変数の読みが見えなくなる。
    #[test]
    fn a_percent_literal_after_a_string_holds_code() {
        expect_offense(
            UNUSED_ARGUMENT,
            r#"
            def m(list)
              list.each_with_index do |line, l|
                                       ^^^^ Unused block argument - `line`. You can omit all the arguments if you don't care about them.
                                             ^ Unused block argument - `l`. You can omit all the arguments if you don't care about them.
                puts "%3d"%[1]
              end
            end
            "#,
        );
        expect_no_offenses(
            UNUSED_ARGUMENT,
            concat!(
                "def m(list)\n",
                "  list.each_with_index do |line, l|\n",
                "    puts \"%3d %s\"%[l+1, line]\n",
                "  end\n",
                "end\n",
            ),
        );
    }
}

/// `Style/Semicolon` — 期待値は本家 1.89.0 の `--only Style/Semicolon` 実測。
mod semicolon_shapes {
    use super::*;

    const SEMICOLON: &str = "Style/Semicolon";

    /// 本家は式の分割まで直す。`;` を改行に置き換えるので、後ろの空白はそのまま残る。
    #[test]
    fn a_separator_between_expressions_is_corrected_into_a_line_break() {
        CopCase::annotated(
            SEMICOLON,
            r#"
            puts 1; puts 2
                  ^ Do not use semicolons to terminate expressions.
            "#,
        )
        .correctable(true)
        .corrected("puts 1\n puts 2\n")
        .run();
    }

    /// 改行に置き換えると行の残りが heredoc 本文に落ちるため、本家は corrector を
    /// 1 つも積まない。offense は出るが correctable ではない。
    #[test]
    fn a_heredoc_opened_earlier_on_the_line_leaves_the_separator_uncorrectable() {
        CopCase::annotated(
            SEMICOLON,
            r#"
            x = <<~MSG; y = 2
                      ^ Do not use semicolons to terminate expressions.
              hi
            MSG
            "#,
        )
        .correctable(false)
        .run();
    }

    /// 本家はトークン列を歩くので、`$;` はグローバル変数 1 トークンであって `;` ではない。
    #[test]
    fn the_global_variable_named_semicolon_is_not_a_semicolon() {
        expect_no_offenses(SEMICOLON, "alias $FS $;\n");
    }

    /// コメントもトークンなので、行末コメントがあると `;` は行の最後のトークンではなくなる。
    #[test]
    fn a_trailing_comment_takes_the_last_token_position_from_the_semicolon() {
        expect_no_offenses(SEMICOLON, "x = 1;   # note\n");
    }

    /// `begin ... end` は本家 AST では文をそのまま抱える `kwbegin` で、cop が探す
    /// `begin` ノードにはならない。ループ本体は `begin` になるので報告される。
    #[test]
    fn a_begin_block_does_not_separate_expressions_but_a_loop_body_does() {
        CopCase::annotated(
            SEMICOLON,
            r#"
            begin a = 1; b = 2 end
            while true
              c = 1; d = 2
                   ^ Do not use semicolons to terminate expressions.
            end
            "#,
        )
        .run();
    }
}

/// `Style/RedundantReturn` — 期待値は本家 1.89.0 の `--only Style/RedundantReturn` 実測。
mod redundant_return_branches {
    use super::*;

    const REDUNDANT_RETURN: &str = "Style/RedundantReturn";

    #[test]
    fn every_branch_of_a_conditional_is_followed() {
        CopCase::annotated(
            REDUNDANT_RETURN,
            r#"
            def a
              if x
                return 1
                ^^^^^^ Redundant `return` detected.
              elsif y
                return 2
                ^^^^^^ Redundant `return` detected.
              else
                return 3
                ^^^^^^ Redundant `return` detected.
              end
            end
            "#,
        )
        .corrected("def a\n  if x\n    1\n  elsif y\n    2\n  else\n    3\n  end\nend\n")
        .run();
    }

    #[test]
    fn a_rescue_body_and_its_else_are_followed() {
        CopCase::annotated(
            REDUNDANT_RETURN,
            r#"
            def a
              x
            rescue Foo
              return 1
              ^^^^^^ Redundant `return` detected.
            else
              return 2
              ^^^^^^ Redundant `return` detected.
            end
            "#,
        )
        .run();
    }

    /// `ensure` の中身は戻り値にならないので、本家は追わない。
    #[test]
    fn an_ensure_body_is_not_followed() {
        expect_no_offenses(REDUNDANT_RETURN, "def a\n  x\nensure\n  return 1\nend\n");
    }

    #[test]
    fn case_branches_are_followed() {
        CopCase::annotated(
            REDUNDANT_RETURN,
            r#"
            def a
              case x
              when 1
                return 1
                ^^^^^^ Redundant `return` detected.
              else
                return 2
                ^^^^^^ Redundant `return` detected.
              end
            end
            "#,
        )
        .run();
    }

    /// `lambda {}` も `-> {}` も本家には `lambda` 呼び出しなので、本体はメソッド本体扱い。
    #[test]
    fn a_lambda_body_is_a_method_body() {
        CopCase::annotated(
            REDUNDANT_RETURN,
            r#"
            def a
              lambda { return 1 }
                       ^^^^^^ Redundant `return` detected.
            end
            def b
              -> { return 2 }
                   ^^^^^^ Redundant `return` detected.
            end
            "#,
        )
        .corrected("def a\n  lambda { 1 }\nend\ndef b\n  -> { 2 }\nend\n")
        .run();
    }

    /// ループ本体の `return` は末尾式ではないので追わない。修飾子付きの `if` は追う。
    #[test]
    fn a_loop_body_is_not_followed_but_a_modifier_branch_is() {
        CopCase::annotated(
            REDUNDANT_RETURN,
            r#"
            def a
              while x
                return 1
              end
            end
            def b
              return 1 if y
              ^^^^^^ Redundant `return` detected.
            end
            "#,
        )
        .run();
    }
}

/// `Layout/LineLength` の autocorrect 可否。期待値は本家 1.89.0 の実測。
mod line_length_breakable {
    use super::*;

    const LINE_LENGTH: &str = "Layout/LineLength";
    const TOO_LONG: &str = "Line is too long. [126/120]";

    /// 引数が 2 つ以上ある呼び出しは、上限を越える最初の引数の 1 つ手前で折れる。
    #[test]
    fn a_long_call_breaks_before_the_last_argument_within_the_limit() {
        let long = "        assert_equal(:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, \
                    :bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, :cccccccccccccccccccccccccccccccc)";
        CopCase::new(
            LINE_LENGTH,
            format!("def foo\n{long}\nend\n"),
            vec![Annotation::new(2, 121, 6, TOO_LONG)],
        )
        .correctable(true)
        .run();
    }

    /// メソッド定義は本体ではなく引数リストの広がりで「もう折れている」かを見るので、
    /// 本体が複数行でも 1 行に収まった引数リストは折れる。
    #[test]
    fn a_long_definition_breaks_inside_its_parameter_list() {
        let long = "  def grouped_collection_select(method, collection, group_method, \
                    group_label_method, option_key_method, option_value_method, options = {})";
        CopCase::new(
            LINE_LENGTH,
            format!("class A\n{long}\n    1\n  end\nend\n"),
            vec![Annotation::new(2, 121, 19, "Line is too long. [139/120]")],
        )
        .correctable(true)
        .run();
    }

    /// `__END__` から後ろはデータであってコードではない。本家は行の走査から外す。
    #[test]
    fn the_data_section_is_not_measured() {
        expect_no_offenses(
            LINE_LENGTH,
            &format!("x = 1\n__END__\n{}\n", "D".repeat(130)),
        );
    }

    /// endless method は通常のメソッドに書き直せるので、本家は常に correctable にする。
    #[test]
    fn an_endless_method_is_rewritten_as_a_regular_one() {
        let body = format!("/<conversation-{}>/", "x".repeat(110));
        CopCase::new(
            LINE_LENGTH,
            format!("class A\n  def conversation_header_regex = {body}\nend\n"),
            vec![Annotation::new(2, 121, 41, "Line is too long. [161/120]")],
        )
        .target_ruby("3.0")
        .correctable(true)
        .corrected(&format!(
            "class A\n  def conversation_header_regex\n    {body}\n  end\nend\n"
        ))
        .run();
    }
}

/// `Style/HashSyntax` が新記法で書けると認めるシンボル。期待値は本家 1.89.0 の実測。
mod hash_syntax_symbols {
    use super::*;

    const HASH_SYNTAX: &str = "Style/HashSyntax";
    const MSG_19: &str = "Use the new Ruby 1.9 hash syntax.";

    /// 末尾の `?` / `!` は新記法で書けるが、`=` は書けない。書けないキーが 1 つでも
    /// あると、その hash は 2 つの記法が混ざらないよう丸ごと見送られる。
    #[test]
    fn a_trailing_question_mark_is_acceptable_but_a_setter_is_not() {
        CopCase::new(
            HASH_SYNTAX,
            "a = { begin: 1, end: 2, :exclude_end? => false }\nc = { :foo => 1, :foo= => 2 }\n",
            vec![Annotation::new(1, 25, 16, MSG_19)],
        )
        .corrected("a = { begin: 1, end: 2, exclude_end?: false }\nc = { :foo => 1, :foo= => 2 }\n")
        .run();
    }

    /// クォート付きシンボルは中身が何であれ新記法で書ける。
    #[test]
    fn a_quoted_symbol_is_acceptable() {
        CopCase::new(
            HASH_SYNTAX,
            "b = { :\"user name\" => \"z\" }\n",
            vec![Annotation::new(1, 7, 15, MSG_19)],
        )
        .corrected("b = { \"user name\": \"z\" }\n")
        .run();
    }
}

/// `Security/Eval` — 期待値は本家 1.89.0 の実測。
mod eval_literal_code {
    use super::*;

    /// 補間された heredoc は本家 AST では素の `str` なので、本文がリテラルなら
    /// 評価される文字列全体もリテラルであって offense にならない。
    #[test]
    fn an_interpolated_heredoc_of_literal_text_is_not_an_offense() {
        expect_no_offenses("Security/Eval", "eval(\"#{<<~A}\")\n  literal\nA\n");
    }
}

/// `Layout/LineLength` が同じ行の候補のうちどれを折るか。どちらも本家 AST と文法の
/// 走査順の違いが出る形で、期待値は本家 1.89.0 の `-A` 実測。
mod line_length_visit_order {
    use super::*;

    const LINE_LENGTH: &str = "Layout/LineLength";

    /// 引数の中のラムダのブロックは、そのブロックを持つ呼び出しのブロックより **後** に
    /// 走査されて上書きする。文法は逆順に並べるので、並べ直さないと外側で折ってしまう。
    #[test]
    fn a_block_written_in_the_arguments_wins_over_the_one_that_owns_them() {
        let long = "    field(:text, type: 'text', analyzer: 'verbatim', \
                    value: ->(account) { account.searchable_text }) \
                    { field :stemmed, type: 'text', analyzer: 'natural' }";
        CopCase::new(
            LINE_LENGTH,
            format!("class A\n  def self.call\n{long}\n  end\nend\n"),
            vec![Annotation::new(3, 121, 34, "Line is too long. [154/120]")],
        )
        .corrected(concat!(
            "class A\n",
            "  def self.call\n",
            "    field(:text, type: 'text', analyzer: 'verbatim', value: ->(account) {\n",
            " account.searchable_text }) { field :stemmed, type: 'text', analyzer: 'natural' }\n",
            "  end\n",
            "end\n",
        ))
        .run();
    }

    /// 修飾子の条件は本家 AST では本体より先に来るので、同じ行では条件側の呼び出しが
    /// 先に折り位置を取る。
    #[test]
    fn a_modifier_condition_is_reached_before_its_body() {
        let long = "    errors.add(:base, I18n.t('scheduled_statuses.over_daily_limit', \
                    limit: DAILY_LIMIT)) if account.scheduled_statuses\
                    .where('scheduled_at::date = ?::date', scheduled_at).count >= DAILY_LIMIT";
        CopCase::new(
            LINE_LENGTH,
            format!("class A\n  def validate_daily_limit\n{long}\n  end\nend\n"),
            vec![Annotation::new(3, 121, 71, "Line is too long. [191/120]")],
        )
        .corrected(concat!(
            "class A\n",
            "  def validate_daily_limit\n",
            "    errors.add(:base, \n",
            "I18n.t('scheduled_statuses.over_daily_limit', limit: DAILY_LIMIT)) ",
            "if account.scheduled_statuses.where(\n",
            "'scheduled_at::date = ?::date', scheduled_at).count >= DAILY_LIMIT\n",
            "  end\n",
            "end\n",
        ))
        .run();
    }
}

/// 版に無い構文と、それを踏んだ本家の parser がどこまで読み直せるか。
///
/// 本家は `TargetRubyVersion` で固定した parser gem を動かすので、その版の文法に
/// 無い構文はすべて構文エラーになる。tree-sitter は版に関係なく最新の文法で受理
/// するため、版ごとのゲートと、誤りのあとに parser が何を読むかを手で持つ。
///
/// 期待値はすべて rubocop 1.89.0 の `--only Lint/Syntax --format json` 実測。
mod syntax_version_gates {
    use super::*;

    const HINT: &str =
        "(Using Ruby 2.7 parser; configure using `TargetRubyVersion` parameter, under `AllCops`)";

    fn unexpected(line: usize, column: usize, length: usize, token: &str) -> Annotation {
        Annotation::new(
            line,
            column,
            length,
            format!("unexpected token {token}\n{HINT}"),
        )
    }

    fn at_2_7(source: &str, expected: Vec<Annotation>) -> CopCase {
        CopCase::new("Lint/Syntax", source, expected).target_ruby("2.7")
    }

    fn accepted(source: &str, version: &str) -> CopCase {
        CopCase::new("Lint/Syntax", source, Vec::new()).target_ruby(version)
    }

    /// 検索パターン `[*, x, *]` は 3.0 から。2.7 の配列パターンに入る splat は 1 つ
    /// なので、2 つ目が読めなくなった位置になる。
    ///
    /// 実測: `case x / in [*, 1, *]` → 2:11 tSTAR / `x in [*a, 1, *b]` → 1:14 tSTAR
    #[test]
    fn a_find_pattern_needs_ruby_3_0() {
        at_2_7(
            "case x\nin [*, 1, *]\n  y\nend\n",
            vec![unexpected(2, 11, 1, "tSTAR")],
        )
        .run();
        at_2_7("x in [*a, 1, *b]\n", vec![unexpected(1, 14, 1, "tSTAR")]).run();
        accepted("case x\nin [*, 1, *]\n  y\nend\n", "3.0").run();
    }

    /// 一行のパターンマッチは自分の文で終わるので、parser は次の文を読み直せる。
    ///
    /// 実測: `w = z in [*, 1, *]` と `s = z in [*, 2, *]` の 2 件とも報告される
    #[test]
    fn a_one_line_pattern_match_lets_the_parser_pick_the_next_statement_up() {
        at_2_7(
            "z = [1]\nw = z in [*, 1, *]\ns = z in [*, 2, *]\n",
            vec![unexpected(2, 17, 1, "tSTAR"), unexpected(3, 17, 1, "tSTAR")],
        )
        .run();
    }

    /// 右代入は矢印で止まるので、そのうしろに書いたパターンは読まれない。
    ///
    /// 実測: `[1] => [*, 1, *]` → 1:5 tASSOC の 1 件のみ
    #[test]
    fn a_rightward_assignment_hides_the_pattern_written_after_it() {
        at_2_7(
            "[1] => [*, 1, *]\n",
            vec![Annotation::new(
                1,
                5,
                2,
                format!("unexpected token tASSOC\n{HINT}"),
            )],
        )
        .run();
    }

    /// `case`/`in` の中で行き詰まると、それを閉じる `end` まで持っていかれるので、
    /// parser はファイルの残りを読む足場を失う。
    ///
    /// 実測: 2 つ目の `case`/`in` も、あとに書いた endless メソッドも報告されない
    #[test]
    fn a_case_expression_takes_the_rest_of_the_file_with_it() {
        at_2_7(
            "case x\nin [*, 1, *]\n  y\nend\ncase x\nin [*, 2, *]\n  z\nend\n",
            vec![unexpected(2, 11, 1, "tSTAR")],
        )
        .run();
        at_2_7(
            "case x\nin [*, 1, *]\n  y\nend\ndef c = 1\n",
            vec![unexpected(2, 11, 1, "tSTAR")],
        )
        .run();
    }

    /// 空白のあとに書いた `(` は引数の始まりとして読まれ、3.3 より前は文を 1 つと
    /// 改行 1 つしか入らない。`;` も 2 つ目の改行も 2 つ目の文も、そこで止まる。
    ///
    /// 実測: `p (;x)` → 1:4 tSEMI / `p (x;)` → 1:5 tSEMI /
    /// `assert_equal("x", defined? (;x))` → 1:29 tSEMI /
    /// `p (x\n\n)` → 2:1 tNL / `p (x\ny)` → 2:1 tIDENTIFIER
    #[test]
    fn statements_in_a_command_argument_parenthesis_need_ruby_3_3() {
        at_2_7("p (;x)\n", vec![unexpected(1, 4, 1, "tSEMI")]).run();
        at_2_7("p (x;)\n", vec![unexpected(1, 5, 1, "tSEMI")]).run();
        at_2_7(
            "assert_equal(\"x\", defined? (;x))\n",
            vec![unexpected(1, 29, 1, "tSEMI")],
        )
        .run();
        at_2_7("p (x\ny)\n", vec![unexpected(2, 1, 1, "tIDENTIFIER")]).run();
        accepted("p (;x)\n", "3.3").run();
    }

    /// 文のうしろに置ける改行は 1 つだけで、2 つ目がその位置になる。行をまたぐ
    /// レンジなので、注記ではなく `locations` と `lengths` で固定する。
    ///
    /// 実測: `p (x\n\n)` → 2:1-3:1 (1 文字) tNL
    #[test]
    fn only_one_newline_fits_before_the_closing_parenthesis() {
        at_2_7(
            "p (x\n\n)\n",
            vec![Annotation::new(
                2,
                1,
                0,
                format!("unexpected token tNL\n{HINT}"),
            )],
        )
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .run();
    }

    /// 引数の始まりではない `(` は、どの版でも文をいくつでも受ける。ヒアドキュメント
    /// の本体は、それを開いたトークンのものであって文ではない。
    #[test]
    fn a_parenthesis_the_lexer_reads_as_an_expression_takes_a_whole_body() {
        accepted(
            "p (x)\np (\n  y\n)\np ()\na = (;x)\nfoo bar, (;x)\nreturn (;x)\n[(;x)]\n",
            "2.7",
        )
        .run();
        accepted("p (<<~E\n  a\nE\n)\n", "2.7").run();
    }

    /// メソッド定義の中で行き詰まると、その定義は最後まで還元されない。本家は以降の
    /// `class` / `module` 定義をメソッド本体に書かれたものとして報告し続け、`class <<`
    /// は同じ検査を持たないので対象にならない。
    ///
    /// 実測: `class Bar` → 6:3 class definition in method body /
    /// `class << self` は報告されず、代わりに 8:4 $end
    #[test]
    fn losing_a_method_definition_blames_every_later_class_definition() {
        CopCase::new(
            "Lint/Syntax",
            "class Foo\n  def a\n    p (;x)\n  end\n\n  class Bar\n  end\nend\n",
            vec![
                unexpected(3, 8, 1, "tSEMI"),
                Annotation::new(6, 3, 5, format!("class definition in method body\n{HINT}")),
            ],
        )
        .target_ruby("2.7")
        .run();
        at_2_7(
            "class Foo\n  def a\n    p (;x)\n  end\n\n  class << self\n  end\nend\n",
            vec![
                unexpected(3, 8, 1, "tSEMI"),
                Annotation::new(8, 4, 0, format!("unexpected token $end\n{HINT}")),
            ],
        )
        .locations(&[(3, 8, 3, 8), (8, 4, 9, 1)])
        .lengths(&[1, 1])
        .run();
    }

    /// 最後に読んだものが `class` / `module` 定義でなければ、ファイルはキーワードを
    /// 1 つ欠いたまま終わる。
    ///
    /// 実測: `x = 1` で終わる → 7:4 $end
    #[test]
    fn a_file_that_does_not_end_on_a_definition_runs_out_of_input() {
        at_2_7(
            "class Foo\n  def a\n    p (;x)\n  end\n\n  x = 1\nend\n",
            vec![
                unexpected(3, 8, 1, "tSEMI"),
                Annotation::new(7, 4, 0, format!("unexpected token $end\n{HINT}")),
            ],
        )
        .locations(&[(3, 8, 3, 8), (7, 4, 8, 1)])
        .lengths(&[1, 1])
        .run();
    }

    /// `Lint/Syntax` はどのディレクティブでも止められない。本家は `DirectiveComment`
    /// の cop 一覧からこの cop を必ず落とすので、名前でも部門でも `all` でも消えない。
    ///
    /// 実測: `# rubocop:disable Lint/Syntax` を書いたファイルでも 2:4 tEQL が出る
    #[test]
    fn a_directive_cannot_turn_off_the_report_that_a_file_does_not_parse() {
        at_2_7(
            "# rubocop:disable Lint/Syntax\n1+1=2\n# rubocop:enable Lint/Syntax\n",
            vec![unexpected(2, 4, 1, "tEQL")],
        )
        .run();
        at_2_7(
            "# rubocop:disable all\n1+1=2\n",
            vec![unexpected(2, 4, 1, "tEQL")],
        )
        .run();
        at_2_7(
            "# rubocop:disable Lint\n1+1=2\n",
            vec![unexpected(2, 4, 1, "tEQL")],
        )
        .run();
    }
}

/// `Lint/UnusedMethodArgument`。本家は `UnusedArgument` mixin と `VariableForce` の上に
/// 乗る。ここで固定するのは 2 つ: メッセージの継ぎ足し 3 通り (下線の助言はキーワード
/// 引数には付かない、引数が 1 つも読まれていなければ `名前(*)` の案内が付く) と、
/// 「読んだことにする」例外 (`raise NotImplementedError` / `yield` / 引数無し `super`)。
///
/// 期待値はすべて本家 1.89.0 の `--only Lint/UnusedMethodArgument --format json` 実測。
mod unused_method_argument {
    use super::*;

    const COP: &str = "Lint/UnusedMethodArgument";

    /// 読まれている引数が 1 つでもあれば `名前(*)` の案内は付かない。
    #[test]
    fn an_argument_nothing_reads_is_reported_beside_one_that_is_read() {
        expect_offense(
            COP,
            r#"
            def m(used, unused, _skipped)
                        ^^^^^^ Unused method argument - `unused`. If it's necessary, use `_` or `_unused` as an argument name to indicate that it won't be used. If it's unnecessary, remove it.
              used
            end
            "#,
        );
    }

    /// どの引数も読まれていなければ `m(*)` の案内が付く。キーワード引数には下線の助言が
    /// 付かない -- 下線を足すとキーワードそのものが変わってしまうため。
    #[test]
    fn a_keyword_argument_is_not_told_to_take_an_underscore() {
        expect_offense(
            COP,
            r#"
            def m(a, b: 1)
                  ^ Unused method argument - `a`. If it's necessary, use `_` or `_a` as an argument name to indicate that it won't be used. If it's unnecessary, remove it. You can also write as `m(*)` if you want the method to accept any arguments but don't care about them.
                     ^ Unused method argument - `b`. You can also write as `m(*)` if you want the method to accept any arguments but don't care about them.
              1
            end
            "#,
        );
    }

    /// 同じ理由でキーワード引数は autocorrect できない。
    #[test]
    fn a_keyword_argument_offense_is_not_correctable() {
        CopCase::annotated(
            COP,
            r#"
            def m(b: 1)
                  ^ Unused method argument - `b`. [...]
              1
            end
            "#,
        )
        .correctable(false)
        .run();
    }

    /// `IgnoreNotImplementedMethods` の既定。本体が `raise NotImplementedError` 1 文だけ
    /// なら引数は署名のためのもの。行末コメントは文ではないので、これがあっても 1 文。
    #[test]
    fn a_method_that_only_announces_it_is_unimplemented_keeps_its_arguments() {
        expect_no_offenses(
            COP,
            "def m(x)\n  raise NotImplementedError # not yet\nend\n",
        );
    }

    /// `fail` は引数を取らなくても同じ扱い。`NotImplementedExceptions` に無い例外を
    /// 上げるだけのメソッドは対象のまま。
    #[test]
    fn a_bare_fail_counts_but_another_exception_class_does_not() {
        expect_no_offenses(COP, "def m(x)\n  fail\nend\n");
        expect_offense(
            COP,
            r#"
            def o(x)
                  ^ Unused method argument - `x`. [...]
              raise ArgumentError
            end
            "#,
        );
    }

    /// 明示した `&block` を `yield` で呼ぶメソッドは、その引数を名前で読まなくても
    /// 使っている。
    #[test]
    fn a_block_argument_reached_through_yield_is_used() {
        expect_no_offenses(COP, "def m(&block)\n  yield\nend\n");
        expect_offense(
            COP,
            r#"
            def n(&block)
                   ^^^^^ Unused method argument - `block`. [...]
              1
            end
            "#,
        );
    }

    /// 引数無しの `super` はメソッドの引数をそのまま渡すので全部読む。ブロック付きで
    /// 書いても同じ (本家は `zsuper` にブロックが乗った形として読む) が、`super()` は
    /// 空の引数リストを渡す別物。
    #[test]
    fn a_zero_arity_super_reads_every_argument_even_with_a_block() {
        expect_no_offenses(
            COP,
            "class A\n  def m(a, b)\n    super do |x|\n      x\n    end\n  end\nend\n",
        );
        expect_offense(
            COP,
            r#"
            class A
              def n(c)
                    ^ Unused method argument - `c`. [...]
                super()
              end
            end
            "#,
        );
    }

    /// `IgnoreEmptyMethods` の既定。本体が無いメソッドは署名だけを書いたもの。
    #[test]
    fn an_empty_method_keeps_its_arguments() {
        expect_no_offenses(COP, "def m(a, b)\nend\n");
    }

    /// autocorrect は名前に `_` を足す。ブロック引数だけは足すのではなく消す -- 読まれない
    /// `&blk` は名前が悪いのではなく余分だから。
    #[test]
    fn correction_prefixes_an_underscore_but_deletes_a_block_argument() {
        expect_correction(
            COP,
            "def m(x, *rest, &blk)\n  1\nend\n",
            "def m(_x, *_rest)\n  1\nend\n",
        );
    }
}

/// `Lint/AssignmentInCondition`。本家は条件式を前順に歩き、`send` に当たったところで
/// 子を打ち切る。ここで固定するのは、その打ち切りが起きる形と、括弧付き代入
/// (`AllowSafeAssignment` の既定) の見逃し、そして tree-sitter が本家と違う木を作る
/// 3 箇所 -- `defined?(x = 1)` の括弧、`a[i] =~ re`、`/re/ =~ x = y`。
///
/// 期待値はすべて本家 1.89.0 の `--only Lint/AssignmentInCondition --format json` 実測。
mod assignment_in_condition {
    use super::*;

    const COP: &str = "Lint/AssignmentInCondition";
    const MSG: &str = "Use `==` if you meant to do a comparison or wrap the expression in \
                       parentheses to indicate you meant to assign in a condition.";

    /// offense は代入式全体ではなく `=` 1 文字に付く。
    #[test]
    fn the_offense_covers_the_equals_sign_only() {
        CopCase::annotated_with(COP, "if x = 1\n     ^ %{msg}\n  1\nend\n", &[("msg", MSG)]).run();
    }

    /// `if` / `unless` / `while` / `until` と後置形、`elsif`、三項演算子まで同じ扱い。
    /// ただし `begin ... end while` は本家では `while_post` で、この cop は見ない。
    #[test]
    fn every_conditional_form_is_inspected_except_a_post_condition_loop() {
        expect_offense(
            COP,
            r#"
            a = 1 if b = 2
                       ^ Use `==` [...]
            puts 1 while c = 3
                           ^ Use `==` [...]
            d = 4 unless e = 5
                           ^ Use `==` [...]
            "#,
        );
        expect_offense(
            COP,
            r#"
            if aa = 1
                  ^ Use `==` [...]
              1
            elsif bb = 2
                     ^ Use `==` [...]
              1
            end
            "#,
        );
        expect_no_offenses(COP, "begin\n  1\nend while xx = 1\n");
    }

    /// 括弧で囲んだ代入は「分かって書いている」の印なので既定では見逃す。中身が 2 文
    /// あれば代入の値は捨てられるので、これも条件ではない。
    #[test]
    fn parentheses_around_the_assignment_excuse_it() {
        expect_no_offenses(COP, "if (x = 1)\n  1\nend\n");
        expect_no_offenses(COP, "if (s = 1; t)\n  1\nend\n");
        expect_no_offenses(COP, "if \"#{cc = 1}\"\n  1\nend\n");
    }

    /// メソッド呼び出しに当たったら、その引数の中は条件ではないので歩かない。
    /// ブロックも同じ。一方 `super` と `yield` は呼び出しではないので歩く。
    #[test]
    fn the_walk_stops_at_a_call_but_not_at_super() {
        expect_no_offenses(COP, "if foo(i = 1)\n  1\nend\n");
        expect_no_offenses(COP, "if foo { |q| u = 1 }\n  1\nend\n");
        expect_no_offenses(COP, "if ->(q) { hh = 1 }\n  1\nend\n");
        expect_offense(
            COP,
            r#"
            if super(ii = 1)
                        ^ Use `==` [...]
              1
            end
            "#,
        );
    }

    /// セッター呼び出しと添字代入も本家では代入メソッドの `send`。添字の中は歩くが、
    /// 値として読むだけの `a[i]` は歩かない。
    #[test]
    fn a_setter_and_a_subscript_assignment_are_assignments() {
        expect_offense(
            COP,
            r#"
            if self.j = 1
                      ^ Use `==` [...]
              1
            end
            "#,
        );
        expect_offense(
            COP,
            r#"
            if k2[kk = 1] = 2
                     ^ Use `==` [...]
                          ^ Use `==` [...]
              1
            end
            "#,
        );
        expect_no_offenses(COP, "if a[jj = 1]\n  1\nend\n");
    }

    /// `||=` の類は本家の対象型に入っていない。
    #[test]
    fn a_shorthand_assignment_is_not_reported() {
        expect_no_offenses(COP, "if r ||= 1\n  1\nend\n");
    }

    /// `defined?(x = 1)` の括弧は演算子のもので、本家の木には括弧のノードが無い。
    /// 空白を空ければただの括弧に戻る。
    #[test]
    fn the_parentheses_of_defined_belong_to_the_operator() {
        expect_offense(
            COP,
            r#"
            if defined?(ee = 1)
                           ^ Use `==` [...]
              1
            end
            "#,
        );
        expect_no_offenses(COP, "if defined? (ff = 1)\n  1\nend\n");
        expect_no_offenses(COP, "if defined?((hh = 1))\n  1\nend\n");
    }

    /// tree-sitter は `a[i] =~ re` を「`~re` の代入」と読むが、Ruby は `=` に付けて
    /// 書かれた `~` を `=~` の後半と読む。逆に `/re/ =~ x = y` は本家では
    /// `match_with_lvasgn` で、右辺の代入は条件のまま。
    #[test]
    fn a_match_operator_is_not_an_assignment() {
        expect_no_offenses(COP, "if a['k'] =~ /re/\n  1\nend\n");
        expect_offense(
            COP,
            r#"
            if /re/ =~ commonmk = File.read("c")
                                ^ Use `==` [...]
              1
            end
            "#,
        );
    }

    /// autocorrect は代入を括弧で囲む。`SafeAutoCorrect: false` なので `-a` では
    /// 適用されない。
    #[test]
    fn correction_wraps_the_assignment_in_parentheses() {
        expect_correction(COP, "if x = 1\n  1\nend\n", "if (x = 1)\n  1\nend\n");
        CopCase::annotated(COP, "if x = 1\n     ^ Use `==` [...]\n  1\nend\n")
            .correct_mode(sonicop::engine::CorrectMode::Safe)
            .corrected("if x = 1\n  1\nend\n")
            .run();
    }
}
/// `Security` 部門の残り 4 cop。期待値は本家 1.89.0 の `--only <cop> --format json` 実測。
mod security_load_and_open {
    use super::*;

    /// `(send (const {nil? cbase} :Marshal) {:load :restore} !(send ... :dump ...) _?)`。
    /// `::` 越しの呼び出しも本家 AST では同じ `send`。
    #[test]
    fn marshal_load_flags_loading_from_an_untrusted_payload() {
        expect_offense(
            "Security/MarshalLoad",
            r#"
            Marshal.load("{}")
                    ^^^^ Avoid using `Marshal.load`.
            Marshal.restore("{}")
                    ^^^^^^^ Avoid using `Marshal.restore`.
            ::Marshal.load(x)
                      ^^^^ Avoid using `Marshal.load`.
            Marshal::load(x)
                     ^^^^ Avoid using `Marshal.load`.
            Marshal.load(x, proc)
                    ^^^^ Avoid using `Marshal.load`.
            "#,
        );
    }

    /// 引数 0 個と 3 個はパターンに合わず、`Marshal.dump` を読み直す deep copy と
    /// safe navigation (`csend`) も対象外。`Foo::Marshal` は別の定数。
    #[test]
    fn marshal_load_accepts_the_shapes_the_pattern_excludes() {
        expect_no_offenses(
            "Security/MarshalLoad",
            "Marshal.load(Marshal.dump({}))\n\
             Marshal.load\n\
             Marshal.load(a, b, c)\n\
             Marshal&.load(x)\n\
             Marshal.dump(\"{}\")\n\
             Foo::Marshal.load(x)\n",
        );
    }

    #[test]
    fn marshal_load_reports_convention_severity_without_a_correction() {
        CopCase::new("Security/MarshalLoad", "Marshal.load(x)\n", Vec::new())
            .without_offense_check()
            .severity(Severity::Convention)
            .correctable(false)
            .run();
    }

    #[test]
    fn json_load_flags_the_deserializing_methods() {
        expect_offense(
            "Security/JSONLoad",
            r#"
            JSON.load('{}')
                 ^^^^ Prefer `JSON.parse` over `JSON.load`.
            JSON.restore('{}')
                 ^^^^^^^ Prefer `JSON.parse` over `JSON.restore`.
            ::JSON.load(x)
                   ^^^^ Prefer `JSON.parse` over `JSON.load`.
            JSON.load('{}', proc)
                 ^^^^ Prefer `JSON.parse` over `JSON.load`.
            "#,
        );
    }

    /// `!`(pair (sym :create_additions) _)` は最後の引数の**部分木**を探すので、
    /// `merge` の中に書いても除外される。引数が 1 つも無い呼び出しは対象外。
    #[test]
    fn json_load_accepts_an_explicit_create_additions() {
        expect_no_offenses(
            "Security/JSONLoad",
            "JSON.load('{}', create_additions: true)\n\
             JSON.load('{}', create_additions: false)\n\
             JSON.load('{}', opts.merge(create_additions: true))\n\
             JSON.load\n\
             JSON.parse('{}')\n",
        );
    }

    /// `SafeAutoCorrect: false` なので `-a` では書き換わらない。
    #[test]
    fn json_load_corrects_the_selector_only_when_unsafe_corrections_are_allowed() {
        expect_correction(
            "Security/JSONLoad",
            "JSON.load('{}')\nJSON.restore(x)\n",
            "JSON.parse('{}')\nJSON.parse(x)\n",
        );
        CopCase::new("Security/JSONLoad", "JSON.load('{}')\n", Vec::new())
            .without_offense_check()
            .correct_mode(sonicop::engine::CorrectMode::Safe)
            .corrected("JSON.load('{}')\n")
            .run();
    }

    #[test]
    fn open_flags_dynamic_and_piped_arguments() {
        expect_offense(
            "Security/Open",
            r##"
            open(something)
            ^^^^ The use of `Kernel#open` is a serious security risk.
            open("| #{something}")
            ^^^^ The use of `Kernel#open` is a serious security risk.
            open("| foo")
            ^^^^ The use of `Kernel#open` is a serious security risk.
            open("")
            ^^^^ The use of `Kernel#open` is a serious security risk.
            open("#{x}")
            ^^^^ The use of `Kernel#open` is a serious security risk.
            open(x, "r")
            ^^^^ The use of `Kernel#open` is a serious security risk.
            URI.open(something)
                ^^^^ The use of `URI.open` is a serious security risk.
            ::URI.open(y)
                  ^^^^ The use of `::URI.open` is a serious security risk.
            "##,
        );
    }

    /// `__FILE__` は本家のパーサが解析時に `str` へ畳むので、リテラル引数と同じ扱い。
    /// `"a" + b` は先頭のリテラルで判定され、`?a` も 1 文字の `str`。
    #[test]
    fn open_accepts_a_literal_path() {
        expect_no_offenses(
            "Security/Open",
            "open(\"foo.text\")\n\
             open(\"a\" + b)\n\
             open(?a)\n\
             open(__FILE__)\n\
             URI.open(\"http://example.com\")\n\
             File.open(something)\n\
             open\n",
        );
    }

    /// heredoc は本文が補間を含まなければ `str` なので、`str_content` の先頭で決まる。
    /// `<<~` の字下げは落としてから見る。
    #[test]
    fn open_judges_a_heredoc_by_its_dedented_body() {
        expect_offense(
            "Security/Open",
            r#"
            open(<<~SAFE)
              hi
            SAFE
            open(<<~PIPED)
            ^^^^ The use of `Kernel#open` is a serious security risk.
              | hi
            PIPED
            "#,
        );
    }

    #[test]
    fn yaml_load_flags_the_unsafe_loader() {
        expect_offense(
            "Security/YAMLLoad",
            r#"
            YAML.load("x")
                 ^^^^ Prefer using `YAML.safe_load` over `YAML.load`.
            ::YAML.load(x)
                   ^^^^ Prefer using `YAML.safe_load` over `YAML.load`.
            YAML.load
                 ^^^^ Prefer using `YAML.safe_load` over `YAML.load`.
            YAML.safe_load(x)
            "#,
        );
        expect_correction(
            "Security/YAMLLoad",
            "YAML.load('x')\n",
            "YAML.safe_load('x')\n",
        );
    }

    /// `maximum_target_ruby_version 3.0`。Psych 4 を積む Ruby 3.1 以降では cop ごと退場する。
    #[test]
    fn yaml_load_stops_at_ruby_3_1() {
        CopCase::new("Security/YAMLLoad", "YAML.load('x')\n", Vec::new())
            .target_ruby("3.1")
            .run();
    }
}

/// `Gemspec` 部門。`Include: ['**/*.gemspec']` があるので、対象は gemspec ファイルだけ。
mod gemspec_department {
    use super::*;

    const GEMSPEC: &str = "example.gemspec";

    #[test]
    fn ordered_dependencies_flags_a_pair_out_of_order() {
        CopCase::annotated(
            "Gemspec/OrderedDependencies",
            r#"
            Gem::Specification.new do |spec|
              spec.add_dependency 'rubocop'
              spec.add_dependency 'rspec'
              ^^^^^^^^^^^^^^^^^^^^^^^^^^^ Dependencies should be sorted in an alphabetical order within their section of the gemspec. Dependency `rspec` should appear before `rubocop`.
            end
            "#,
        )
        .path(GEMSPEC)
        .corrected(
            "Gem::Specification.new do |spec|\n  spec.add_dependency 'rspec'\n  spec.add_dependency 'rubocop'\nend\n",
        )
        .run();
    }

    /// 空行は節を分け、宣言の方法が違えば別の節。既定では行コメントも節を分ける。
    #[test]
    fn ordered_dependencies_accepts_separate_sections() {
        CopCase::new(
            "Gemspec/OrderedDependencies",
            "Gem::Specification.new do |spec|\n  spec.add_dependency 'rubocop'\n\n  spec.add_dependency 'rspec'\n  spec.add_development_dependency 'a'\nend\n",
            Vec::new(),
        )
        .path(GEMSPEC)
        .run();
        CopCase::new(
            "Gemspec/OrderedDependencies",
            "Gem::Specification.new do |spec|\n  # quality\n  spec.add_dependency 'rubocop'\n  # tests\n  spec.add_dependency 'rspec'\nend\n",
            Vec::new(),
        )
        .path(GEMSPEC)
        .run();
    }

    /// `gem_specification` を `def_node_search` で呼ぶ本家の条件は enumerator なので常に真。
    /// `Gem::Specification.new` ブロックが無い gemspec でも `RUBY_VERSION` は報告される。
    #[test]
    fn ruby_version_globals_usage_flags_every_spelling_of_the_constant() {
        CopCase::annotated(
            "Gemspec/RubyVersionGlobalsUsage",
            r#"
            RUBY_VERSION
            ^^^^^^^^^^^^ Do not use `RUBY_VERSION` in gemspec file.
            Ruby::VERSION
            ^^^^^^^^^^^^^ Do not use `Ruby::VERSION` in gemspec file.
            ::RUBY_VERSION
            ^^^^^^^^^^^^^^ Do not use `::RUBY_VERSION` in gemspec file.
            ::Ruby::VERSION
            ^^^^^^^^^^^^^^^ Do not use `::Ruby::VERSION` in gemspec file.
            Foo::RUBY_VERSION
            "#,
        )
        .path(GEMSPEC)
        .severity(Severity::Warning)
        .run();
    }

    /// `add_global_offense` はファイル先頭の長さ 0 のレンジで報告される。
    #[test]
    fn required_ruby_version_reports_a_missing_declaration_at_the_head_of_the_file() {
        CopCase::annotated(
            "Gemspec/RequiredRubyVersion",
            r#"
            Gem::Specification.new do |spec|
            ^{} `required_ruby_version` should be specified.
              spec.name = 'x'
            end
            "#,
        )
        .path(GEMSPEC)
        .severity(Severity::Warning)
        .correctable(false)
        .run();
    }

    /// `extract_ruby_version` は `[>=]` を含む最初の要求から数字を 2 つ拾う。変数や
    /// 引数無しの呼び出しを含む値は動的とみなして見送る。
    #[test]
    fn required_ruby_version_compares_the_declared_version_with_the_target() {
        CopCase::annotated(
            "Gemspec/RequiredRubyVersion",
            r#"
            Gem::Specification.new do |spec|
              spec.required_ruby_version = '>= 2.7.0'
              spec.required_ruby_version = '>= 2.4.0'
                                           ^^^^^^^^^^ `required_ruby_version` and `TargetRubyVersion` (2.7, which may be specified in .rubocop.yml) should be equal.
              spec.required_ruby_version = ['>= 2.4.0', '< 3.0']
                                           ^^^^^^^^^^^^^^^^^^^^^ `required_ruby_version` and `TargetRubyVersion` (2.7, which may be specified in .rubocop.yml) should be equal.
              spec.required_ruby_version = Gem::Requirement.new('>= 2.4')
                                           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ `required_ruby_version` and `TargetRubyVersion` (2.7, which may be specified in .rubocop.yml) should be equal.
              spec.required_ruby_version = version
              spec.required_ruby_version = @version
              spec.required_ruby_version = ''
                                           ^^ `required_ruby_version` and `TargetRubyVersion` (2.7, which may be specified in .rubocop.yml) should be equal.
            end
            "#,
        )
        .path(GEMSPEC)
        .run();
    }

    /// `spec.requirements <<` のような追記は重複ではない。添字代入は鍵の**値**で
    /// まとめられるので、引用符の違いは同じ代入になる。
    #[test]
    fn duplicated_assignment_flags_a_repeated_attribute() {
        CopCase::annotated(
            "Gemspec/DuplicatedAssignment",
            r#"
            Gem::Specification.new do |spec|
              spec.name = 'x'
              spec.name = 'y'
              ^^^^^^^^^^^^^^^ `name=` method calls already given on line 2 of the gemspec.
              spec.metadata["k"] = 'v'
              spec.metadata['k'] = 'w'
              ^^^^^^^^^^^^^^^^^^^^^^^^ `metadata['k']=` method calls already given on line 4 of the gemspec.
              spec.requirements << 'a'
              spec.requirements << 'b'
            end
            "#,
        )
        .path(GEMSPEC)
        .severity(Severity::Warning)
        .run();
    }

    /// 報告されるのは「1 行目のカラムから **最終行**の終端カラムまで」なので、
    /// 複数行の代入では 1 行目の途中で切れる。
    #[test]
    fn duplicated_assignment_reports_the_first_line_cut_at_the_last_column() {
        CopCase::annotated(
            "Gemspec/DuplicatedAssignment",
            r#"
            Gem::Specification.new do |spec|
              spec.metadata = {
                'a' => 1
              }
              spec.metadata = {
              ^ `metadata=` method calls already given on line 2 of the gemspec.
                'b' => 2
              }
            end
            "#,
        )
        .path(GEMSPEC)
        .run();
    }

    /// 受け手が仕様ブロックの引数名でなければ代入は仕様の属性ではない。ブロックが
    /// 無いファイルでは `_1` と `it` 以外どの名前も一致しない。
    #[test]
    fn duplicated_assignment_needs_the_specification_block_variable() {
        CopCase::new(
            "Gemspec/DuplicatedAssignment",
            "spec.name = 'x'\nspec.name = 'y'\n",
            Vec::new(),
        )
        .path(GEMSPEC)
        .run();
    }
}

/// `Bundler` 部門。`Include` は `**/Gemfile` などで、Gemfile 以外には効かない。
mod bundler_department {
    use super::*;

    const GEMFILE: &str = "Gemfile";

    #[test]
    fn ordered_gems_flags_each_pair_out_of_order() {
        CopCase::annotated(
            "Bundler/OrderedGems",
            r#"
            gem 'rubocop'
            gem 'rspec'
            ^^^^^^^^^^^ Gems should be sorted in an alphabetical order within their section of the Gemfile. Gem `rspec` should appear before `rubocop`.
            gem 'a2'
            ^^^^^^^^ Gems should be sorted in an alphabetical order within their section of the Gemfile. Gem `a2` should appear before `rspec`.
            "#,
        )
        .path(GEMFILE)
        .corrected("gem 'a2'\ngem 'rspec'\ngem 'rubocop'\n")
        .run();
    }

    #[test]
    fn ordered_gems_accepts_sorted_and_separated_declarations() {
        CopCase::new(
            "Bundler/OrderedGems",
            "gem 'rspec'\ngem 'rubocop'\n\ngem 'a'\n",
            Vec::new(),
        )
        .path(GEMFILE)
        .run();
    }

    /// `TreatCommentsAsGroupSeparators: false` では宣言の頭がその上のコメントまで
    /// 伸びるので、コメントごと入れ替わる。
    #[test]
    fn ordered_gems_moves_the_comment_with_the_declaration() {
        CopCase::annotated(
            "Bundler/OrderedGems",
            r#"
            # quality
            gem 'rubocop'
            # tests
            gem 'rspec'
            ^^^^^^^^^^^ Gems should be sorted in an alphabetical order within their section of the Gemfile. Gem `rspec` should appear before `rubocop`.
            "#,
        )
        .path(GEMFILE)
        .config("Bundler/OrderedGems:\n  TreatCommentsAsGroupSeparators: false\n")
        .corrected("# tests\ngem 'rspec'\n# quality\ngem 'rubocop'\n")
        .run();
    }

    /// 既定の `AllowHttpProtocol: true` では `http://rubygems.org` は見送られ、
    /// 記号の source だけが報告される。
    #[test]
    fn insecure_protocol_source_flags_the_deprecated_symbols() {
        CopCase::annotated(
            "Bundler/InsecureProtocolSource",
            r#"
            source :rubygems
                   ^^^^^^^^^ The source `:rubygems` is deprecated because HTTP requests are insecure. Please change your source to 'https://rubygems.org' if possible, or 'http://rubygems.org' if not.
            source :gemcutter
                   ^^^^^^^^^^ The source `:gemcutter` is deprecated because HTTP requests are insecure. Please change your source to 'https://rubygems.org' if possible, or 'http://rubygems.org' if not.
            source :rubyforge
                   ^^^^^^^^^^ The source `:rubyforge` is deprecated because HTTP requests are insecure. Please change your source to 'https://rubygems.org' if possible, or 'http://rubygems.org' if not.
            source 'http://rubygems.org'
            source 'https://rubygems.org'
            "#,
        )
        .path(GEMFILE)
        .severity(Severity::Warning)
        .corrected(
            "source 'https://rubygems.org'\nsource 'https://rubygems.org'\nsource 'https://rubygems.org'\nsource 'http://rubygems.org'\nsource 'https://rubygems.org'\n",
        )
        .run();
    }

    #[test]
    fn insecure_protocol_source_flags_http_when_it_is_not_allowed() {
        CopCase::annotated(
            "Bundler/InsecureProtocolSource",
            r#"
            source 'http://rubygems.org'
                   ^^^^^^^^^^^^^^^^^^^^^ Use `https://rubygems.org` instead of `http://rubygems.org`.
            "#,
        )
        .path(GEMFILE)
        .config("Bundler/InsecureProtocolSource:\n  AllowHttpProtocol: false\n")
        .corrected("source 'https://rubygems.org'\n")
        .run();
    }

    /// `add_global_offense` なので位置はファイル先頭。メッセージには本家が検査前に
    /// 絶対化したパスがそのまま入る。
    #[test]
    fn gem_filename_reports_the_manifest_the_configuration_did_not_ask_for() {
        CopCase::annotated(
            "Bundler/GemFilename",
            "gem 'a'\n^{} `gems.rb` file was found but `Gemfile` is required (file path: /tmp/example/gems.rb).\n",
        )
        .path("/tmp/example/gems.rb")
        .severity(Severity::Convention)
        .correctable(false)
        .run();
        CopCase::new("Bundler/GemFilename", "gem 'a'\n", Vec::new())
            .path("/tmp/example/Gemfile")
            .run();
    }

    /// 同じ集合のグループは並び順が違っても同じグループ。
    #[test]
    fn duplicated_group_flags_a_group_declared_twice() {
        CopCase::annotated(
            "Bundler/DuplicatedGroup",
            r#"
            group :development do
              gem 'a'
            end

            group :development do
            ^^^^^^^^^^^^^^^^^^ Gem group `:development` already defined on line 1 of the Gemfile.
              gem 'b'
            end

            group :test, :development do
              gem 'c'
            end

            group :development, :test do
            ^^^^^^^^^^^^^^^^^^^^^^^^^ Gem group `:development, :test` already defined on line 9 of the Gemfile.
              gem 'd'
            end
            "#,
        )
        .path(GEMFILE)
        .severity(Severity::Warning)
        .run();
    }

    /// 囲む `platforms` / `source` / `git` / `path` が違えば別のグループ。
    #[test]
    fn duplicated_group_separates_groups_by_their_enclosing_source() {
        CopCase::new(
            "Bundler/DuplicatedGroup",
            "platforms :ruby do\n  group :default do\n    gem 'openssl'\n  end\nend\n\nplatforms :jruby do\n  group :default do\n    gem 'jruby-openssl'\n  end\nend\n",
            Vec::new(),
        )
        .path(GEMFILE)
        .run();
    }

    /// 宣言は本家 AST のノード同士の同値でまとめられるので、引用符の違いは同じ gem。
    #[test]
    fn duplicated_gem_flags_a_gem_declared_twice() {
        CopCase::annotated(
            "Bundler/DuplicatedGem",
            r#"
            gem 'a'
            gem "a"
            ^^^^^^^ Gem `a` requirements already given on line 1 of the Gemfile.
            "#,
        )
        .path(GEMFILE)
        .severity(Severity::Warning)
        .run();
    }

    /// 同じ条件分岐の各枝に 1 つずつ書かれた宣言は 1 回の宣言。`elsif` の連なりは
    /// 1 つの条件分岐として平らに見る。
    #[test]
    fn duplicated_gem_accepts_one_declaration_per_branch() {
        CopCase::new(
            "Bundler/DuplicatedGem",
            "if Dir.exist?(local)\n  gem 'rubocop', path: local\nelsif ENV['V'] == 'master'\n  gem 'rubocop', git: 'https://example.com/rubocop.git'\nelse\n  gem 'rubocop', '~> 0.90.0'\nend\n",
            Vec::new(),
        )
        .path(GEMFILE)
        .run();
    }
}

/// `Migration/DepartmentName`。部門名の無い cop 名を指すディレクティブを報告する。
mod migration_department_name {
    use super::*;

    const COP: &str = "Migration/DepartmentName";

    /// 走査は `/[^,]+|\W+/` なので、コンマ区切りの各名前と区切り自体が交互に届く。
    /// 部門を 1 つに決められない名前 (`Foo` / `Bar`) は correction が付かない。
    #[test]
    fn flags_a_cop_name_without_its_department() {
        CopCase::annotated(
            COP,
            r#"
            # rubocop:disable AbcSize
                              ^^^^^^^ Department name is missing.
            # rubocop:enable Metrics/AbcSize, AbcSize
                                              ^^^^^^^ Department name is missing.
            # rubocop:todo Foo,Bar
                           ^^^ Department name is missing.
                               ^^^ Department name is missing.
            # rubocop:disable Syntax
                              ^^^^^^ Department name is missing.
            "#,
        )
        .corrected(
            "# rubocop:disable Metrics/AbcSize\n# rubocop:enable Metrics/AbcSize, Metrics/AbcSize\n# rubocop:todo Foo,Bar\n# rubocop:disable Lint/Syntax\n",
        )
        .run();
    }

    /// `valid_content_token?` の `/\W+/` は部分一致なので、非単語文字を含む字面は
    /// すべて素通しする。`all` も、部門名も同じ。
    #[test]
    fn accepts_a_qualified_name_a_department_and_all() {
        expect_no_offenses(
            COP,
            "# rubocop:disable Metrics/AbcSize\n\
             # rubocop:disable Metrics\n\
             # rubocop:enable AbcSize -- reason\n\
             # a plain comment\n",
        );
        expect_no_offenses(COP, "# rubocop:disable all\n");
    }
}

/// cop 単位の `Include`。`AllCops/Include` が選んだファイルのうち、その cop 自身の
/// `Include` が届くものだけを検査する本家 `Cop::Base#relevant_file?` の仕組み。
mod cop_level_include {
    use super::*;

    #[test]
    fn a_bundler_cop_stays_off_an_ordinary_ruby_file() {
        CopCase::new(
            "Bundler/OrderedGems",
            "gem 'rubocop'\ngem 'rspec'\n",
            Vec::new(),
        )
        .run();
        CopCase::new(
            "Gemspec/OrderedDependencies",
            "Gem::Specification.new do |spec|\n  spec.add_dependency 'rubocop'\n  spec.add_dependency 'rspec'\nend\n",
            Vec::new(),
        )
        .run();
    }

    /// `Migration/DepartmentName` は `Include` を持たないので、対象のファイルすべてに効く。
    #[test]
    fn a_cop_without_an_include_applies_to_every_target() {
        CopCase::annotated(
            "Migration/DepartmentName",
            r#"
            # rubocop:disable AbcSize
                              ^^^^^^^ Department name is missing.
            "#,
        )
        .path("Gemfile")
        .run();
    }
}

/// `Lint/AmbiguousBlockAssociation`。本家は 2 つの入口を持つ -- `{}` ブロックが引数側の
/// 呼び出しに付く形 (`on_send`) と、`do` ブロックが外側に付いてしまう形 (`on_block`)。
/// tree-sitter はブロックを呼び出しの子に置くので、本家が 1 段上に持つブロックノードと
/// 「呼び出しの範囲」がずれる。ここではそのずれと、`foo (a)` の括弧が引数リストでは
/// ないことを固定する。
///
/// 期待値はすべて本家 1.89.0 の `--only Lint/AmbiguousBlockAssociation --format json` 実測。
mod ambiguous_block_association {
    use super::*;

    const COP: &str = "Lint/AmbiguousBlockAssociation";

    /// offense は外側の呼び出し全体に付き、メッセージはブロック引数とその呼び出しを
    /// それぞれの原文で名指しする。
    #[test]
    fn the_offense_covers_the_whole_outer_call() {
        expect_offense(
            COP,
            r#"
            some_method a { |val| puts val }
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Parenthesize the param `a { |val| puts val }` to make sure that the block will be associated with the `a` method call.
            "#,
        );
    }

    /// 括弧を付ければ曖昧でなくなる。ラムダと proc、演算子メソッド、代入も対象外。
    #[test]
    fn parentheses_a_lambda_and_an_operator_are_all_unambiguous() {
        expect_no_offenses(COP, "some_method(a { |val| puts val })\n");
        expect_no_offenses(COP, "some_method(a) { |val| puts val }\n");
        expect_no_offenses(COP, "foo == bar { |b| b.baz }\n");
        expect_no_offenses(COP, "foo = ->(bar) { bar.baz }\n");
        expect_no_offenses(COP, "foo lambda { |x| x }\n");
        expect_no_offenses(COP, "foo Proc.new { |x| x }\n");
        expect_no_offenses(COP, "self.foo = a { |x| x }\n");
        expect_no_offenses(COP, "foo[a { |x| x }]\n");
    }

    /// 内側の呼び出しが引数を取っていれば、ブロックはそちらのものにしかなり得ない。
    #[test]
    fn an_inner_call_with_arguments_is_not_ambiguous() {
        expect_no_offenses(COP, "foo a(1) { |x| x }\n");
    }

    /// `foo (a)` の括弧は引数リストではなく式のグループ。本家では
    /// `parenthesized?` が偽なので、この形も報告される。
    #[test]
    fn a_space_before_the_parenthesis_makes_it_a_grouped_expression() {
        expect_offense(
            COP,
            r#"
            run_without_aborting (ADAPTERS - ["test"]).map { |a| a }
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Parenthesize the param `(ADAPTERS - ["test"]).map { |a| a }` to make sure that the block will be associated with the `(ADAPTERS - ["test"]).map` method call.
            "#,
        );
    }

    /// `do` ブロックは外側に付くので、引数の末尾にある列挙メソッドはブロック無しで
    /// 呼ばれる。offense はその内側の呼び出しに付き、autocorrect は無い。
    #[test]
    fn a_do_block_binds_to_the_outer_call() {
        expect_offense(
            COP,
            r#"
            render json: data.map do |item|
                         ^^^^^^^^ `map` is called without a block because the `do` block binds to `render`. Use braces or extract to a variable.
              item.to_h
            end
            "#,
        );
        expect_no_offenses(COP, "render json: data.map { |item| item.to_h }\n");
    }

    /// 候補は「引数の終わりで終わる呼び出し」だけ。別の呼び出しに繋がれたもの、
    /// 括弧の中に埋まったもの、引数を取るものは候補ではない。
    #[test]
    fn only_the_call_the_block_could_have_attached_to_is_a_candidate() {
        expect_no_offenses(COP, "foo bar.map.to_a do |x|\n  x\nend\n");
        expect_no_offenses(COP, "foo bar(baz.map) do |x|\n  x\nend\n");
        expect_no_offenses(COP, "foo bar.each_with_object({}) do |x, h|\n  h\nend\n");
        expect_no_offenses(COP, "super bar.map do |x|\n  x\nend\n");
    }

    /// autocorrect は引数を括弧で囲む。メソッド名と引数のあいだの空白は落ちる。
    #[test]
    fn correction_wraps_the_arguments_in_parentheses() {
        expect_correction(
            COP,
            "some_method a { |val| puts val }\n",
            "some_method(a { |val| puts val })\n",
        );
        expect_correction(
            COP,
            "run_without_aborting (ADAPTERS - [\"test\"]).map { |a| a }\n",
            "run_without_aborting((ADAPTERS - [\"test\"]).map { |a| a })\n",
        );
    }
}

/// Naming 部門の残り 12 cop。期待値はすべて本家 1.89.0 の `--only <cop> --format json`
/// および `-A` の実出力から取っている。
mod naming_rest {
    use super::*;

    const ACCESSOR: &str = "Naming/AccessorMethodName";
    const BINARY_OPERATOR: &str = "Naming/BinaryOperatorParameterName";
    const BLOCK_PARAMETER: &str = "Naming/BlockParameterName";
    const CAMEL_CASE: &str = "Naming/ClassAndModuleCamelCase";
    const FILE_NAME: &str = "Naming/FileName";
    const HEREDOC_CASE: &str = "Naming/HeredocDelimiterCase";
    const HEREDOC_NAMING: &str = "Naming/HeredocDelimiterNaming";
    const MEMOIZED: &str = "Naming/MemoizedInstanceVariableName";
    const METHOD_PARAMETER: &str = "Naming/MethodParameterName";
    const PREDICATE_PREFIX: &str = "Naming/PredicatePrefix";
    const RESCUED: &str = "Naming/RescuedExceptionsVariableName";
    const VARIABLE_NUMBER: &str = "Naming/VariableNumber";

    /// `get_` は引数を取らないときだけ、`set_` は必須引数がちょうど 1 つのときだけ
    /// 報告される。`def get_foo()` は括弧が空なので引数無しの扱い。
    #[test]
    fn accessor_method_name_requires_the_arity_of_a_real_accessor() {
        expect_offense(
            ACCESSOR,
            r#"
            def get_value
                ^^^^^^^^^ Do not prefix reader method names with `get_`.
            end
            def set_value(value)
                ^^^^^^^^^ Do not prefix writer method names with `set_`.
            end
            def self.get_thing
                     ^^^^^^^^^ Do not prefix reader method names with `get_`.
            end
            "#,
        );
        for source in [
            "def get_value(attr)\nend\n",
            "def set_value\nend\n",
            "def set_value(a, b)\nend\n",
            "def set_value(*a)\nend\n",
            "def set_value(a = 1)\nend\n",
            "def get_value?\nend\n",
            "def get_value=(v)\nend\n",
        ] {
            expect_no_offenses(ACCESSOR, source);
        }
        // 空の括弧は引数無しと同じなので、こちらは報告される側。
        expect_offense(
            ACCESSOR,
            r#"
            def get_value()
                ^^^^^^^^^ Do not prefix reader method names with `get_`.
            end
            "#,
        );
    }

    /// `op_method?` は語頭が単語文字でない名前と `eql?` / `equal?` だけを演算子と
    /// みなし、`EXCLUDED` の 8 つは除く。`defs` には `on_defs` の別名が無い。
    #[test]
    fn binary_operator_parameter_name_covers_only_the_operators_upstream_lists() {
        expect_offense(
            BINARY_OPERATOR,
            r#"
            def +(amount)
                  ^^^^^^ When defining the `+` operator, name its argument `other`.
              amount + 1
            end
            def eql?(y)
                     ^ When defining the `eql?` operator, name its argument `other`.
              y
            end
            "#,
        );
        for source in [
            "def ==(other)\nend\n",
            "def ==(_other)\nend\n",
            "def <<(a)\nend\n",
            "def [](a)\nend\n",
            "def []=(a)\nend\n",
            "def ===(a)\nend\n",
            "def =~(a)\nend\n",
            "def -@\nend\n",
            "def self.+(a)\nend\n",
            "def +(a, b)\nend\n",
        ] {
            expect_no_offenses(BINARY_OPERATOR, source);
        }
    }

    /// autocorrect は引数と、それを読むローカル変数のすべてを `other` にする。
    #[test]
    fn binary_operator_parameter_name_renames_the_reads_as_well() {
        expect_correction(
            BINARY_OPERATOR,
            "def +(amount)\n  amount + 1\nend\n",
            "def +(other)\n  other + 1\nend\n",
        );
    }

    /// `AllowedNames` は名前から取り除かれてから `_` の有無を見るので、
    /// `module_parent::MyModule` は通り `module_parent::My_Module` は通らない。
    /// レンジは定数パス全体。
    #[test]
    fn class_and_module_camel_case_strips_the_allowed_names_first() {
        expect_offense(
            CAMEL_CASE,
            r#"
            class My_Class
                  ^^^^^^^^ Use CamelCase for classes and modules.
            end
            module My_Module
                   ^^^^^^^^^ Use CamelCase for classes and modules.
            end
            class module_parent::My_Module
                  ^^^^^^^^^^^^^^^^^^^^^^^^ Use CamelCase for classes and modules.
            end
            class Foo::Bar_Baz
                  ^^^^^^^^^^^^ Use CamelCase for classes and modules.
            end
            "#,
        );
        expect_no_offenses(CAMEL_CASE, "class module_parent::MyModule\nend\n");
        expect_no_offenses(CAMEL_CASE, "class MyClass\nend\n");
    }

    /// `UncommunicativeName` のレンジは引数の先頭から名前の文字数ぶん。`*` は 1、
    /// `**` は 2 足されるが `&` は足されないので、ブロック引数は名前の途中で切れる。
    /// 分解引数は名前ではなく S 式の長さで測られる。
    #[test]
    fn block_parameter_name_measures_the_range_from_the_sigil() {
        expect_offense(
            BLOCK_PARAMETER,
            r#"
            bar { |xA, y| xA }
                   ^^ Only use lowercase characters for block parameter.
            baz { |;shadowA| 1 }
                    ^^^^^^^ Only use lowercase characters for block parameter.
            qux { |*rB| 1 }
                   ^^^ Only use lowercase characters for block parameter.
            quux { |&bC| 1 }
                    ^^ Only use lowercase characters for block parameter.
            corge { |(dD, e)| 1 }
                     ^^^^^^^^^ Only use lowercase characters for block parameter.
            grault { |x, **kE| 1 }
                         ^^^^ Only use lowercase characters for block parameter.
            lam = ->(fF) { fF }
                     ^^ Only use lowercase characters for block parameter.
            "#,
        );
        // 番号付きブロック引数には `on_block` の handler が無い。
        expect_no_offenses(BLOCK_PARAMETER, "[1].map { _1 }\n");
        expect_no_offenses(BLOCK_PARAMETER, "bar { |x, y| x }\n");
    }

    /// 既定の `MinNameLength` は 3。`AllowedNames` に載る `id` / `io` は免れ、
    /// 先頭の `_` は名前の一部として数えないが、レンジの長さには含まれる。
    /// tree-sitter が `x = A, y = 2` を 1 つの多重代入として読む欠陥も、
    /// 本家どおり 2 つの省略可能引数に戻す。
    #[test]
    fn method_parameter_name_counts_every_parameter_form() {
        expect_offense(
            METHOD_PARAMETER,
            r#"
            def m(a, b = 1, *c, d:, e: 2, **f, &g)
                  ^ Method parameter must be at least 3 characters long.
                     ^ Method parameter must be at least 3 characters long.
                            ^^ Method parameter must be at least 3 characters long.
                                ^ Method parameter must be at least 3 characters long.
                                    ^ Method parameter must be at least 3 characters long.
                                          ^^^ Method parameter must be at least 3 characters long.
                                               ^ Method parameter must be at least 3 characters long.
            end
            def r(_a, __b, _, ab, abc, aB, x1, id, io)
                  ^^ Method parameter must be at least 3 characters long.
                      ^^^ Method parameter must be at least 3 characters long.
                              ^^ Method parameter must be at least 3 characters long.
                                       ^^ Only use lowercase characters for method parameter.
                                           ^^ Method parameter must be at least 3 characters long.
            end
            def t(x = A, y = 2)
                  ^ Method parameter must be at least 3 characters long.
                         ^ Method parameter must be at least 3 characters long.
            end
            "#,
        );
        expect_no_offenses(METHOD_PARAMETER, "def u\nend\n");
        expect_no_offenses(METHOD_PARAMETER, "def u()\nend\n");
    }

    /// 分解引数の名前は `arg.children.first.to_s`、つまり S 式そのもの。大文字を
    /// 含むかどうかだけがそこで問われ、レンジは S 式の文字数ぶん伸びる。
    #[test]
    fn method_parameter_name_reads_a_destructured_parameter_as_an_s_expression() {
        // `(arg :mA)` は 9 文字あるので、レンジは引数より長く、行末を越えて次の行に
        // 届く。キャレット注記では書けないので位置を直接指定する。
        CopCase::new(METHOD_PARAMETER, "def s((mA, b))\nend\n", Vec::new())
            .without_offense_check()
            .locations(&[(1, 7, 2, 1)])
            .lengths(&[9])
            .run();
        expect_no_offenses(METHOD_PARAMETER, "def s((m, n_o))\nend\n");
    }

    /// 空の heredoc には終端の位置が無いので、offense は開始デリミタに付く。
    /// 終端が字下げされていれば、レンジは行頭から始まる。
    #[test]
    fn heredoc_delimiter_naming_reports_the_opening_of_an_empty_heredoc() {
        expect_offense(
            HEREDOC_NAMING,
            r#"
            a = <<-END
              x
            END
            ^^^ Use meaningful heredoc delimiters.
            b = <<~EOS
              y
            EOS
            ^^^ Use meaningful heredoc delimiters.
            d = <<~END
                ^^^^^^ Use meaningful heredoc delimiters.
            END
            e = <<~"--"
              q
            --
            ^^ Use meaningful heredoc delimiters.
            "#,
        );
        expect_no_offenses(HEREDOC_NAMING, "c = <<-SQL\n  z\nSQL\n");
        expect_no_offenses(HEREDOC_NAMING, "g = <<~_\n  h\n_\n");
    }

    /// `loc.heredoc_end` は終端の行頭から始まるので、字下げされた終端を直すと
    /// 字下げごと消える。開始デリミタも同時に書き換わる。
    #[test]
    fn heredoc_delimiter_case_correction_drops_the_indentation_of_the_terminator() {
        expect_offense(
            HEREDOC_CASE,
            r#"
            a = <<-sql
              x
            sql
            ^^^ Use uppercase heredoc delimiters.
            def f
              b = <<~Eos
                y
                Eos
            ^^^^^^^ Use uppercase heredoc delimiters.
            end
            "#,
        );
        expect_correction(
            HEREDOC_CASE,
            "a = <<-sql\n  x\nsql\ndef f\n  b = <<~Eos\n    y\n    Eos\nend\n",
            "a = <<-SQL\n  x\nSQL\ndef f\n  b = <<~EOS\n    y\nEOS\nend\n",
        );
        expect_no_offenses(HEREDOC_CASE, "c = <<~SQL\n  z\nSQL\n");
    }

    /// 接頭辞のあとが数字だったり、名前が `=` で終わったり、`AllowedMethods` に
    /// 載っていれば免れる。動的定義はレシーバ無しの `define_method` に限られ、
    /// 第 1 引数はシンボルでなければならない。
    #[test]
    fn predicate_prefix_skips_the_names_upstream_allows() {
        expect_offense(
            PREDICATE_PREFIX,
            r#"
            def is_even(value)
                ^^^^^^^ Rename `is_even` to `even?`.
            end
            def has_foo
                ^^^^^^^ Rename `has_foo` to `foo?`.
            end
            def self.does_bar
                     ^^^^^^^^ Rename `does_bar` to `bar?`.
            end
            define_method(:is_even) { |v| }
                          ^^^^^^^^ Rename `is_even` to `even?`.
            "#,
        );
        for source in [
            "def is_a?(x)\nend\n",
            "def is_1(x)\nend\n",
            "def is_(x)\nend\n",
            "def is_foo=(v)\nend\n",
            "def isfoo\nend\n",
            "define_method(\"is_str\") { |v| }\n",
            "Foo.define_method(:is_y) { }\n",
            "def_node_matcher(:is_z) { }\n",
        ] {
            expect_no_offenses(PREDICATE_PREFIX, source);
        }
    }

    /// `on_arg` が見るのは必須引数だけ。既定値・キーワード・splat・ブロック引数は
    /// 別の型なので handler が無い。シンボルはエスケープを解いた値で判定される。
    #[test]
    fn variable_number_checks_only_the_nodes_upstream_has_handlers_for() {
        expect_offense(
            VARIABLE_NUMBER,
            r#"
            variable_1 = 1
            ^^^^^^^^^^ Use normalcase for variable numbers.
            @ivar_1 = 3
            ^^^^^^^ Use normalcase for variable numbers.
            @@cvar_1 = 4
            ^^^^^^^^ Use normalcase for variable numbers.
            $gvar_1 = 5
            ^^^^^^^ Use normalcase for variable numbers.
            a_1, b_2 = 1, 2
            ^^^ Use normalcase for variable numbers.
                 ^^^ Use normalcase for variable numbers.
            def some_method_1; end
                ^^^^^^^^^^^^^ Use normalcase for method name numbers.
            :some_sym_1
            ^^^^^^^^^^^ Use normalcase for symbol numbers.
            { key_1: 1 }
              ^^^^^ Use normalcase for symbol numbers.
            def m(arg_1, opt_1 = 1, *rest_1, kw_1:, **krest_1, &blk_1)
                  ^^^^^ Use normalcase for variable numbers.
            end
            proc { |p_1, (q_1, r_1)| }
                    ^^^ Use normalcase for variable numbers.
                          ^^^ Use normalcase for variable numbers.
                               ^^^ Use normalcase for variable numbers.
            "#,
        );
        // 定数の代入には handler が無く、`x86_64` は `AllowedIdentifiers` に載る。
        expect_no_offenses(VARIABLE_NUMBER, "variable1 = 2\n");
        expect_no_offenses(VARIABLE_NUMBER, "CONST_1 = 6\n");
        expect_no_offenses(VARIABLE_NUMBER, "x86_64 = 7\n");
        expect_no_offenses(VARIABLE_NUMBER, "proc { |_1| }\n");
    }

    /// `:"a\x5F1"` の値は `a_1` なので、綴りはエスケープを解いてから見る。
    #[test]
    fn variable_number_resolves_the_escapes_of_a_symbol() {
        expect_offense(
            VARIABLE_NUMBER,
            r#"
            :"a\x5F1"
            ^^^^^^^^^ Use normalcase for symbol numbers.
            "#,
        );
        expect_no_offenses(VARIABLE_NUMBER, ":\"a1\"\n");
    }

    /// 入れ子の rescue は外側だけが問われる。`_` で始まる名前には `_` 付きの
    /// 名前が求められる。
    #[test]
    fn rescued_exceptions_variable_name_leaves_a_nested_rescue_alone() {
        expect_offense(
            RESCUED,
            r#"
            begin
              foo
            rescue StandardError => err
                                    ^^^ Use `e` instead of `err`.
              puts err
            end
            begin
              foo
            rescue => _err
                      ^^^^ Use `_e` instead of `_err`.
            end
            def m
              foo
            rescue => exc
                      ^^^ Use `e` instead of `exc`.
              puts exc
            end
            begin
              foo
            rescue => e1
                      ^^ Use `e` instead of `e1`.
              begin
                bar
              rescue => e2
                baz
              end
            end
            "#,
        );
        expect_no_offenses(RESCUED, "begin\n  foo\nrescue => e\nend\n");
        expect_no_offenses(RESCUED, "begin\n  foo\nrescue => _e\nend\n");
    }

    /// autocorrect は再代入までの読み出しを書き換え、そこで止まる。再代入が
    /// 無ければ `begin`/`end` のあとに続く文も書き換える。
    #[test]
    fn rescued_exceptions_variable_name_stops_correcting_at_a_reassignment() {
        expect_correction(
            RESCUED,
            r#"
            begin
              foo
            rescue => err
              puts err
              err = 2
              puts err
            end
            puts err
            begin
              bar
            rescue => other
              puts other
            end
            puts other
            "#,
            r#"
            begin
              foo
            rescue => e
              puts e
              err = 2
              puts err
            end
            puts err
            begin
              bar
            rescue => e
              puts e
            end
            puts e
            "#,
        );
    }

    /// メモ化は本体の末尾にあるときだけ見られる。`initialize` 系と、`_` を落とせば
    /// 一致する名前は免れる。`defined?` 形式は 3 か所すべてが報告される。
    #[test]
    fn memoized_instance_variable_name_matches_the_method_it_memoizes() {
        expect_offense(
            MEMOIZED,
            r#"
            def foo
              @something ||= calculate
              ^^^^^^^^^^ Memoized variable `@something` does not match method name `foo`. Use `@foo` instead.
            end
            def waldo
              return @x if defined?(@x)
                     ^^ Memoized variable `@x` does not match method name `waldo`. Use `@waldo` instead.
                                    ^^ Memoized variable `@x` does not match method name `waldo`. Use `@waldo` instead.
              @x = compute
              ^^ Memoized variable `@x` does not match method name `waldo`. Use `@waldo` instead.
            end
            define_method(:plugh) do
              @nope ||= 1
              ^^^^^ Memoized variable `@nope` does not match method name `plugh`. Use `@plugh` instead.
            end
            "#,
        );
        for source in [
            "def bar\n  @bar ||= calculate\nend\n",
            "def _baz\n  @baz ||= calculate\nend\n",
            "def initialize\n  @whatever ||= 1\nend\n",
            "def +(other)\n  @plus ||= other\nend\n",
            "def first\n  @first ||= 1\n  do_something\nend\n",
        ] {
            expect_no_offenses(MEMOIZED, source);
        }
    }

    /// 本体が 1 文のときはその文の最後の子まで見る。ブロックの中の代入も
    /// `body.children.last` になり得る。
    #[test]
    fn memoized_instance_variable_name_looks_at_the_last_child_of_the_body() {
        expect_offense(
            MEMOIZED,
            r#"
            def nested
              [1].each { @inner ||= 1 }
                         ^^^^^^ Memoized variable `@inner` does not match method name `nested`. Use `@nested` instead.
            end
            def bytesize
              case value
              when NilClass
                0
              else
                @s ||= 1
                ^^ Memoized variable `@s` does not match method name `bytesize`. Use `@bytesize` instead.
              end
            end
            "#,
        );
        expect_no_offenses(
            MEMOIZED,
            "def other\n  case value\n  when NilClass\n    @t ||= 1\n  end\nend\n",
        );
    }

    #[test]
    fn memoized_instance_variable_name_correction_renames_the_variable() {
        expect_correction(
            MEMOIZED,
            "def foo\n  @something ||= calculate\nend\n",
            "def foo\n  @foo ||= calculate\nend\n",
        );
        expect_correction(
            MEMOIZED,
            "def waldo\n  return @x if defined?(@x)\n  @x = compute\nend\n",
            "def waldo\n  return @waldo if defined?(@waldo)\n  @waldo = compute\nend\n",
        );
    }

    /// `add_global_offense` はファイルの先頭を長さ 0 で指す。shebang のある
    /// スクリプトと gemspec、大文字を含む `Include` に載る名前は免れる。
    #[test]
    fn file_name_reports_the_whole_file_at_its_first_character() {
        let report = CopCase::annotated(
            FILE_NAME,
            "x = 1\n^{} The name of this source file (`fooBar.rb`) should use snake_case.\n",
        )
        .path("fooBar.rb")
        .run();
        assert_eq!(report.offenses.len(), 1);

        for (path, source) in [
            ("good_name.rb", "x = 1\n"),
            ("barBaz.rb", "#!/usr/bin/env ruby\nx = 1\n"),
            ("my-gem.gemspec", "x = 1\n"),
            ("Rakefile", "x = 1\n"),
        ] {
            CopCase::new(FILE_NAME, source, Vec::new()).path(path).run();
        }
    }

    /// 拡張子は最後の 1 つだけが落ちるので、途中に大文字があれば残る。
    /// 空のファイルでも名前は問われる。
    #[test]
    fn file_name_strips_only_the_last_extension() {
        for path in ["a.b.C.rb", "UPPER.rb", "weird name.rb"] {
            let report = CopCase::new(FILE_NAME, "x = 1\n", Vec::new())
                .path(path)
                .without_offense_check()
                .run();
            assert_eq!(report.offenses.len(), 1, "{path} should be reported");
        }
        let report = CopCase::new(FILE_NAME, "", Vec::new())
            .path("emptyName.rb")
            .without_offense_check()
            .run();
        assert_eq!(report.offenses.len(), 1);
    }
}

/// `Style/Documentation`: クラス・モジュールの直上に本物のコメントがあるか。
///
/// 期待値は本家 1.89.0 の `--only Style/Documentation --format json` の実測。
mod documentation {
    use super::*;

    #[test]
    fn reports_an_undocumented_class_and_module() {
        expect_offense(
            "Style/Documentation",
            r#"
            class Foo
            ^^^^^^^^^ Missing top-level documentation comment for `class Foo`.
              def bar; end
            end
            "#,
        );
        expect_offense(
            "Style/Documentation",
            r#"
            module Empty
            ^^^^^^^^^^^^ Missing top-level documentation comment for `module Empty`.
            end
            "#,
        );
    }

    /// 本体を持たないクラスは対象外。モジュールは本体が無くても対象になる。
    #[test]
    fn a_class_without_a_body_is_not_asked_for_documentation() {
        expect_no_offenses("Style/Documentation", "class Empty\nend\n");
    }

    #[test]
    fn a_comment_directly_above_the_definition_documents_it() {
        expect_no_offenses(
            "Style/Documentation",
            "# Documented\nclass Documented\n  def bar; end\nend\n",
        );
    }

    /// 注記コメント (`TODO:` 等)、マジックコメント、rubocop ディレクティブは
    /// 説明ではないので、直上にあっても文書とは見なされない。
    #[test]
    fn an_annotation_comment_does_not_document_anything() {
        expect_offense(
            "Style/Documentation",
            r#"
            # TODO: fix this
            class Annotated
            ^^^^^^^^^^^^^^^ Missing top-level documentation comment for `class Annotated`.
              def x; end
            end
            "#,
        );
        expect_offense(
            "Style/Documentation",
            r#"
            # frozen_string_literal: true
            class Magic
            ^^^^^^^^^^^ Missing top-level documentation comment for `class Magic`.
              def x; end
            end
            "#,
        );
    }

    /// 名前空間としてしか使っていないモジュール、`include` しかしない本体、
    /// `:nodoc:` を付けた定義は対象外。`:nodoc: all` は入れ子も免除する。
    #[test]
    fn namespaces_inclusions_and_nodoc_are_exempt() {
        expect_offense(
            "Style/Documentation",
            r#"
            module Namespace
              class Inner
              ^^^^^^^^^^^ Missing top-level documentation comment for `class Namespace::Inner`.
                def x; end
              end
            end
            "#,
        );
        expect_no_offenses("Style/Documentation", "module Mixin\n  include Foo\nend\n");
        expect_no_offenses(
            "Style/Documentation",
            "class WithNodoc # :nodoc:\n  def x; end\nend\n",
        );
        expect_no_offenses(
            "Style/Documentation",
            "module Outer # :nodoc: all\n  class Inside\n    def x; end\n  end\nend\n",
        );
    }

    /// 上位ノードが同じ位置から始まると、直上のコメントはそちらに吸われる。
    /// `module Foo end if false` はコメントを `if` に取られて未文書になる。
    ///
    /// 実測: ruby/ruby の `lib/English.rb:48` がこの形。
    #[test]
    fn a_modifier_keeps_the_definition_from_owning_the_comment_above_it() {
        expect_offense(
            "Style/Documentation",
            r#"
            # Explanation
            module English end if false
            ^^^^^^^^^^^^^^ Missing top-level documentation comment for `module English`.
            "#,
        );
    }
}

/// `%`-リテラルの区切り文字と、角括弧配列との相互変換。
///
/// 期待値は本家 1.89.0 の `--format json` と `-A` の実測。
mod percent_literals {
    use super::*;

    #[test]
    fn percent_literal_delimiters() {
        expect_offense(
            "Style/PercentLiteralDelimiters",
            r#"
            %w(a b)
            ^^^^^^^ `%w`-literals should be delimited by `[` and `]`.
            "#,
        );
        expect_offense(
            "Style/PercentLiteralDelimiters",
            r#"
            %q{bar}
            ^^^^^^^ `%q`-literals should be delimited by `(` and `)`.
            "#,
        );
        expect_no_offenses(
            "Style/PercentLiteralDelimiters",
            "%w[g h]\n%r{baz}\n%s(sym)\n",
        );
        expect_correction(
            "Style/PercentLiteralDelimiters",
            "%w(a b)\n%i(c d)\n%r[foo]\n%q{bar}\n",
            "%w[a b]\n%i[c d]\n%r{foo}\n%q(bar)\n",
        );
    }

    /// 中身に希望の区切り文字が入っていると書き換えられないので報告しない。
    /// `%w` / `%i` は自分が使っている区切り文字を含む場合も同じ。
    #[test]
    fn a_literal_holding_the_preferred_delimiter_is_left_alone() {
        expect_no_offenses("Style/PercentLiteralDelimiters", "%w(i (j))\n");
        expect_no_offenses("Style/PercentLiteralDelimiters", "x = %q{a (b}\n");
    }

    /// 完成したリテラルの後ろの `%` は剰余演算子で、`%`-リテラルではない。
    ///
    /// 実測: ruby/ruby の `libexec/erb:150` がこの形。
    #[test]
    fn a_percent_after_a_string_is_the_modulo_operator() {
        expect_no_offenses("Style/PercentLiteralDelimiters", "puts \"%3d\"%[l, x]\n");
    }

    #[test]
    fn symbol_and_word_arrays() {
        expect_offense(
            "Style/SymbolArray",
            r#"
            a = [:foo, :bar]
                ^^^^^^^^^^^^ Use `%i` or `%I` for an array of symbols.
            "#,
        );
        expect_offense(
            "Style/WordArray",
            r#"
            b = ['one', 'two']
                ^^^^^^^^^^^^^^ Use `%w` or `%W` for an array of words.
            "#,
        );
        expect_correction(
            "Style/SymbolArray",
            "a = [:foo, :bar]\n",
            "a = %i[foo bar]\n",
        );
        expect_correction(
            "Style/WordArray",
            "b = ['one', 'two']\n",
            "b = %w[one two]\n",
        );
    }

    /// 単語に見えない中身、要素が 1 つだけの配列、既に `%` 記法のものは対象外。
    #[test]
    fn arrays_that_cannot_take_the_percent_form_are_left_alone() {
        expect_no_offenses("Style/WordArray", "d = [\"it's\", 'plain']\n");
        expect_no_offenses("Style/SymbolArray", "g = [:a]\n");
        expect_no_offenses("Style/SymbolArray", "h = [:\"with space\", :b]\n");
        expect_no_offenses("Style/SymbolArray", "f = %i[a b]\n");
    }

    /// 複数行の配列は、各要素が書かれていた行と字下げをそのまま引き継ぐ。
    #[test]
    fn a_multiline_array_keeps_its_layout() {
        CopCase::new(
            "Style/SymbolArray",
            "c = [\n  :x,\n  :y\n]\n",
            vec![Annotation::new(
                1,
                5,
                1,
                "Use `%i` or `%I` for an array of symbols.",
            )],
        )
        .corrected("c = %i[\n  x\n  y\n]\n")
        .run();
    }
}

/// `Style/RegexpLiteral` / `Style/ClassAndModuleChildren` / `Style/SingleLineMethods`。
///
/// 期待値は本家 1.89.0 の `--format json` と `-A` の実測。
mod style_rest {
    use super::*;

    #[test]
    fn regexp_literal() {
        expect_offense(
            "Style/RegexpLiteral",
            r#"
            x = %r{foo}
                ^^^^^^^ Use `//` around regular expression.
            "#,
        );
        expect_offense(
            "Style/RegexpLiteral",
            r"
            y = /a\/b/
                ^^^^^^ Use `%r` around regular expression.
            ",
        );
        expect_correction("Style/RegexpLiteral", "x = %r{foo}\n", "x = /foo/\n");
        expect_correction("Style/RegexpLiteral", "y = /a\\/b/\n", "y = %r{a/b}\n");
    }

    /// スラッシュを含む `%r`、括弧を省いた呼び出しの引数、括弧の釣り合わない
    /// スラッシュリテラルは、どれも書き換えられないので報告しない。
    #[test]
    fn regexp_literals_that_cannot_change_form_are_left_alone() {
        expect_no_offenses("Style/RegexpLiteral", "w = %r{a/b}\n");
        expect_no_offenses("Style/RegexpLiteral", "u = bar %r{ baz}\n");
        expect_no_offenses("Style/RegexpLiteral", "t = /a{b/\n");
        expect_no_offenses("Style/RegexpLiteral", "z = /plain/\n");
    }

    #[test]
    fn class_and_module_children() {
        expect_offense(
            "Style/ClassAndModuleChildren",
            r#"
            class Foo::Bar
                  ^^^^^^^^ Use nested module/class definitions instead of compact style.
              X = 1
            end
            "#,
        );
        expect_correction(
            "Style/ClassAndModuleChildren",
            "class Foo::Bar\n  X = 1\nend\n",
            "module Foo\n  class Bar\n  X = 1\n  end\nend\n",
        );
    }

    /// 直前の兄弟に同名のクラス定義があれば `class`、無ければ `module` で包む。
    /// 外側の定義の本体がそれ 1 つだけのときは、そもそも報告しない。
    #[test]
    fn the_wrapper_keyword_comes_from_the_previous_statement() {
        expect_correction(
            "Style/ClassAndModuleChildren",
            "module Deep\n  class Q\n  end\n  class Q::R\n    def s; end\n  end\nend\n",
            "module Deep\n  class Q\n  end\n  class Q\n      class R\n    def s; end\n      end\n  end\nend\n",
        );
        expect_no_offenses(
            "Style/ClassAndModuleChildren",
            "module Foo\n  class Bar::Baz\n    def a; end\n  end\nend\n",
        );
    }

    #[test]
    fn trailing_comma() {
        expect_offense(
            "Style/TrailingCommaInArrayLiteral",
            r#"
            a = [1, 2,]
                     ^ Avoid comma after the last item of an array.
            "#,
        );
        expect_offense(
            "Style/TrailingCommaInHashLiteral",
            r#"
            b = { c: 1, }
                      ^ Avoid comma after the last item of a hash.
            "#,
        );
        expect_offense(
            "Style/TrailingCommaInArguments",
            r#"
            foo(1, 2,)
                    ^ Avoid comma after the last parameter of a method call.
            "#,
        );
        expect_offense(
            "Style/TrailingCommaInArguments",
            r#"
            d = e[1,]
                   ^ Avoid comma after the last parameter of a method call.
            "#,
        );
        expect_correction(
            "Style/TrailingCommaInArrayLiteral",
            "f = [\n  1,\n  2,\n]\n",
            "f = [\n  1,\n  2\n]\n",
        );
    }

    /// 括弧を伴わない呼び出しと `super` / `yield` は、そもそも `on_send` に
    /// 来ないので対象外。
    #[test]
    fn only_bracketed_argument_lists_are_checked() {
        expect_no_offenses("Style/TrailingCommaInArguments", "foo 1, 2\n");
        expect_no_offenses(
            "Style/TrailingCommaInArguments",
            "def a(b)\n  super(b,)\nend\n",
        );
    }

    #[test]
    fn single_line_methods() {
        expect_offense(
            "Style/SingleLineMethods",
            r#"
            def foo; bar; end
            ^^^^^^^^^^^^^^^^^ Avoid single-line method definitions.
            "#,
        );
        expect_no_offenses("Style/SingleLineMethods", "def empty; end\n");
        expect_correction(
            "Style/SingleLineMethods",
            "class K\n  def m; n; o; end\nend\n",
            "class K\n  def m; \n    n; \n    o; \n  end\nend\n",
        );
    }

    /// Ruby 3.0 以降は本体を `=` の右に畳む。引数を持つ呼び出しは括弧を補って
    /// 書き直され、算術・比較の演算子だけはそのまま残る。
    ///
    /// 実測: `.ruby-version` に 3.2.0 を置いた `-A` の出力。
    #[test]
    fn an_endless_definition_replaces_the_single_line_one_from_ruby_3_0() {
        let source = concat!(
            "def foo; bar; end\n",
            "def self.logger; config.logger; end\n",
            "def with_args(a); helper a; end\n",
            "def arith; a + b; end\n",
            "def shovel; a << b; end\n",
            "def multi; a; b; end\n",
            "def ret; return 1; end\n",
            "def op=(v); @v = v; end\n",
        );
        CopCase::new("Style/SingleLineMethods", source, Vec::new())
            .without_offense_check()
            .target_ruby("3.2")
            .corrected(concat!(
                "def foo() = bar\n",
                "def self.logger() = config.logger\n",
                "def with_args(a) = helper(a)\n",
                "def arith() = a + b\n",
                "def shovel() = a.<<(b)\n",
                "def multi; \n  a; \n  b; \nend\n",
                "def ret; \n  return 1; \nend\n",
                "def op=(v); \n  @v = v; \nend\n",
            ))
            .run();
    }

    /// 行末コメントは定義の上の行に持ち上げられる。
    #[test]
    fn a_trailing_comment_moves_above_the_definition() {
        expect_correction(
            "Style/SingleLineMethods",
            "def foo; bar; end # note\n",
            "# note\ndef foo; \n  bar; \nend \n",
        );
    }
}

/// `Metrics/AbcSize` / `Metrics/CyclomaticComplexity` / `Metrics/PerceivedComplexity` /
/// `Metrics/BlockNesting`。期待値はすべて本家 1.89.0 の実出力から取った。
///
/// ABC は `<assignment, branch, condition>` のベクタまでメッセージに出るので、
/// どの数え方が食い違ったかがそのまま読める。数え方の根拠は
/// `lib/rubocop/cop/metrics/utils/abc_size_calculator.rb` と
/// `lib/rubocop/cop/mixin/method_complexity.rb`。
mod metrics_complexity {
    use super::*;

    /// 実測: `[<4, 3, 2> 5.39/2]` / 1:1-7:3 / length 110
    #[test]
    fn abc_size_reports_the_whole_definition_with_its_vector() {
        CopCase::annotated(
            "Metrics/AbcSize",
            r#"
            def compute(input)
            ^^^^^^^^^^^^^^^^^^ Assignment Branch Condition size for `compute` is too high. [<4, 3, 2> 5.39/2]
              total = 0
              input.each do |item|
                total += item.value if item.valid?
              end
              total
            end
            "#,
        )
        .config("Metrics/AbcSize:\n  Max: 2\n")
        .locations(&[(1, 1, 7, 3)])
        .lengths(&[110])
        .severity(Severity::Convention)
        .correctable(false)
        .run();
    }

    /// 代入の数え方。`_` 始まりの名前は数えず、`self.c =` と `d[0] =` は setter 呼び出しなので
    /// 代入と分岐の両方に入り、`g ||= h` は `or_asgn` 自身ではなく畳まれた子が数えられる。
    ///
    /// 実測: `[<10, 4, 1> 10.82/0]` / 1:1-11:3 / length 120
    #[test]
    fn abc_size_counts_every_shape_of_assignment() {
        CopCase::annotated(
            "Metrics/AbcSize",
            r#"
            def sizes
            ^^^^^^^^^ Assignment Branch Condition size for `sizes` is too high. [<10, 4, 1> 10.82/0]
              a = 1
              _skipped = 2
              @b = a
              self.c = a
              d[0] = a
              e, f = a, a
              g ||= h
              i = -1
              j = defined?(a)
            end
            "#,
        )
        .config("Metrics/AbcSize:\n  Max: 0\n")
        .locations(&[(1, 1, 11, 3)])
        .lengths(&[120])
        .run();
    }

    /// 分岐の数え方。`x == 1` は比較なので条件に回り、`&.` は分岐と条件の両方に入るが
    /// 2 度目は割り引かれ、`super` と `defined?` は呼び出しではない。`->(y) { y }` は
    /// `lambda` 呼び出し 1 個として数えられる。
    ///
    /// 実測: `[<2, 7, 3> 7.87/0]` / 1:1-11:3 / length 109
    #[test]
    fn abc_size_counts_calls_as_branches() {
        CopCase::annotated(
            "Metrics/AbcSize",
            r#"
            def branches(x)
            ^^^^^^^^^^^^^^^ Assignment Branch Condition size for `branches` is too high. [<2, 7, 3> 7.87/0]
              x.foo
              x&.bar
              x&.bar
              x == 1
              x[0]
              yield
              super
              ->(y) { y }
              x.map { |y| y }
            end
            "#,
        )
        .config("Metrics/AbcSize:\n  Max: 0\n")
        .locations(&[(1, 1, 11, 3)])
        .lengths(&[109])
        .run();
    }

    /// 条件の数え方。`else` を持つ `if` は 2、三項演算子は `loc.else` を持たないので 1、
    /// `case` 自身は数えず `when` が数えられ、`rescue` 節は 1。
    ///
    /// 実測: `[<0, 0, 6> 6/0]` / 1:1-20:3 / length 182
    #[test]
    fn abc_size_counts_decision_points_as_conditions() {
        CopCase::annotated(
            "Metrics/AbcSize",
            r#"
            def conditions(x)
            ^^^^^^^^^^^^^^^^^ Assignment Branch Condition size for `conditions` is too high. [<0, 0, 6> 6/0]
              if x
                1
              else
                2
              end
              x ? 1 : 2
              while x
                break
              end
              case x
              when 1 then 2
              else 3
              end
              begin
                x
              rescue StandardError
                nil
              end
            end
            "#,
        )
        .config("Metrics/AbcSize:\n  Max: 0\n")
        .locations(&[(1, 1, 20, 3)])
        .lengths(&[182])
        .run();
    }

    /// 同じ変数への `&.` の繰り返しは 1 度しか条件に数えられず、その変数への代入で数え直す。
    ///
    /// 実測: ABC `[<1, 3, 2> 3.74/0]` / Cyclomatic `[3/0]`
    #[test]
    fn repeated_safe_navigation_is_discounted_until_the_variable_is_written() {
        CopCase::annotated(
            "Metrics/AbcSize",
            r#"
            def repeated(var)
            ^^^^^^^^^^^^^^^^^ Assignment Branch Condition size for `repeated` is too high. [<1, 3, 2> 3.74/0]
              var&.one
              var&.two
              var = 1
              var&.three
            end
            "#,
        )
        .config("Metrics/AbcSize:\n  Max: 0\n")
        .locations(&[(1, 1, 6, 3)])
        .run();
        CopCase::annotated(
            "Metrics/CyclomaticComplexity",
            r#"
            def repeated(var)
            ^^^^^^^^^^^^^^^^^ Cyclomatic complexity for `repeated` is too high. [3/0]
              var&.one
              var&.two
              var = 1
              var&.three
            end
            "#,
        )
        .config("Metrics/CyclomaticComplexity:\n  Max: 0\n")
        .locations(&[(1, 1, 6, 3)])
        .run();
    }

    /// 本家 cop のドキュメントに載っている例。合計 6 になる。
    #[test]
    fn cyclomatic_complexity_matches_the_documented_example() {
        CopCase::annotated(
            "Metrics/CyclomaticComplexity",
            r#"
            def each_child_node(*types)
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^ Cyclomatic complexity for `each_child_node` is too high. [6/5]
              unless block_given?
                return to_enum(__method__, *types)
              end

              children.each do |child|
                next unless child.is_a?(Node)

                yield child if types.empty? ||
                               types.include?(child.type)
              end

              self
            end
            "#,
        )
        .config("Metrics/CyclomaticComplexity:\n  Max: 5\n")
        .locations(&[(1, 1, 14, 3)])
        .lengths(&[256])
        .run();
    }

    /// 反復メソッドでないブロックと `&:sym`、`begin … end while`、`_1` を使う numblock は
    /// いずれも経路を増やさない。数えるのは `each {}` と `each(&:to_s)` の 2 つだけ。
    ///
    /// 実測: `[3/0]`
    #[test]
    fn only_iterating_blocks_add_a_path() {
        CopCase::annotated(
            "Metrics/CyclomaticComplexity",
            r#"
            def blocks(list)
            ^^^^^^^^^^^^^^^^ Cyclomatic complexity for `blocks` is too high. [3/0]
              list.each { |x| x }
              list.other { |x| x }
              list.map(&:to_s)
              list.other(&:to_s)
              begin
                list
              end while list
              list.map { _1 }
            end
            "#,
        )
        .config("Metrics/CyclomaticComplexity:\n  Max: 0\n")
        .locations(&[(1, 1, 10, 3)])
        .lengths(&[157])
        .run();
    }

    /// 本家 cop のドキュメントに載っている例。`case` は 0.8 点、`when` は 1 つ 0.2 点。
    #[test]
    fn perceived_complexity_matches_the_documented_case_example() {
        CopCase::annotated(
            "Metrics/PerceivedComplexity",
            r#"
            def example_1
            ^^^^^^^^^^^^^ Perceived complexity for `example_1` is too high. [7/6]
              if cond
                case var
                when 1 then func_one
                when 2 then func_two
                when 3 then func_three
                when 4..10 then func_other
                end
              else
                do_something until a && b
              end
            end
            "#,
        )
        .config("Metrics/PerceivedComplexity:\n  Max: 6\n")
        .locations(&[(1, 1, 12, 3)])
        .lengths(&[199])
        .run();
    }

    /// 同じくドキュメントの例。リテラルだけの `in` 節は `when` と同じ 0.2 点に割り引かれる。
    #[test]
    fn perceived_complexity_discounts_literal_in_patterns() {
        CopCase::annotated(
            "Metrics/PerceivedComplexity",
            r#"
            def example_2
            ^^^^^^^^^^^^^ Perceived complexity for `example_2` is too high. [2/1]
              case color
              in "red" then func_red
              in "blue" then func_blue
              in "green" then func_green
              end
            end
            "#,
        )
        .config("Metrics/PerceivedComplexity:\n  Max: 1\n")
        .target_ruby("3.0")
        .locations(&[(1, 1, 7, 3)])
        .lengths(&[117])
        .run();
    }

    /// `define_method` はリテラルの名前を渡したときだけ測られる。名前が式なら
    /// `on_block` のパターンに合わないので、その定義はどの複雑度 cop も見ない。
    #[test]
    fn define_method_is_measured_only_with_a_literal_name() {
        CopCase::annotated(
            "Metrics/AbcSize",
            r#"
            define_method(:literal) do |value|
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Assignment Branch Condition size for `literal` is too high. [<0, 3, 0> 3/1]
              value.to_s + value.to_s
            end
            define_method(name) do |value|
              value.to_s + value.to_s
            end
            "#,
        )
        .config("Metrics/AbcSize:\n  Max: 1\n")
        .locations(&[(1, 1, 3, 3)])
        .lengths(&[64])
        .run();
    }

    /// `AllowedMethods` と `AllowedPatterns` はどちらも名前で定義を丸ごと除外する。
    #[test]
    fn allowed_methods_and_patterns_skip_a_definition() {
        let source =
            "def skipped(a)\n  a.to_s + a.to_s\nend\n\ndef watched(a)\n  a.to_s + a.to_s\nend\n";
        CopCase::new(
            "Metrics/AbcSize",
            source,
            vec![Annotation::new(
                5,
                1,
                14,
                "Assignment Branch Condition size for `watched` is too high. [<0, 3, 0> 3/1]",
            )],
        )
        .config("Metrics/AbcSize:\n  Max: 1\n  AllowedMethods:\n    - skipped\n")
        .locations(&[(5, 1, 7, 3)])
        .run();
        CopCase::new(
            "Metrics/CyclomaticComplexity",
            source,
            vec![Annotation::new(
                5,
                1,
                14,
                "Cyclomatic complexity for `watched` is too high. [1/0]",
            )],
        )
        .config("Metrics/CyclomaticComplexity:\n  Max: 0\n  AllowedPatterns:\n    - '\\Askip'\n")
        .locations(&[(5, 1, 7, 3)])
        .run();
    }

    /// 本体の無いメソッドは `check_complexity` が最初に弾く。コメントしか無いものも同じ。
    #[test]
    fn a_method_without_a_body_is_never_measured() {
        for cop in [
            "Metrics/AbcSize",
            "Metrics/CyclomaticComplexity",
            "Metrics/PerceivedComplexity",
        ] {
            CopCase::new(
                cop,
                "def empty\nend\n\ndef only_comment\n  # nothing\nend\n",
                Vec::new(),
            )
            .config(&format!("{cop}:\n  Max: 0\n"))
            .run();
        }
    }

    /// 実測: 5:9-7:11 / length 39
    #[test]
    fn block_nesting_reports_the_innermost_level_over_the_limit() {
        CopCase::annotated(
            "Metrics/BlockNesting",
            r#"
            def deep(a, b, c, d)
              if a
                if b
                  if c
                    if d
                    ^^^^ Avoid more than 3 levels of block nesting.
                      do_something
                    end
                  end
                end
              end
            end
            "#,
        )
        .locations(&[(5, 9, 7, 11)])
        .lengths(&[39])
        .severity(Severity::Convention)
        .correctable(false)
        .run();
    }

    /// `elsif` は上の `if` の続きなので段を増やさず、修飾子の `if` も既定では増やさない。
    /// ただし段が既に上限を超えていれば、増やさない節でも報告はされる。
    #[test]
    fn elsif_and_modifier_forms_do_not_add_a_level() {
        let source = concat!(
            "def chained(a, b, c, d)\n",
            "  if a\n",
            "    if b\n",
            "      if c\n",
            "        if d\n",
            "          one\n",
            "        elsif d\n",
            "          two\n",
            "        end\n",
            "        three if d\n",
            "      end\n",
            "    end\n",
            "  end\n",
            "end\n",
        );
        let over = Annotation::new(5, 9, 4, "Avoid more than 3 levels of block nesting.");
        CopCase::new("Metrics/BlockNesting", source, vec![over.clone()])
            .locations(&[(5, 9, 9, 11)])
            .lengths(&[60])
            .run();
        CopCase::new(
            "Metrics/BlockNesting",
            source,
            vec![
                over,
                Annotation::new(10, 9, 10, "Avoid more than 3 levels of block nesting."),
            ],
        )
        .config("Metrics/BlockNesting:\n  CountBlocks: true\n  CountModifierForms: true\n")
        .locations(&[(5, 9, 9, 11), (10, 9, 10, 18)])
        .lengths(&[60, 10])
        .run();
    }

    /// `rescue` 節は `resbody` として 1 段に数えられる。修飾子の `rescue` も同じで、
    /// 報告されるのはキーワードからハンドラまで -- 守られている式は外側にある。
    #[test]
    fn rescue_clauses_are_a_nesting_level_of_their_own() {
        CopCase::annotated(
            "Metrics/BlockNesting",
            r#"
            def guarded(a, b, c)
              if a
                while b
                  if c
                    begin
                      work
                    rescue StandardError
                    ^^^^^^^^^^^^^^^^^^^^ Avoid more than 3 levels of block nesting.
                      recover
                    end
                    other rescue nil
                          ^^^^^^^^^^ Avoid more than 3 levels of block nesting.
                  end
                end
              end
            end
            "#,
        )
        .locations(&[(7, 9, 8, 17), (10, 15, 10, 24)])
        .lengths(&[38, 10])
        .run();
    }

    /// ブロックは `CountBlocks` を立てたときだけ段に数えられ、報告はブロックを取る
    /// 呼び出しから始まる -- 上流の `block` ノードはそこから始まっているため。
    #[test]
    fn blocks_count_only_when_asked_for() {
        let source = concat!(
            "def blocky(list)\n",
            "  list.each do |a|\n",
            "    a.each do |b|\n",
            "      b.each do |c|\n",
            "        c.each do |d|\n",
            "          d\n",
            "        end\n",
            "      end\n",
            "    end\n",
            "  end\n",
            "end\n",
        );
        CopCase::new("Metrics/BlockNesting", source, Vec::new()).run();
        CopCase::new(
            "Metrics/BlockNesting",
            source,
            vec![Annotation::new(
                5,
                9,
                13,
                "Avoid more than 3 levels of block nesting.",
            )],
        )
        .config("Metrics/BlockNesting:\n  CountBlocks: true\n")
        .locations(&[(5, 9, 7, 11)])
        .lengths(&[37])
        .run();
    }
}

/// `Lint/InterpolationCheck`。本家は `str` と、シングルクォートで書かれた `dstr` だけを
/// 見る。判定の核は「引用符を差し替えたものが Ruby として通り、かつ補間する文字列に
/// なるか」で、`%q()` はこれで落ちる。ヒアドキュメントとその中身は対象外。
///
/// 期待値はすべて本家 1.89.0 の `--only Lint/InterpolationCheck --format json` 実測。
mod interpolation_check {
    use super::*;

    const COP: &str = "Lint/InterpolationCheck";
    const MSG: &str = "Interpolation in single quoted string detected. Use double quoted strings \
                       if you need interpolation.";

    #[test]
    fn a_single_quoted_string_holding_an_interpolation_is_reported() {
        CopCase::new(
            COP,
            "foo = 'something with #{interpolation} inside'\n",
            vec![Annotation::new(1, 7, 40, MSG)],
        )
        .run();
    }

    /// 二重引用符で書かれていれば意図どおり。`%q()` は引用符を差し替えても中身が
    /// そのままなので、補間する文字列にならない。バックスラッシュで逃した `\#{` も同じ。
    #[test]
    fn a_string_that_means_what_it_says_is_left_alone() {
        expect_no_offenses(COP, "bar = \"something with #{interpolation} inside\"\n");
        expect_no_offenses(COP, "baz = %q(#{x})\n");
        expect_no_offenses(COP, "esc = '\\#{x}'\n");
        expect_no_offenses(COP, "sym = :'#{x}'\n");
        expect_no_offenses(COP, "words = %w[#{x}]\n");
    }

    /// `#{` と `}` が別の行にあるものは本家の正規表現に掛からない。行を跨ぐのは
    /// 「補間の中身」ではなく、複数行の単一引用文字列そのもの。
    #[test]
    fn the_braces_have_to_close_on_the_line_they_opened_on() {
        expect_no_offenses(COP, "split = '#{\nx}'\n");
        CopCase::new(
            COP,
            "multi = 'a\n#{x}'\n",
            vec![Annotation::new(1, 9, 2, MSG)],
        )
        .locations(&[(1, 9, 2, 5)])
        .lengths(&[8])
        .run();
    }

    /// ヒアドキュメントは本文も、その補間の中に書かれた文字列も対象外。正規表現の
    /// 補間の中は対象。
    #[test]
    fn a_heredoc_and_everything_written_inside_one_is_skipped() {
        expect_no_offenses(COP, "x = <<~TEXT\n  #{'#{y}'}\nTEXT\n");
        expect_no_offenses(COP, "z = <<~'TEXT'\n  #{y}\nTEXT\n");
        expect_offense(
            COP,
            r#"
            w = /#{'#{y}'}/
                   ^^^^^^ Interpolation in single quoted string [...]
            "#,
        );
    }

    /// autocorrect は引用符を二重引用符に差し替える。中に `"` があると literal が
    /// そこで終わってしまうので、そのときだけ `%{...}` にする。
    #[test]
    fn correction_swaps_the_quotes_or_falls_back_to_percent_braces() {
        expect_correction(COP, "foo = '#{x}'\n", "foo = \"#{x}\"\n");
        expect_correction(
            COP,
            "qux = 'say \"#{x}\" now'\n",
            "qux = %{say \"#{x}\" now}\n",
        );
        expect_correction(COP, "concat = 'a' '#{x}'\n", "concat = 'a' \"#{x}\"\n");
    }
}

/// `Layout/SpaceInsideArrayLiteralBrackets` と
/// `Layout/SpaceInsidePercentLiteralDelimiters`。期待値は本家 1.89.0 の
/// `--only <cop> --format json` / `-A` の実出力から取った。
mod layout_bracket_spacing {
    use super::*;

    const BRACKETS: &str = "Layout/SpaceInsideArrayLiteralBrackets";
    const PERCENT: &str = "Layout/SpaceInsidePercentLiteralDelimiters";
    const MSG: &str = "Do not use space inside array brackets.";
    const EMPTY_MSG: &str = "Do not use space inside empty array brackets.";
    const PERCENT_MSG: &str = "Do not use spaces inside percent literal delimiters.";

    /// 空白は左右それぞれ 1 件ずつ報告される。空の `[ ]` は括弧ごと 1 件。
    /// 入れ子は内側の配列も別の node なので、それぞれが自分の括弧を見る。
    #[test]
    fn each_bracket_reports_the_run_of_spaces_beside_it() {
        CopCase::new(
            BRACKETS,
            concat!(
                "array = [ a, b ]\n",
                "empty = [ ]\n",
                "nested = [[ 1 ], [ 2 ]]\n",
            ),
            vec![
                Annotation::new(1, 10, 1, MSG),
                Annotation::new(1, 15, 1, MSG),
                Annotation::new(2, 9, 3, EMPTY_MSG),
                Annotation::new(3, 12, 1, MSG),
                Annotation::new(3, 14, 1, MSG),
                Annotation::new(3, 19, 1, MSG),
                Annotation::new(3, 21, 1, MSG),
            ],
        )
        .run();
    }

    /// 本家は node 1 個につき 1 回しか corrector を回さないので、2 件目の offense は
    /// corrector が空のまま `correctable: false` で出る。
    #[test]
    fn only_the_first_offense_of_a_node_carries_the_correction() {
        let report = CopCase::new(BRACKETS, "array = [ a, b ]\n", vec![])
            .without_offense_check()
            .inspect();
        let correctable: Vec<bool> = report
            .offenses
            .iter()
            .map(sonicop::diagnostic::Offense::is_correctable)
            .collect();
        assert_eq!(correctable, vec![true, false]);
    }

    /// 閉じ括弧が自分の行を独り占めしていれば右側は免除される。行頭の `[` に
    /// コメントが続く場合は左側が免除される。免除された側も autocorrect では
    /// 一緒に詰められる。
    #[test]
    fn a_bracket_on_its_own_line_and_a_comment_after_the_opening_one_are_excused() {
        CopCase::new(
            BRACKETS,
            concat!(
                "own = [ 1,\n",
                "  2\n",
                "    ]\n",
                "comment = [ # note\n",
                "  1 ]\n",
            ),
            vec![Annotation::new(1, 8, 1, MSG), Annotation::new(5, 4, 1, MSG)],
        )
        .run();
        expect_correction(
            BRACKETS,
            concat!("own = [ 1,\n", "  2\n", "    ]\n"),
            concat!("own = [1,\n", "  2\n", "]\n"),
        );
        expect_correction(
            BRACKETS,
            concat!("comment = [ # note\n", "  1 ]\n"),
            concat!("comment = [# note\n", "  1]\n"),
        );
    }

    /// `%w[...]` は括弧を持たないので配列側の cop は触らない。
    #[test]
    fn a_percent_literal_is_not_a_bracketed_array() {
        expect_no_offenses(BRACKETS, "words = %w[ a b ]\n");
    }

    /// 前後の空白はそれぞれ 1 件。空の本文はまとめて 1 件。複数行は前後の空白を
    /// 見ない。`\` で逃した空白は語の一部なので、末尾の 1 個だけが残る。
    #[test]
    fn percent_literals_report_their_edge_spaces() {
        CopCase::new(
            PERCENT,
            concat!(
                "w = %w( foo bar )\n",
                "i = %i(  baz )\n",
                "x = %x( ls )\n",
                "blank = %w( )\n",
                "multiline = %w( a\n",
                "  b )\n",
                "escaped = %w(a\\  )\n",
                "plain = %w(ok)\n",
            ),
            vec![
                Annotation::new(1, 8, 1, PERCENT_MSG),
                Annotation::new(1, 16, 1, PERCENT_MSG),
                Annotation::new(2, 8, 2, PERCENT_MSG),
                Annotation::new(2, 13, 1, PERCENT_MSG),
                Annotation::new(3, 8, 1, PERCENT_MSG),
                Annotation::new(3, 11, 1, PERCENT_MSG),
                Annotation::new(4, 12, 1, PERCENT_MSG),
                Annotation::new(7, 17, 1, PERCENT_MSG),
            ],
        )
        .run();
    }

    /// バッククォートのコマンド実行は `%` で始まらないので対象外。
    #[test]
    fn a_backtick_command_is_not_a_percent_literal() {
        expect_no_offenses(PERCENT, "x = `ls `\n");
        expect_no_offenses(PERCENT, "q = %q( s )\n");
    }
}

/// `Layout/EmptyLinesAroundAccessModifier` と `Layout/EmptyLineAfterGuardClause`。
mod layout_blank_lines {
    use super::*;

    const MODIFIER: &str = "Layout/EmptyLinesAroundAccessModifier";
    const GUARD: &str = "Layout/EmptyLineAfterGuardClause";

    /// class 本体の先頭行に来た修飾子は「後ろだけ」を求められる。それ以外は前後
    /// 両方。空行が揃っていれば報告しない。
    #[test]
    fn the_message_depends_on_whether_the_modifier_opens_the_body() {
        CopCase::new(
            MODIFIER,
            concat!(
                "class Foo\n",
                "  def a; end\n",
                "  private\n",
                "  def b; end\n",
                "end\n",
                "class Bar\n",
                "  private\n",
                "  def b; end\n",
                "end\n",
                "module Baz\n",
                "  def a; end\n",
                "\n",
                "  private\n",
                "\n",
                "  def b; end\n",
                "end\n",
            ),
            vec![
                Annotation::new(3, 3, 7, "Keep a blank line before and after `private`."),
                Annotation::new(7, 3, 7, "Keep a blank line after `private`."),
            ],
        )
        .run();
    }

    /// 引数付きの `private :foo` と、class 本体の外に書かれた `private` は
    /// `bare_access_modifier?` に落ちない。
    #[test]
    fn only_a_bare_modifier_inside_a_class_like_body_counts() {
        expect_no_offenses(MODIFIER, "class Foo\n  private :bar\n  def b; end\nend\n");
        expect_no_offenses(MODIFIER, "def foo\n  private\n  bar\nend\n");
        expect_no_offenses(
            MODIFIER,
            "x = Class.new do\n  private\n\n  def b; end\nend\n",
        );
    }

    /// 修飾子形式は node ごと、`if ... end` 形式は `end` を指す。次の行が空なら
    /// 報告しない。
    #[test]
    fn a_guard_clause_wants_a_blank_line_after_it() {
        CopCase::new(
            GUARD,
            concat!(
                "def foo\n",
                "  return if a\n",
                "  bar\n",
                "end\n",
                "def baz\n",
                "  return if a\n",
                "  return if b\n",
                "\n",
                "  qux\n",
                "end\n",
                "def quux\n",
                "  if a\n",
                "    return\n",
                "  end\n",
                "  corge\n",
                "end\n",
            ),
            vec![
                Annotation::new(2, 3, 11, "Add empty line after guard clause."),
                Annotation::new(14, 3, 3, "Add empty line after guard clause."),
            ],
        )
        .run();
    }

    /// 続く文が無い、`while` 本体の最後、else 節の直前といった場所では
    /// `right_sibling` が無いか else に当たるので報告しない。
    #[test]
    fn a_guard_clause_with_nothing_after_it_is_left_alone() {
        expect_no_offenses(GUARD, "def foo\n  bar\n  return if a\nend\n");
        expect_no_offenses(GUARD, "if a\n  return if b\nelse\n  c\nend\n");
        expect_no_offenses(
            GUARD,
            "def foo\n  return if a\n  # rubocop:enable Style/Foo\n\n  bar\nend\n",
        );
    }

    /// `while` / `until` の本体も `begin` になるので、そこに書かれた guard clause も
    /// 対象になる。
    #[test]
    fn a_loop_body_is_a_statement_list_too() {
        CopCase::new(
            GUARD,
            concat!("until a\n", "  next if b\n", "  c\n", "end\n"),
            vec![Annotation::new(
                2,
                3,
                9,
                "Add empty line after guard clause.",
            )],
        )
        .run();
    }

    /// autocorrect は行末に改行を足す。ヒアドキュメントを持つ guard clause は
    /// 終端子の行の後ろに入る。
    #[test]
    fn the_blank_line_goes_after_the_whole_clause() {
        expect_correction(
            GUARD,
            concat!("def foo\n", "  return if a\n", "  bar\n", "end\n"),
            concat!("def foo\n", "  return if a\n", "\n", "  bar\n", "end\n"),
        );
        expect_correction(
            GUARD,
            concat!(
                "def foo\n",
                "  raise <<~MSG if a\n",
                "    hello\n",
                "  MSG\n",
                "  bar\n",
                "end\n",
            ),
            concat!(
                "def foo\n",
                "  raise <<~MSG if a\n",
                "    hello\n",
                "  MSG\n",
                "\n",
                "  bar\n",
                "end\n",
            ),
        );
    }
}

/// `Lint/ConstantDefinitionInBlock`。本家の判定は「ブロック本体の文として直接書かれて
/// いるか」。`rescue` / `else` / `ensure` 節が付くと本体がもう 1 段深いノードになるので、
/// 同じ見た目でも対象から外れる。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod constant_definition_in_block {
    use super::*;

    const COP: &str = "Lint/ConstantDefinitionInBlock";
    const MSG: &str = "Do not define constants this way within a block.";

    #[test]
    fn a_constant_or_a_class_written_in_a_block_is_reported() {
        expect_offense(
            COP,
            r#"
            [1].each do
              FOO = 1
              ^^^^^^^ Do not define constants this way within a block.
              BAR = 2
              ^^^^^^^ Do not define constants this way within a block.
            end
            "#,
        );
        CopCase::new(
            COP,
            "[1].each do\n  class Qux\n  end\n\n  module Mod\n  end\nend\n",
            vec![
                Annotation::new(2, 3, 9, MSG),
                Annotation::new(5, 3, 10, MSG),
            ],
        )
        .locations(&[(2, 3, 3, 5), (5, 3, 6, 5)])
        .lengths(&[15, 16])
        .run();
    }

    /// 修飾された名前は置き場所を自分で決めているので対象外。ブロックの直下でない
    /// もの、`ensure` の付いたブロックの中も同じ。`AllowedMethods` の既定は `enums`。
    #[test]
    fn only_a_bare_name_written_directly_in_the_block_counts() {
        expect_no_offenses(COP, "[1].each do\n  Foo::BAZ = 1\nend\n");
        expect_no_offenses(COP, "[1].each do\n  if x\n    QUUX = 1\n  end\nend\n");
        expect_no_offenses(COP, "[1].each do\n  CORGE = 1\nensure\n  y\nend\n");
        expect_no_offenses(COP, "enums do\n  GRAULT = 1\nend\n");
    }
}

/// `Lint/MissingSuper`。`initialize` は「状態を持つ親クラスの中」でだけ、ライフサイクル
/// コールバックはクラス/モジュールの中なら常に対象。親を名指しするのは
/// `class Foo < Bar` と `Class.new(Bar) do ... end` の 2 つだけで、それ以外のブロックの
/// 中に書かれた `initialize` は親が見えないので対象外。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod missing_super {
    use super::*;

    const COP: &str = "Lint/MissingSuper";
    const CONSTRUCTOR: &str = "Call `super` to initialize state of the parent class.";
    const CALLBACK: &str = "Call `super` to invoke callback defined in the parent class.";

    #[test]
    fn a_constructor_of_a_subclass_has_to_call_super() {
        CopCase::new(
            COP,
            "class Foo < Bar\n  def initialize\n    @x = 1\n  end\nend\n",
            vec![Annotation::new(2, 3, 14, CONSTRUCTOR)],
        )
        .locations(&[(2, 3, 4, 5)])
        .lengths(&[31])
        .run();
        expect_no_offenses(
            COP,
            "class Grault < Bar\n  def initialize\n    super\n  end\nend\n",
        );
    }

    /// `Object` と `BasicObject` は状態を持たないので、そこから継いだ constructor は
    /// 呼ぶ先が無い。親を書いていないクラスも同じ。
    #[test]
    fn a_stateless_parent_or_none_at_all_needs_no_super() {
        expect_no_offenses(COP, "class Baz < Object\n  def initialize\n  end\nend\n");
        expect_no_offenses(COP, "class Qux\n  def initialize\n  end\nend\n");
    }

    /// `Class.new(Parent) do ... end` は親を名指ししているので対象。引数の無い
    /// `Class.new do ... end` は名指ししていない。
    #[test]
    fn a_class_new_block_names_its_parent_only_when_given_one() {
        CopCase::new(
            COP,
            "Class.new(Bar) do\n  def initialize\n  end\nend\n",
            vec![Annotation::new(2, 3, 14, CONSTRUCTOR)],
        )
        .locations(&[(2, 3, 3, 5)])
        .lengths(&[20])
        .run();
        expect_no_offenses(COP, "Class.new do\n  def initialize\n  end\nend\n");
    }

    /// ライフサイクルコールバックはクラスの中なら親の有無に関わらず対象。
    /// 特異メソッドとして書いても同じ。
    #[test]
    fn a_lifecycle_callback_is_reported_whatever_the_parent_is() {
        CopCase::new(
            COP,
            "class Corge\n  def self.inherited(sub)\n  end\n\n  def method_added(name)\n  end\nend\n",
            vec![Annotation::new(2, 3, 23, CALLBACK), Annotation::new(5, 3, 22, CALLBACK)],
        )
        .locations(&[(2, 3, 3, 5), (5, 3, 6, 5)])
        .lengths(&[29, 28])
        .run();
    }
}

/// `Lint/SuppressedException`。offense の範囲がこの cop の難所で、本家のノードは本体を
/// 導く `;` や `then` までは含み、その後ろのコメントや区切りの `;` は含まない。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod suppressed_exception {
    use super::*;

    const COP: &str = "Lint/SuppressedException";

    #[test]
    fn an_empty_rescue_is_reported_at_the_clause() {
        expect_offense(
            COP,
            r#"
            begin
              do_something
            rescue
            ^^^^^^ Do not suppress exceptions.
            end
            "#,
        );
    }

    /// `AllowNil` と `AllowComments` の既定。`nil` だけの本体と、`rescue` の次の行から
    /// `end` までにコメントがあるものは見逃す。行末コメントは `rescue` と同じ行なので
    /// 数えない。
    #[test]
    fn a_nil_body_or_a_comment_below_the_rescue_excuses_it() {
        expect_no_offenses(COP, "begin\n  do_something\nrescue\n  nil\nend\n");
        expect_no_offenses(
            COP,
            "begin\n  do_something\nrescue Foo => e\n  # handled\nend\n",
        );
        expect_offense(
            COP,
            r#"
            begin
              do_something
            rescue LoadError # trailing
            ^^^^^^^^^^^^^^^^ Do not suppress exceptions.
            end
            "#,
        );
    }

    /// 本体を導く `;` は本家のノードに含まれる。本体の中の `;` (空文) は含まれない。
    #[test]
    fn the_separator_that_introduces_the_body_is_part_of_the_clause() {
        expect_offense(
            COP,
            r#"
            begin
              a
            rescue; end
            ^^^^^^^ Do not suppress exceptions.
            "#,
        );
        expect_offense(
            COP,
            r#"
            begin
              a
            rescue EOFError
            ^^^^^^^^^^^^^^^ Do not suppress exceptions.
              ;
            end
            "#,
        );
    }
}

/// `Lint/Syntax` と NUL バイト。tree-sitter は生成レキサの終端番兵に NUL を予約して
/// いるので、ソースに書かれた NUL をトークンとして読めず、そこに構文エラーを立てる。
/// Ruby のレキサはコード中の NUL を入力の終わりとして扱い、コメント中の NUL は
/// コメントの本文として読み飛ばす。
///
/// 期待値はすべて本家 1.89.0 の `--only Lint/Syntax --format json` 実測。
mod syntax_nul_bytes {
    use super::*;

    const COP: &str = "Lint/Syntax";

    /// コード中の NUL から後ろは、何が書いてあっても読まれない。
    #[test]
    fn a_nul_written_in_code_ends_the_program() {
        expect_no_offenses(COP, "x = 1\n\0 this is ) not ( ruby at all\n");
    }

    /// コメント中の NUL はコメントの一部。行末までがコメントのままなので、
    /// 続く行はふつうに解析される。
    #[test]
    fn a_nul_written_in_a_comment_is_part_of_it() {
        expect_no_offenses(COP, "# comment \0 with nul\nx = 1\n");
    }

    /// BOM 無しの UTF-16 は Ruby から見ると 1 バイト目が NUL のファイル。先頭が
    /// NUL なら空のプログラムで、`#` で始まっていれば 1 行目がまるごとコメント。
    #[test]
    fn a_utf16_source_reads_as_an_empty_program_or_one_comment() {
        expect_no_offenses(COP, "\0#\0 \0h\0i\0\n");
        expect_no_offenses(COP, "#\0 \0h\0i\0\n\0p\0u\0t\0s\0\n");
    }
}

/// 整列系。期待値は本家 1.89.0 の `--only <cop>` の実出力から取った。
mod layout_alignment {
    use super::*;

    const HASH: &str = "Layout/HashAlignment";
    const ARGUMENT: &str = "Layout/ArgumentAlignment";
    const FIRST_HASH: &str = "Layout/FirstHashElementIndentation";
    const FIRST_ARRAY: &str = "Layout/FirstArrayElementIndentation";
    const KEY_MSG: &str = "Align the keys of a hash literal if they span more than one line.";
    const KWSPLAT_MSG: &str =
        "Align keyword splats with the rest of the hash if it spans more than one line.";
    const ARG_MSG: &str = "Align the arguments of a method call if they span more than one line.";

    /// 既定は `key` 揃え。行頭に来た要素だけが鍵の桁で比べられ、`**splat` は専用の
    /// メッセージで報告される。鍵と値の間の余分な空白も同じ offense に入る。
    #[test]
    fn keys_are_measured_against_the_first_pair() {
        CopCase::new(
            HASH,
            concat!(
                "h = {\n",
                "  a: 1,\n",
                "   bb: 2,\n",
                " c: 3,\n",
                "  **d,\n",
                "   **e,\n",
                "}\n",
                "r = {\n",
                "  'a'  => 1,\n",
                "  'b' =>  2,\n",
                "}\n",
            ),
            vec![
                Annotation::new(3, 4, 5, KEY_MSG),
                Annotation::new(4, 2, 4, KEY_MSG),
                Annotation::new(6, 4, 3, KWSPLAT_MSG),
                Annotation::new(9, 3, 9, KEY_MSG),
                Annotation::new(10, 3, 9, KEY_MSG),
            ],
        )
        .run();
    }

    /// 波括弧の無いハッシュ引数も本家では 1 つの `hash` にまとまる。`Hash[...]` は
    /// `[]` の呼び出しなので引数整列の対象で、`super` は別の node なので対象外。
    #[test]
    fn a_brace_less_hash_argument_is_one_hash() {
        CopCase::new(
            ARGUMENT,
            concat!(
                "foo :bar,\n",
                "  :baz,\n",
                "      :qux\n",
                "Hash[a: 1,\n",
                "  b: 2]\n",
                "bar(\n",
                "  1,\n",
                "    2\n",
                ")\n",
            ),
            vec![
                Annotation::new(2, 3, 4, ARG_MSG),
                Annotation::new(3, 7, 4, ARG_MSG),
                Annotation::new(5, 3, 4, ARG_MSG),
                Annotation::new(8, 5, 1, ARG_MSG),
            ],
        )
        .run();
        expect_no_offenses(ARGUMENT, "super a: 1,\n  b: 2\n");
    }

    /// 整列の autocorrect は要素が跨る行を丸ごと動かす。
    #[test]
    fn the_correction_moves_every_line_of_the_element() {
        expect_correction(
            HASH,
            concat!("h = {\n", "  a: 1,\n", "   bb: 2,\n", "}\n"),
            concat!("h = {\n", "  a: 1,\n", "  bb: 2,\n", "}\n"),
        );
        expect_correction(
            ARGUMENT,
            concat!("foo :bar,\n", "  :baz,\n", "      :qux\n"),
            concat!("foo :bar,\n", "    :baz,\n", "    :qux\n"),
        );
    }

    /// 先頭要素の字下げは、既定では括弧の直後 (`special_inside_parentheses`)、
    /// それ以外は左波括弧のある行の頭が基準。親のハッシュキーが基準になる形もある。
    #[test]
    fn the_first_element_is_measured_against_the_opening_line() {
        CopCase::new(
            FIRST_HASH,
            concat!(
                "x = {\n",
                "    a: 1,\n",
                "  }\n",
                "foo({\n",
                "  a: 1,\n",
                "})\n",
                "z = {\n",
                "  k: {\n",
                "      a: 1,\n",
                "    },\n",
                "  other: 2,\n",
                "}\n",
            ),
            vec![
                Annotation::new(
                    2,
                    5,
                    4,
                    "Use 2 spaces for indentation in a hash, relative to the start of the line \
                     where the left curly brace is.",
                ),
                Annotation::new(
                    3,
                    3,
                    1,
                    "Indent the right brace the same as the start of the line where the left \
                     brace is.",
                ),
                Annotation::new(
                    5,
                    3,
                    4,
                    "Use 2 spaces for indentation in a hash, relative to the first position \
                     after the preceding left parenthesis.",
                ),
                Annotation::new(
                    6,
                    1,
                    1,
                    "Indent the right brace the same as the first position after the preceding \
                     left parenthesis.",
                ),
                Annotation::new(
                    9,
                    7,
                    4,
                    "Use 2 spaces for indentation in a hash, relative to the parent hash key.",
                ),
                Annotation::new(
                    10,
                    5,
                    1,
                    "Indent the right brace the same as the parent hash key.",
                ),
            ],
        )
        .run();
    }

    /// 配列側も同じ形。`%w[]` も本家では array なので対象になる。
    #[test]
    fn an_array_and_a_percent_literal_share_the_rule() {
        CopCase::new(
            FIRST_ARRAY,
            concat!(
                "y = [\n",
                "    1,\n",
                "  ]\n",
                "w = %w[\n",
                "    a\n",
                "  ]\n",
            ),
            vec![
                Annotation::new(
                    2,
                    5,
                    1,
                    "Use 2 spaces for indentation in an array, relative to the start of the line \
                     where the left square bracket is.",
                ),
                Annotation::new(
                    3,
                    3,
                    1,
                    "Indent the right bracket the same as the start of the line where the left \
                     bracket is.",
                ),
                Annotation::new(
                    5,
                    5,
                    1,
                    "Use 2 spaces for indentation in an array, relative to the start of the line \
                     where the left square bracket is.",
                ),
                Annotation::new(
                    6,
                    3,
                    1,
                    "Indent the right bracket the same as the start of the line where the left \
                     bracket is.",
                ),
            ],
        )
        .run();
    }
}

/// `Layout/ArrayAlignment`。
mod layout_array_alignment {
    use super::*;

    const COP: &str = "Layout/ArrayAlignment";
    const MSG: &str = "Align the elements of an array literal if they span more than one line.";

    /// 括弧の有無を問わず「本家が array と呼ぶもの」が対象。多重代入の右辺だけは
    /// `masgn` の子なので除外される。
    #[test]
    fn every_shape_the_parser_calls_an_array_is_checked() {
        CopCase::new(
            COP,
            concat!(
                "x = [1,\n",
                "  2,\n",
                "      3]\n",
                "y = [\n",
                "  1,\n",
                "   2,\n",
                "]\n",
                "a, b = [1,\n",
                "  2]\n",
                "c = 1,\n",
                "  2\n",
            ),
            vec![
                Annotation::new(2, 3, 1, MSG),
                Annotation::new(3, 7, 1, MSG),
                Annotation::new(6, 4, 1, MSG),
                Annotation::new(11, 3, 1, MSG),
            ],
        )
        .run();
    }

    /// `return 1, 2` の値は array にならない。keyword の子のままなので対象外。
    #[test]
    fn the_values_of_a_return_are_not_an_array() {
        expect_no_offenses(COP, "def f\n  return 1,\n    2\nend\n");
    }

    /// `rescue A, B` の例外リストは括弧が無くても `array`。継続行は最初の例外の桁で
    /// 揃える。
    #[test]
    fn the_exception_list_of_a_rescue_is_an_array() {
        CopCase::new(
            COP,
            concat!(
                "begin\n",
                "  x\n",
                "rescue AAA, BBB,\n",
                "  CCC => e\n",
                "  y\n",
                "rescue DDD,\n",
                "       EEE\n",
                "  z\n",
                "end\n",
            ),
            vec![Annotation::new(4, 3, 3, MSG)],
        )
        .run();
    }

    #[test]
    fn the_correction_lines_the_elements_up_with_the_first() {
        expect_correction(
            COP,
            concat!("x = [1,\n", "  2,\n", "      3]\n"),
            concat!("x = [1,\n", "     2,\n", "     3]\n"),
        );
    }
}

/// 文字列まわりの残り 3 cop。
///
/// 期待値は本家 1.89.0 の `--format json` と `-A` の実測。
mod style_strings {
    use super::*;

    /// 補間の中の文字列は `Style/StringLiterals` ではなくこちらが見る。正規表現の
    /// 中の補間も対象で、コマンドリテラル (`` ` ``) の中だけは対象外。
    #[test]
    fn string_literals_in_interpolation() {
        expect_offense(
            "Style/StringLiteralsInInterpolation",
            r##"
            a = "#{"x"}"
                   ^^^ Prefer single-quoted strings inside interpolations.
            "##,
        );
        expect_no_offenses("Style/StringLiteralsInInterpolation", "b = \"#{'y'}\"\n");
        expect_no_offenses(
            "Style/StringLiteralsInInterpolation",
            "c = \"#{ \"it's\" }\"\n",
        );
        expect_no_offenses("Style/StringLiteralsInInterpolation", "g = `#{\"cmd\"}`\n");
        expect_correction(
            "Style/StringLiteralsInInterpolation",
            "d = /#{\"z\"}/\ne = :\"#{\"s\"}\"\n",
            "d = /#{'z'}/\ne = :\"#{'s'}\"\n",
        );
    }

    /// 既定は `annotated`。`%{foo}` はどこにあっても報告されるが、書き換えられる
    /// のは書式文字列として使われている literal だけ。
    #[test]
    fn format_string_token() {
        expect_offense(
            "Style/FormatStringToken",
            r#"
            c = "%{foo}"
                 ^^^^^^ Prefer annotated tokens (like `%<foo>s`) over template tokens (like `%{foo}`).
            "#,
        );
        expect_correction(
            "Style/FormatStringToken",
            "a = format(\"%{foo}\", foo: 1)\n",
            "a = format(\"%<foo>s\", foo: 1)\n",
        );
    }

    /// 注記のないトークンは `format` / `sprintf` / `printf` の第 1 引数か `%` の
    /// 受け手にあるときだけ、しかも `MaxUnannotatedPlaceholdersAllowed` を超えた
    /// ときだけ報告される。
    #[test]
    fn unannotated_tokens_need_a_format_context_and_a_crowd() {
        expect_no_offenses("Style/FormatStringToken", "b = \"%s %s\"\n");
        expect_no_offenses("Style/FormatStringToken", "f = format(\"%s\", x)\n");
        expect_no_offenses("Style/FormatStringToken", "i = /%{foo}/\n");
        expect_offense(
            "Style/FormatStringToken",
            r#"
            e = format("%s %s", x, y)
                        ^^ Prefer annotated tokens (like `%<foo>s`) over unannotated tokens (like `%s`).
                           ^^ Prefer annotated tokens (like `%<foo>s`) over unannotated tokens (like `%s`).
            "#,
        );
    }

    /// 既定の `prefer_alias` は、字句スコープの `alias_method` を keyword へ寄せ、
    /// `alias :a :b` のコロンを落とす。メソッドや `instance_eval` の中、
    /// グローバル変数の別名は対象外。
    #[test]
    fn alias_prefers_the_keyword_in_a_lexical_scope() {
        expect_offense(
            "Style/Alias",
            r#"
            alias_method :foo, :bar
            ^^^^^^^^^^^^ Use `alias` instead of `alias_method` at the top level.
            "#,
        );
        expect_offense(
            "Style/Alias",
            r#"
            class K
              alias_method :a, :b
              ^^^^^^^^^^^^ Use `alias` instead of `alias_method` in a class body.
            end
            "#,
        );
        expect_offense(
            "Style/Alias",
            r#"
            alias :foo :bar
                  ^^^^^^^^^ Use `alias foo bar` instead of `alias :foo :bar`.
            "#,
        );
        expect_no_offenses("Style/Alias", "alias foo bar\n");
        expect_no_offenses("Style/Alias", "alias $a $b\n");
        expect_no_offenses("Style/Alias", "def m\n  alias_method :a, :b\nend\n");
        expect_no_offenses("Style/Alias", "Class.new { alias_method :a, :b }\n");
        expect_correction(
            "Style/Alias",
            "alias :foo :bar\nalias_method :a, :b\n",
            "alias foo bar\nalias a b\n",
        );
    }

    /// `class << self` は `scope_type` が名前を挙げるどの型でもないので、外側が
    /// 何であるかで決まる。ブロックやメソッドの中なら keyword は使えない。
    ///
    /// 実測: rails の `activerecord/test/cases/adapter_test.rb:226` がこの形。
    #[test]
    fn a_singleton_class_does_not_stop_the_scope_walk() {
        expect_no_offenses(
            "Style/Alias",
            "def m\n  class << @c\n    alias_method :a, :b\n  end\nend\n",
        );
        expect_offense(
            "Style/Alias",
            r#"
            class K
              class << self
                alias_method :a, :b
                ^^^^^^^^^^^^ Use `alias` instead of `alias_method` in a class body.
              end
            end
            "#,
        );
    }
}

/// `Lint/RescueException`。offense の範囲は `rescue` 節そのもので、`Lint/SuppressedException`
/// と同じ端の取り方をする (`src/rules/lint/rescue_clause.rs` を共有)。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod rescue_exception {
    use super::*;

    const COP: &str = "Lint/RescueException";
    const MSG: &str =
        "Avoid rescuing the `Exception` class. Perhaps you meant to rescue `StandardError`?";

    /// 先頭に `::` が付いても同じ定数。名前空間が付いたものは別の定数。
    #[test]
    fn only_the_top_level_exception_class_counts() {
        CopCase::new(
            COP,
            "begin\n  a\nrescue Exception\n  b\nend\n",
            vec![Annotation::new(3, 1, 16, MSG)],
        )
        .locations(&[(3, 1, 4, 3)])
        .lengths(&[20])
        .run();
        CopCase::new(
            COP,
            "begin\n  a\nrescue ::Exception => e\n  b\nend\n",
            vec![Annotation::new(3, 1, 23, MSG)],
        )
        .locations(&[(3, 1, 4, 3)])
        .lengths(&[27])
        .run();
        expect_no_offenses(COP, "begin\n  a\nrescue Foo::Exception\n  b\nend\n");
    }

    /// 並べたうちの 1 つでも `Exception` なら対象。
    #[test]
    fn one_of_several_listed_classes_is_enough() {
        CopCase::new(
            COP,
            "begin\n  a\nrescue StandardError, Exception\n  b\nend\n",
            vec![Annotation::new(3, 1, 31, MSG)],
        )
        .locations(&[(3, 1, 4, 3)])
        .lengths(&[35])
        .run();
    }
}

/// `Lint/UnderscorePrefixedVariableName`。`_` を付けた名前を読んでいたら報告する。
/// ただし「読んだ」に数えるのは書かれた読みだけで、引数無しの `super` や `binding` が
/// 暗黙に読むものは数えない。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod underscore_prefixed_variable_name {
    use super::*;

    const COP: &str = "Lint/UnderscorePrefixedVariableName";
    const MSG: &str = "Do not use prefix `_` for a variable that is used.";

    #[test]
    fn an_underscored_name_that_is_read_is_reported() {
        expect_offense(
            COP,
            r#"
            def m(_foo)
                  ^^^^ Do not use prefix `_` for a variable that is used.
              _foo
            end
            "#,
        );
        expect_offense(
            COP,
            r#"
            _bar = 1
            ^^^^ Do not use prefix `_` for a variable that is used.
            puts _bar
            "#,
        );
    }

    /// 引数無しの `super` はメソッドの引数を暗黙に渡すだけで、書かれた読みではない。
    #[test]
    fn a_zero_arity_super_is_not_a_read_that_was_written() {
        expect_no_offenses(COP, "class A < B\n  def m(_foo)\n    super\n  end\nend\n");
    }

    /// 正規表現の名前付きキャプチャが作る変数は、名前ではなく正規表現リテラル全体を指す。
    #[test]
    fn a_named_capture_is_reported_at_the_regexp() {
        CopCase::new(
            COP,
            "/(?<_year>\\d+)/ =~ text\nputs _year\n",
            vec![Annotation::new(1, 1, 15, MSG)],
        )
        .run();
    }
}

/// `Lint/BooleanSymbol`。`:true` / `:false` を報告する。`%i[]` の要素は本家では
/// パーセントリテラルの配列として除外され、tree-sitter でもそこだけ別のノードになる。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod boolean_symbol {
    use super::*;

    const COP: &str = "Lint/BooleanSymbol";

    #[test]
    fn a_boolean_named_symbol_is_reported_in_every_spelling() {
        expect_offense(
            COP,
            r#"
            a = :true
                ^^^^^ Symbol with a boolean name - you probably meant to use `true`.
            "#,
        );
        expect_offense(
            COP,
            r#"
            c = { true: 1 }
                  ^^^^ Symbol with a boolean name - you probably meant to use `true`.
            "#,
        );
        expect_offense(
            COP,
            r#"
            f = :"false"
                ^^^^^^^^ Symbol with a boolean name - you probably meant to use `false`.
            "#,
        );
    }

    /// `%i[]` / `%I[]` の要素と、補間を含むものは対象外。キーワード引数の `true:` も
    /// シンボルではない。
    #[test]
    fn a_percent_literal_element_is_not_reported() {
        expect_no_offenses(COP, "i = %I[true]\n");
        expect_no_offenses(COP, "e = %i[true false]\n");
        expect_no_offenses(COP, "j = :\"tr#{x}ue\"\n");
        expect_no_offenses(COP, "def m(true: 1)\nend\n");
    }

    /// autocorrect はコロンを落とす。`true:` のキーはコロンが後ろに付いているので、
    /// ハッシュロケットに書き換える。
    #[test]
    fn correction_drops_the_colon_or_moves_it_to_a_hash_rocket() {
        expect_correction(COP, "a = :true\n", "a = true\n");
        expect_correction(COP, "c = { true: 1 }\n", "c = { true => 1 }\n");
        expect_correction(COP, "d = { :true => 1 }\n", "d = { true => 1 }\n");
        expect_correction(COP, "h = { \"true\": 1 }\n", "h = { \"true\" => 1 }\n");
    }
}

/// `Lint/LiteralInInterpolation`。offense はリテラルに付き、autocorrect は `#{}` ごと
/// リテラルが表す値へ置き換える。値の書き方が本家と 1 バイトでも違うと autocorrect が
/// 崩れるので、種類ごとに固定する。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod literal_in_interpolation {
    use super::*;

    const COP: &str = "Lint/LiteralInInterpolation";

    #[test]
    fn the_offense_covers_the_literal_not_the_interpolation() {
        expect_offense(
            COP,
            r##"
            x = "a#{1}b"
                    ^ Literal interpolation detected.
            "##,
        );
    }

    /// `#{}` に複数の文があれば、値になるのは最後の 1 つ。
    #[test]
    fn only_the_last_statement_is_the_value() {
        expect_offense(
            COP,
            r##"
            o = "#{1; 2}"
                      ^ Literal interpolation detected.
            "##,
        );
    }

    /// 呼び出しや括弧はリテラルではない。`__FILE__` と `__LINE__`、終端の無い範囲、
    /// 正規表現に直接書かれた配列も対象外。
    #[test]
    fn only_a_literal_that_prints_as_itself_is_reported() {
        expect_no_offenses(COP, "a = \"#{foo}\"\n");
        expect_no_offenses(COP, "n = \"#{(1)}\"\n");
        expect_no_offenses(COP, "a = \"#{__FILE__}\"\n");
        expect_no_offenses(COP, "b = \"#{__LINE__}\"\n");
        expect_no_offenses(COP, "c = \"#{1..}\"\n");
        expect_no_offenses(COP, "j = /a#{[1,2]}b/\n");
    }

    /// 値の書き方。数値は 10 進へ、シンボルは名前へ、ハッシュと配列は `to_s` の形へ。
    #[test]
    fn correction_writes_the_value_the_literal_stands_for() {
        expect_correction(COP, "a = \"#{-1}\"\n", "a = \"-1\"\n");
        expect_correction(COP, "c = \"#{1_000}\"\n", "c = \"1000\"\n");
        expect_correction(COP, "d = \"#{0b101}\"\n", "d = \"5\"\n");
        expect_correction(COP, "f = \"#{1e3}\"\n", "f = \"1000.0\"\n");
        expect_correction(COP, "g = \"#{:abc}\"\n", "g = \"abc\"\n");
        expect_correction(COP, "h = \"#{nil}\"\n", "h = \"\"\n");
        expect_correction(COP, "i = \"#{[1, [2, 3]]}\"\n", "i = \"[1, [2, 3]]\"\n");
        expect_correction(
            COP,
            "j = \"#{%w[a b]}\"\n",
            "j = \"[\\\"a\\\", \\\"b\\\"]\"\n",
        );
        expect_correction(
            COP,
            "k = \"#{{ a: { b: 1 } }}\"\n",
            "k = \"{:a=>{:b=>1}}\"\n",
        );
        expect_correction(
            COP,
            "l = \"#{{ 'x' => [1, :y] }}\"\n",
            "l = \"{\\\"x\\\"=>[1, :y]}\"\n",
        );
        expect_correction(COP, "m = \"#{'a\"b'}\"\n", "m = \"a\\\"b\"\n");
    }

    /// `?x` は本家のパーサでは 1 文字の `str` なので、文字列と同じくリテラル。
    /// エスケープは二重引用符の文字列と同じものが使え、書き戻しでは二重引用符
    /// だけを逃がす (`autocorrected_value_for_string` の非引用符側の枝)。
    #[test]
    fn a_character_literal_is_the_one_character_string_it_names() {
        expect_offense(
            COP,
            r##"
            a = "th#{?r}ee"
                     ^^ Literal interpolation detected.
            "##,
        );
        expect_correction(COP, "a = \"th#{?r}ee\"\n", "a = \"three\"\n");
        expect_correction(COP, "b = \"q#{?\"}q\"\n", "b = \"q\\\"q\"\n");
        expect_correction(COP, "c = \"n#{?\\n}n\"\n", "c = \"n\nn\"\n");
    }
}

/// `Style/CaseEquality`: `===` を直接書かない。
///
/// 期待値は本家 1.89.0 の `--format json` と `-A` の実測。
mod case_equality {
    use super::*;

    #[test]
    fn reports_the_operator_and_rewrites_what_it_can() {
        expect_offense(
            "Style/CaseEquality",
            r#"
            Integer === x
                    ^^^ Avoid the use of the case equality operator `===`.
            "#,
        );
        expect_correction(
            "Style/CaseEquality",
            "Integer === x\n(1..3) === y\nself.class === z\nArray === a + b\n",
            "x.is_a?(Integer)\n(1..3).include?(y)\nz.is_a?(self.class)\n(a + b).is_a?(Array)\n",
        );
    }

    /// 正規表現と、小文字を含まない定数 (値としての定数) は対象外。書き換えられ
    /// ない受け手は報告だけで、修正は付かない。
    #[test]
    fn a_regexp_and_a_screaming_constant_are_left_alone() {
        expect_no_offenses("Style/CaseEquality", "/re/ === w\n");
        expect_no_offenses("Style/CaseEquality", "FOO === v\n");
        CopCase::annotated(
            "Style/CaseEquality",
            r#"
            a === b
              ^^^ Avoid the use of the case equality operator `===`.
            "#,
        )
        .correctable(false)
        .run();
    }
}

/// `Style/IfUnlessModifier`: 単文の本体は条件の後ろへ、長すぎる修飾形はブロック形へ。
///
/// 期待値は本家 1.89.0 の `--only Style/IfUnlessModifier` と `-A` の実測。
mod if_unless_modifier {
    use super::*;

    const COP: &str = "Style/IfUnlessModifier";

    #[test]
    fn reports_the_keyword_of_a_single_statement_body() {
        expect_offense(
            COP,
            r#"
            if a
            ^^ Favor modifier `if` usage when having a single-line body. Another good alternative is the usage of control flow `&&`/`||`.
              b
            end
            "#,
        );
        expect_correction(COP, "if a\n  b\nend\n", "b if a\n");
        expect_correction(COP, "unless c\n  d\nend\n", "d unless c\n");
    }

    /// 大きな式の一部にいるときは、修飾形が結合を変えてしまうので括弧を促す。
    #[test]
    fn a_conditional_inside_a_larger_expression_asks_for_parentheses() {
        expect_offense(
            COP,
            r#"
            x = if e
                ^^ Favor modifier `if` usage when having a single-line body. Wrap the expression in parentheses to keep the current behavior, as it is part of a larger expression.
              f
            end
            "#,
        );
        expect_correction(COP, "x = if e\n  f\nend\n", "x = (f if e)\n");
    }

    /// 修飾形の行が長すぎるならブロック形へ戻す。
    #[test]
    fn a_modifier_that_makes_the_line_too_long_goes_back_to_block_form() {
        let source = "foo_bar_baz_qux(argument_one, argument_two, argument_three) \
                      if some_condition_that_is_rather_long && another_condition_here\n";
        expect_offense(
            COP,
            &format!(
                "{source}{}^^ Modifier form of `if` makes the line too long.\n",
                " ".repeat(60)
            ),
        );
        expect_correction(
            COP,
            source,
            "if some_condition_that_is_rather_long && another_condition_here\n  \
             foo_bar_baz_qux(argument_one, argument_two, argument_three)\nend\n",
        );
    }

    /// 長さの原因が行末のコメントだけなら、コメントを上の行へ移すだけで足りる。
    #[test]
    fn a_comment_that_is_what_made_the_line_too_long_moves_above_it() {
        expect_correction(
            COP,
            "do_something(with_an_argument) if condition_here \
             # a comment that is long enough to push this single line over the limit!\n",
            "# a comment that is long enough to push this single line over the limit!\n\
             do_something(with_an_argument) if condition_here\n",
        );
    }

    /// 免除される形。`defined?` の引数が未定義になり得るもの、複数文の本体、
    /// `else` を持つもの、条件が局所変数を束縛するもの、補間の中のもの。
    #[test]
    fn forms_the_modifier_cannot_carry_are_left_alone() {
        expect_no_offenses(COP, "if defined?(foo)\n  bar\nend\n");
        expect_no_offenses(COP, "if a\n  b\n  c\nend\n");
        expect_no_offenses(COP, "if a\n  b\nelse\n  c\nend\n");
        expect_no_offenses(COP, "unless (x, y = foo)\n  z\nend\n");
        expect_no_offenses(COP, "s = \"#{if a then b end}\"\n");
    }

    /// 本体に条件を抱えているものは対象外で、内側だけが報告される。
    #[test]
    fn only_the_innermost_of_two_nested_conditionals_is_reported() {
        expect_offense(
            COP,
            r#"
            if a
              if b
              ^^ Favor modifier `if` usage when having a single-line body. Another good alternative is the usage of control flow `&&`/`||`.
                c
              end
            end
            "#,
        );
        expect_correction(
            COP,
            "if a\n  if b\n    c\n  end\nend\n",
            "if a\n  c if b\nend\n",
        );
    }
}

/// `Style/GuardClause`: 抜けるだけの分岐で本体を包むのをやめ、ガード節にする。
///
/// 期待値は本家 1.89.0 の `--only Style/GuardClause` と `-A` の実測。
mod guard_clause {
    use super::*;

    const COP: &str = "Style/GuardClause";

    /// 定義の末尾に立つ条件は `return` のガードに置き換わる。修正は `end` を
    /// 消すだけなので、本体の字下げはそのまま残る。
    #[test]
    fn a_conditional_closing_a_definition_becomes_a_return_guard() {
        expect_offense(
            COP,
            r#"
            def foo
              bar
              if cond
              ^^ Use a guard clause (`return unless cond`) instead of wrapping the code inside a conditional expression.
                body
              end
            end
            "#,
        );
        expect_correction(
            COP,
            "def foo\n  bar\n  if cond\n    body\n  end\nend\n",
            "def foo\n  bar\n  return unless cond\n    body\n  \nend\n",
        );
    }

    /// `else` を持つ条件は、どちらかの枝が scope を抜けるときだけ対象。
    #[test]
    fn a_branch_that_leaves_the_scope_becomes_the_guard() {
        expect_offense(
            COP,
            r#"
            if c
            ^^ Use a guard clause (`raise "x" if c`) instead of wrapping the code inside a conditional expression.
              raise "x"
            else
              ok
            end
            "#,
        );
        expect_correction(
            COP,
            "if c\n  raise \"x\"\nelse\n  ok\nend\n",
            "raise \"x\" if c\n  \n\n  ok\n\n",
        );
        // `else` 側がガードなら条件が反転する。
        expect_offense(
            COP,
            r#"
            unless d
            ^^^^^^ Use a guard clause (`return if d`) instead of wrapping the code inside a conditional expression.
              a
            else
              return
            end
            "#,
        );
        expect_correction(
            COP,
            "unless d\n  a\nelse\n  return\nend\n",
            "return if d\n  a\n\n  \n\n",
        );
    }

    /// ガードが 1 行に収まらないときは、例示も修正も 3 行の形になる。
    #[test]
    fn a_guard_that_would_not_fit_on_one_line_is_written_over_three() {
        let condition = "a".repeat(110);
        let source = format!("def foo\n  bar\n  if {condition}\n    one\n    two\n  end\nend\n");
        expect_offense(
            COP,
            &format!(
                "def foo\n  bar\n  if {condition}\n  ^^ Use a guard clause (`unless {condition}; return; end`) instead of wrapping the code inside a conditional expression.\n    one\n    two\n  end\nend\n"
            ),
        );
        expect_correction(
            COP,
            &source,
            &format!(
                "def foo\n  bar\n  unless {condition}\n  return\nend\n    one\n    two\n  \nend\n"
            ),
        );
    }

    /// 免除される形。1 行しか無い本体、`elsif` の連なり、条件が束縛した局所変数を
    /// 本体が読むもの、複数行の条件、そして代入の右辺。
    #[test]
    fn forms_that_cannot_become_a_guard_are_left_alone() {
        expect_no_offenses(COP, "def foo\n  bar\n  if cond then body end\nend\n");
        expect_no_offenses(
            COP,
            "def foo\n  bar\n  if a\n    b\n  elsif c\n    d\n  end\nend\n",
        );
        expect_no_offenses(
            COP,
            "def foo\n  bar\n  if (x = compute)\n    use(x)\n  end\nend\n",
        );
        expect_no_offenses(
            COP,
            "def foo\n  bar\n  if a &&\n     b\n    c\n  end\nend\n",
        );
        expect_no_offenses(COP, "x = if c\n  raise 'x'\nelse\n  ok\nend\n");
    }
}

/// `Style/Next`: 反復の末尾を丸ごと包む条件は `next` で抜ける形にする。
///
/// 期待値は本家 1.89.0 の `--only Style/Next` と `-A` の実測。
mod next {
    use super::*;

    const COP: &str = "Style/Next";

    #[test]
    fn a_conditional_wrapping_the_tail_of_an_iteration_is_reported() {
        expect_offense(
            COP,
            r#"
            [1, 2].each do |a|
              if a == 1
              ^^^^^^^^^ Use `next` to skip iteration.
                puts a
                puts a
                puts a
              end
            end
            "#,
        );
        expect_correction(
            COP,
            "[1, 2].each do |a|\n  if a == 1\n    puts a\n    puts a\n    puts a\n  end\nend\n",
            "[1, 2].each do |a|\n  next unless a == 1\n  puts a\n  puts a\n  puts a\nend\n",
        );
    }

    /// `while` も反復。ブロックの側は列挙メソッドに限られる。
    #[test]
    fn a_loop_keyword_counts_as_an_iteration_too() {
        expect_offense(
            COP,
            r#"
            while x
              if y
              ^^^^ Use `next` to skip iteration.
                z
                z
                z
              end
            end
            "#,
        );
    }

    /// 免除される形。既定の `MinBodyLength` は 3 で、修飾形は既定の
    /// `skip_modifier_ifs` で見送られ、`else` を持つものは対象外。
    #[test]
    fn short_bodies_modifier_forms_and_else_branches_are_left_alone() {
        expect_no_offenses(
            COP,
            "[1, 2].each do |a|\n  if a == 1\n    puts a\n  end\nend\n",
        );
        expect_no_offenses(COP, "[1, 2].each { |a| puts a if a == 1 }\n");
        expect_no_offenses(
            COP,
            "[1, 2].reduce(0) do |a, b|\n  if a == 1\n    puts a\n    puts a\n  else\n    puts b\n  end\nend\n",
        );
        // 列挙メソッドではないブロックは反復ではない。
        expect_no_offenses(
            COP,
            "foo.bar do |a|\n  if a == 1\n    puts a\n    puts a\n    puts a\n  end\nend\n",
        );
    }
}

/// `Style/ParallelAssignment`: 値が本当に同時に動く必要が無いなら 1 行 1 代入。
///
/// 期待値は本家 1.89.0 の `--only Style/ParallelAssignment` と `-A` の実測。
mod parallel_assignment {
    use super::*;

    const COP: &str = "Style/ParallelAssignment";

    #[test]
    fn a_plain_parallel_assignment_is_split_into_one_line_each() {
        expect_offense(
            COP,
            r#"
            a, b = 1, 2
            ^^^^^^^^^^^ Do not use parallel assignment.
            "#,
        );
        expect_correction(COP, "a, b = 1, 2\n", "a = 1\nb = 2\n");
        // 右辺が配列リテラルでも同じ。
        expect_correction(COP, "j, k = [1, 2]\n", "j = 1\nk = 2\n");
    }

    /// 後の代入が前の値を読むときは、読む側が先に来るよう並べ替える。
    #[test]
    fn the_assignments_are_ordered_so_that_none_reads_an_overwritten_name() {
        expect_correction(COP, "e, f = 1, e\n", "f = e\ne = 1\n");
    }

    /// 修飾形の条件に包まれているときは、条件をブロックへ開いてから並べる。
    #[test]
    fn a_modifier_condition_is_opened_into_a_block() {
        expect_correction(COP, "m, n = 1, 2 if x\n", "if x\n  m = 1\n  n = 2\nend\n");
    }

    /// 免除される形。入れ替え (循環依存)、splat、左辺が 1 つだけのもの。
    #[test]
    fn a_swap_a_splat_and_a_single_name_are_left_alone() {
        expect_no_offenses(COP, "c, d = d, c\n");
        expect_no_offenses(COP, "g, h = *foo\n");
        expect_no_offenses(COP, "i = 1, 2\n");
    }
}

/// `Style/BlockDelimiters`: 1 行のブロックは波括弧、複数行は `do...end`。
///
/// 期待値は本家 1.89.0 の `--only Style/BlockDelimiters` と `-A` の実測。
mod block_delimiters {
    use super::*;

    const COP: &str = "Style/BlockDelimiters";

    #[test]
    fn a_single_line_do_end_becomes_braces() {
        expect_offense(
            COP,
            r#"
            each_with_index do |x| x end
                            ^^ Prefer `{...}` over `do...end` for single-line blocks.
            "#,
        );
        expect_correction(
            COP,
            "each_with_index do |x| x end\n",
            "each_with_index { |x| x }\n",
        );
    }

    #[test]
    fn a_multi_line_brace_block_becomes_do_end() {
        expect_offense(
            COP,
            r#"
            items.each { |x|
                       ^ Avoid using `{...}` for multi-line blocks.
              puts x
            }
            "#,
        );
        expect_correction(
            COP,
            "items.each { |x|\n  puts x\n}\n",
            "items.each do |x|\n  puts x\nend\n",
        );
    }

    /// `AllowedMethods` の既定は `lambda` / `proc` / `it`。括弧の無い引数に付いた
    /// ブロックは束縛が変わるので対象外。
    #[test]
    fn allowed_methods_and_blocks_bound_to_an_argument_are_left_alone() {
        expect_no_offenses(COP, "lambda do |x| x end\n");
        expect_no_offenses(COP, "foo bar do |x|\n  x\nend\n");
    }
}

/// 字下げ系。期待値は本家 1.89.0 の `--only <cop>` の実出力から取った。
mod layout_indentation {
    use super::*;

    const WIDTH: &str = "Layout/IndentationWidth";
    const CONSISTENCY: &str = "Layout/IndentationConsistency";
    const INCONSISTENT: &str = "Inconsistent indentation detected.";

    /// 補正はノードがまたぐ全行を一律にずらすので、入れ子になった 2 件が両方
    /// 補正すると内側の行が二重にずれる。本家は内側の corrector を捨て、外側の
    /// ずれが効いた次のパスで入れ子でなくなってから直す
    /// (`other_offense_in_same_range?`)。検出だけなら記録もしないので、offense は
    /// 全部 correctable のまま残る。
    ///
    /// 期待値は本家 1.89.0 の `--only Layout/IndentationWidth -A` の実出力。
    #[test]
    fn a_nested_offense_waits_for_the_pass_after_the_one_that_moves_it() {
        const NESTED: &str = concat!(
            "module M\n",
            "  private\n",
            "    def z\n",
            "      w\n",
            "    end\n",
            "\n",
            "    class C\n",
            "      def k\n",
            "        m\n",
            "      end\n",
            "\n",
            "      private\n",
            "        def a\n",
            "          x\n",
            "        end\n",
            "    end\n",
            "end\n",
        );
        expect_correction(
            WIDTH,
            NESTED,
            concat!(
                "module M\n",
                "  private\n",
                "  def z\n",
                "    w\n",
                "  end\n",
                "\n",
                "  class C\n",
                "    def k\n",
                "      m\n",
                "    end\n",
                "\n",
                "    private\n",
                "    def a\n",
                "      x\n",
                "    end\n",
                "  end\n",
                "end\n",
            ),
        );
        // 検査だけの実行では corrector を取り上げないので、3 件とも修正可能。
        let report = expect_offense(
            WIDTH,
            r#"
            module M
              private
                def z
            ^^^^ Use 2 (not 4) spaces for indentation.
                  w
                end

                class C
            ^^^^ Use 2 (not 4) spaces for indentation.
                  def k
                    m
                  end

                  private
                    def a
                ^^^^ Use 2 (not 4) spaces for indentation.
                      x
                    end
                end
            end
            "#,
        );
        assert!(
            report
                .offenses
                .iter()
                .all(sonicop::diagnostic::Offense::is_correctable),
            "検査だけの実行では corrector を取り上げない"
        );
    }

    /// 本家が handler を持つ節をひととおり。基準はそれぞれ `def` / `class` /
    /// `if` / `else` / `while` / `when` / `rescue` / `ensure` / ブロックの `end`。
    #[test]
    fn every_handler_of_the_upstream_cop_has_a_node_kind() {
        CopCase::new(
            WIDTH,
            concat!(
                "def foo\n",
                "    bar\n",
                "end\n",
                "class Baz\n",
                "      def a\n",
                "   b\n",
                "      end\n",
                "end\n",
                "if x\n",
                "      y\n",
                "else\n",
                "  z\n",
                "end\n",
                "while a\n",
                "   b\n",
                "end\n",
                "case q\n",
                "when 1\n",
                "      r\n",
                "else\n",
                "   s\n",
                "end\n",
                "begin\n",
                "   t\n",
                "rescue\n",
                "      u\n",
                "ensure\n",
                "   v\n",
                "end\n",
                "[1].each do |i|\n",
                "      i\n",
                "end\n",
            ),
            vec![
                Annotation::new(2, 1, 4, "Use 2 (not 4) spaces for indentation."),
                Annotation::new(5, 1, 6, "Use 2 (not 6) spaces for indentation."),
                // The range runs past the end of its line, which the caret notation cannot
                // show; `lengths` pins what the report actually carries.
                Annotation::new(6, 4, 1, "Use 2 (not -3) spaces for indentation."),
                Annotation::new(10, 1, 6, "Use 2 (not 6) spaces for indentation."),
                Annotation::new(15, 1, 3, "Use 2 (not 3) spaces for indentation."),
                Annotation::new(19, 1, 6, "Use 2 (not 6) spaces for indentation."),
                Annotation::new(21, 1, 3, "Use 2 (not 3) spaces for indentation."),
                Annotation::new(24, 1, 3, "Use 2 (not 3) spaces for indentation."),
                Annotation::new(26, 1, 6, "Use 2 (not 6) spaces for indentation."),
                Annotation::new(28, 1, 3, "Use 2 (not 3) spaces for indentation."),
                Annotation::new(31, 1, 6, "Use 2 (not 6) spaces for indentation."),
            ],
        )
        .lengths(&[4, 6, 3, 6, 3, 6, 3, 3, 6, 3, 6])
        .run();
    }

    /// 一貫性は文の並びごとに見る。先頭のアクセス修飾子が本体より深ければ、その桁が
    /// 基準になる。
    #[test]
    fn a_statement_list_is_measured_against_its_first_member() {
        let source = concat!(
            "class Foo\n",
            "  def a\n",
            "  end\n",
            "    def b\n",
            "    end\n",
            " def c\n",
            " end\n",
            "end\n",
            "module Bar\n",
            "  private\n",
            "\n",
            "    def d\n",
            "    end\n",
            "end\n",
        );
        CopCase::new(
            CONSISTENCY,
            source,
            vec![
                Annotation::new(4, 5, 5, INCONSISTENT),
                Annotation::new(6, 2, 5, INCONSISTENT),
                Annotation::new(12, 5, 5, INCONSISTENT),
            ],
        )
        .lengths(&[13, 10, 13])
        .run();
        CopCase::new(
            WIDTH,
            source,
            vec![
                Annotation::new(4, 1, 4, "Use 2 (not 4) spaces for indentation."),
                Annotation::new(6, 1, 1, "Use 2 (not 1) spaces for indentation."),
                Annotation::new(12, 1, 4, "Use 2 (not 4) spaces for indentation."),
            ],
        )
        .run();
    }

    /// `#{...}` の中身も本家では `begin` なので、複数文なら一貫性の対象になる。
    #[test]
    fn the_code_inside_an_interpolation_is_a_begin() {
        CopCase::new(
            CONSISTENCY,
            concat!("x = \"#{1 + 1\n", " 2 + 2}\"\n"),
            vec![Annotation::new(2, 2, 5, INCONSISTENT)],
        )
        .run();
    }

    /// autocorrect は要素が跨る行を丸ごと動かす。
    #[test]
    fn the_correction_moves_the_whole_member() {
        expect_correction(
            CONSISTENCY,
            concat!(
                "class Foo\n",
                "  def a\n",
                "  end\n",
                "    def b\n",
                "    end\n",
                "end\n"
            ),
            concat!(
                "class Foo\n",
                "  def a\n",
                "  end\n",
                "  def b\n",
                "  end\n",
                "end\n"
            ),
        );
        expect_correction(
            WIDTH,
            concat!("def foo\n", "    bar\n", "end\n"),
            concat!("def foo\n", "  bar\n", "end\n"),
        );
    }
}

/// `Style/CommentedKeyword`: キーワードと同じ行にコメントを置かない。
///
/// 期待値は本家 1.89.0 の `--only Style/CommentedKeyword` と `-A` の実測。
mod commented_keyword {
    use super::*;

    const COP: &str = "Style/CommentedKeyword";

    #[test]
    fn a_comment_on_a_keyword_line_moves_above_it() {
        expect_offense(
            COP,
            r#"
            def foo # comment
                    ^^^^^^^^^ Do not place comments on the same line as the `def` keyword.
              1
            end # another
                ^^^^^^^^^ Do not place comments on the same line as the `end` keyword.
            "#,
        );
        // `end` は説明する対象を持たないので、移さず落とす。
        expect_correction(
            COP,
            "def foo # comment\n  1\nend # another\n",
            "# comment\ndef foo\n  1\nend\n",
        );
    }

    /// `:nodoc:` と `:yields:`、`rubocop:` ディレクティブ、`steep:ignore` は免除。
    #[test]
    fn documentation_markers_and_directives_are_left_alone() {
        expect_no_offenses(COP, "class Bar # :nodoc:\nend\n");
        expect_no_offenses(COP, "def foo # rubocop:disable Style/For\n  1\nend\n");
        expect_no_offenses(COP, "def foo # steep:ignore\n  1\nend\n");
    }
}

/// `Style/GlobalVars`: 自前のグローバル変数を作らない。
///
/// 期待値は本家 1.89.0 の `--only Style/GlobalVars` の実測。
mod global_vars {
    use super::*;

    const COP: &str = "Style/GlobalVars";

    #[test]
    fn only_a_variable_the_interpreter_does_not_own_is_reported() {
        CopCase::annotated(
            COP,
            r#"
            $global = 1
            ^^^^^^^ Do not introduce global variables.
            "#,
        )
        .correctable(false)
        .run();
        // 組み込みのものと、`nth_ref` として読まれる `$1` は対象外。
        expect_no_offenses(COP, "$stdout.puts $1\n");
        expect_no_offenses(COP, "$LOAD_PATH << '.'\n");
    }
}

/// `Lint/IneffectiveAccessModifier`。`private` / `protected` は特異メソッドに効かない。
///
/// 期待値はすべて本家 1.89.0 の `--only Lint/IneffectiveAccessModifier --format json` の実測。
mod ineffective_access_modifier {
    use super::*;

    const COP: &str = "Lint/IneffectiveAccessModifier";
    const PRIVATE: &str = "`private` (on line 2) does not make singleton methods private. \
                           Use `private_class_method` or `private` inside a `class << self` \
                           block instead.";

    #[test]
    fn a_singleton_method_after_a_bare_private_is_reported_at_the_keyword() {
        CopCase::new(
            COP,
            "class C\n  private\n\n  def self.a\n  end\nend\n",
            vec![Annotation::new(4, 3, 3, PRIVATE)],
        )
        .severity(Severity::Warning)
        .correctable(false)
        .run();
    }

    /// `protected` は `class << self` の中に書けとだけ言う。
    #[test]
    fn protected_names_only_the_singleton_class_alternative() {
        CopCase::new(
            COP,
            "module M\n  protected\n  def self.a\n  end\nend\n",
            vec![Annotation::new(
                3,
                3,
                3,
                "`protected` (on line 2) does not make singleton methods protected. \
                 Use `protected` inside a `class << self` block instead.",
            )],
        )
        .run();
    }

    /// `module_function` は特異メソッドの可視性を変えるとは誰も思わないので対象外。
    /// `public` は既定の可視性なので、それ以降の定義は正しい可視性を持つ。
    #[test]
    fn module_function_and_public_are_not_ineffective() {
        expect_no_offenses(
            COP,
            "class C\n  module_function\n\n  def self.a\n  end\nend\n",
        );
        expect_no_offenses(COP, "class C\n  public\n\n  def self.a\n  end\nend\n");
        // `public` が救うのはその後ろの定義だけで、手前の定義は `private` のまま。
        CopCase::new(
            COP,
            "class C\n  private\n  def self.a; end\n  public\n  def self.b; end\nend\n",
            vec![Annotation::new(3, 3, 3, PRIVATE)],
        )
        .run();
    }

    /// `private_class_method` に**シンボルで**渡された名前は既に private なので除外される。
    /// 文字列は `method_name` (Symbol) と一致しないため除外されない。
    #[test]
    fn only_a_symbol_passed_to_private_class_method_exempts_the_definition() {
        expect_no_offenses(
            COP,
            "class C\n  private\n  private_class_method :a\n  def self.a\n  end\nend\n",
        );
        CopCase::new(
            COP,
            "class C\n  private\n  private_class_method 'a'\n  def self.a\n  end\nend\n",
            vec![Annotation::new(4, 3, 3, PRIVATE)],
        )
        .run();
    }

    /// `begin ... end` の中は同じ修飾子を引き継いで走査するが、`if` の中は見ない。
    #[test]
    fn a_kwbegin_is_walked_and_a_conditional_is_not() {
        CopCase::new(
            COP,
            "class C\n  private\n  begin\n    def self.a\n    end\n  end\nend\n",
            vec![Annotation::new(4, 5, 3, PRIVATE)],
        )
        .run();
        expect_no_offenses(
            COP,
            "class C\n  private\n  if x\n    def self.a\n    end\n  end\nend\n",
        );
    }

    /// 本体が 1 文だけのクラスは `begin` にならないので走査されない。
    /// `class << self` の中では修飾子が効くので、そもそも対象外。
    #[test]
    fn a_single_statement_body_and_a_singleton_class_are_left_alone() {
        expect_no_offenses(COP, "class C\n  def self.a; end\nend\n");
        expect_no_offenses(
            COP,
            "class C\n  class << self\n    private\n    def a; end\n  end\nend\n",
        );
    }
}

/// `Lint/UselessAccessModifier`。可視性の状態機械を本家の
/// `check_child_nodes` / `check_new_visibility` どおりに追う。
///
/// 期待値はすべて本家 1.89.0 の `--only Lint/UselessAccessModifier --format json` の実測。
mod useless_access_modifier {
    use super::*;

    const COP: &str = "Lint/UselessAccessModifier";

    #[test]
    fn a_modifier_repeating_the_current_visibility_is_reported() {
        expect_offense(
            COP,
            r#"
            class Foo
              public
              ^^^^^^ Useless `public` access modifier.
              def a; end
            end
            "#,
        );
        expect_offense(
            COP,
            r#"
            class Foo
              private
              def a; end
              private
              ^^^^^^^ Useless `private` access modifier.
              def b; end
            end
            "#,
        );
    }

    /// 何も定義しないまま次の修飾子が来たら、**前の**修飾子が報告される。
    /// 走査の最後に残った修飾子も同じ扱い。
    #[test]
    fn the_modifier_left_with_nothing_to_govern_is_the_one_reported() {
        expect_offense(
            COP,
            r#"
            class Foo
              protected
              ^^^^^^^^^ Useless `protected` access modifier.
              private
              def a; end
            end
            "#,
        );
        expect_offense(
            COP,
            r#"
            class Foo
              private
              ^^^^^^^ Useless `private` access modifier.
            end
            "#,
        );
    }

    /// 特異メソッドは可視性を受け取らないので、`private` は依然として宙に浮く。
    /// `define_method` と `attr_*` はメソッドを作るので浮かない。
    #[test]
    fn what_counts_as_defining_a_method() {
        expect_offense(
            COP,
            r#"
            class Foo
              private
              ^^^^^^^ Useless `private` access modifier.
              def self.a; end
            end
            "#,
        );
        expect_no_offenses(COP, "class Foo\n  private\n  define_method(:a) {}\nend\n");
        expect_no_offenses(COP, "class Foo\n  private\n  attr_reader :a\nend\n");
        expect_no_offenses(
            COP,
            "class Foo\n  private\n  if x\n    def a; end\n  end\nend\n",
        );
    }

    /// トップレベルの修飾子は何も変えないので、必ず報告される。ただし
    /// `on_begin` は根の `begin` にしか反応しないので、文が 1 つだけのファイルは対象外。
    #[test]
    fn a_top_level_modifier_is_always_useless() {
        expect_offense(
            COP,
            r#"
            private
            ^^^^^^^ Useless `private` access modifier.

            def a; end
            "#,
        );
        expect_no_offenses(COP, "private\n");
    }

    /// `class_eval` / `instance_eval` / クラスコンストラクタのブロックは新しい可視性の
    /// スコープを開く。素のブロックは開かないので、外側の状態がそのまま続く。
    #[test]
    fn only_some_blocks_open_a_new_scope() {
        expect_offense(
            COP,
            r#"
            class Foo
              class_eval do
                private
                ^^^^^^^ Useless `private` access modifier.
              end
              def a; end
            end
            "#,
        );
        expect_offense(
            COP,
            r#"
            class Foo
              ::Class.new do
                private
                ^^^^^^^ Useless `private` access modifier.
              end
              def a; end
            end
            "#,
        );
        expect_no_offenses(
            COP,
            "class Foo\n  [1].each do\n    private\n  end\n  def a; end\nend\n",
        );
        expect_no_offenses(
            COP,
            "class Foo\n  Foo::Class.new do\n    private\n  end\n  def a; end\nend\n",
        );
    }

    /// 引数付きの `private_class_method` は上流で `nil` を返し、可視性と保留中の修飾子を
    /// どちらも捨てる。引数無しならそれ自体が報告される。
    #[test]
    fn private_class_method_resets_the_state_when_it_has_arguments() {
        expect_no_offenses(
            COP,
            "class Foo\n  private\n  private_class_method :a\n  private\n  def b; end\nend\n",
        );
        expect_offense(
            COP,
            r#"
            class Foo
              private_class_method
              ^^^^^^^^^^^^^^^^^^^^ Useless `private_class_method` access modifier.
              def a; end
            end
            "#,
        );
        // 本体が 1 文のクラスは `bare_access_modifier?` だけを見るので、
        // `private_class_method` は素通りする。
        expect_no_offenses(COP, "class Foo\n  private_class_method\nend\n");
    }

    /// autocorrect は修飾子が乗っている行を丸ごと、行末の改行ごと消す。
    #[test]
    fn the_correction_removes_the_whole_line() {
        expect_correction(
            COP,
            "class Foo\n  public\n  def a; end\nend\n",
            "class Foo\n  def a; end\nend\n",
        );
        expect_correction(
            COP,
            "class G\n  private\n  def a; end\n  x; private\nend\n",
            "class G\n  private\n  def a; end\nend\n",
        );
    }
}

/// `Lint/UselessMethodDefinition`。`super` へ丸投げするだけの定義を報告する。
///
/// 期待値はすべて本家 1.89.0 の `--only Lint/UselessMethodDefinition --format json` の実測。
mod useless_method_definition {
    use super::*;

    const COP: &str = "Lint/UselessMethodDefinition";
    const MSG: &str = "Useless method definition detected.";

    #[test]
    fn a_body_that_is_only_super_is_reported_over_the_whole_definition() {
        CopCase::new(
            COP,
            "class A\n  def a\n    super\n  end\nend\n",
            vec![Annotation::new(2, 3, 5, MSG)],
        )
        .lengths(&[21])
        .severity(Severity::Warning)
        .correctable(true)
        .run();
    }

    /// 引数付きの `super` は、渡した引数の**ソース**が仮引数のそれと一致したときだけ委譲。
    #[test]
    fn arguments_are_compared_by_source() {
        CopCase::new(
            COP,
            "class A\n  def a(x)\n    super(x)\n  end\nend\n",
            vec![Annotation::new(2, 3, 8, MSG)],
        )
        .lengths(&[27])
        .run();
        expect_no_offenses(COP, "class A\n  def a(x)\n    super(y)\n  end\nend\n");
        // `x:` と `x: x` は違うソースなので委譲ではない。
        expect_no_offenses(COP, "class A\n  def a(x:)\n    super(x: x)\n  end\nend\n");
    }

    /// `*args` / `x = 1` / `x: 1` を取るメソッドは、親と同じ呼び方ができるとは限らない。
    #[test]
    fn rest_and_optional_parameters_are_exempt() {
        expect_no_offenses(COP, "class A\n  def a(*x)\n    super\n  end\nend\n");
        expect_no_offenses(COP, "class A\n  def a(x = 1)\n    super\n  end\nend\n");
        expect_no_offenses(COP, "class A\n  def a(x: 1)\n    super\n  end\nend\n");
        // `x:` は `kwarg` であって `kwoptarg` ではない。
        CopCase::new(
            COP,
            "class A\n  def a(x:)\n    super\n  end\nend\n",
            vec![Annotation::new(2, 3, 9, MSG)],
        )
        .lengths(&[25])
        .run();
    }

    /// `super` にブロックが付くと上流では `block` ノードになり、委譲ではなくなる。
    /// `super.foo` も同様に `send` の受け手でしかない。
    #[test]
    fn super_with_a_block_or_a_receiver_is_not_a_delegation() {
        expect_no_offenses(
            COP,
            "class A\n  def a\n    super do\n      1\n    end\n  end\nend\n",
        );
        expect_no_offenses(COP, "class A\n  def a\n    super.foo\n  end\nend\n");
        expect_no_offenses(COP, "class A\n  def a\n    super\n    other\n  end\nend\n");
    }

    /// マクロの引数として書かれた定義はそのマクロの領分。アクセス修飾子だけは例外で、
    /// 報告は `def` を指し、修正は修飾子ごと消す。
    #[test]
    fn a_definition_handed_to_a_macro_is_left_alone_unless_the_macro_is_an_access_modifier() {
        expect_no_offenses(COP, "class A\n  memoize def a\n    super\n  end\nend\n");
        CopCase::new(
            COP,
            "class A\n  private def a\n    super\n  end\nend\n",
            vec![Annotation::new(2, 11, 5, MSG)],
        )
        .lengths(&[21])
        .run();
        expect_correction(
            COP,
            "class A\n  private def a\n    super\n  end\nend\n",
            "class A\n  \nend\n",
        );
    }

    /// `super()` は引数 0 個の `super` なので、引数を取らない定義とは一致する。
    #[test]
    fn an_explicitly_empty_super_matches_a_definition_without_parameters() {
        CopCase::new(
            COP,
            "class A\n  def a\n    super()\n  end\nend\n",
            vec![Annotation::new(2, 3, 5, MSG)],
        )
        .lengths(&[23])
        .run();
    }
}

/// `Lint/BinaryOperatorWithIdenticalOperands`。左右が**同じノード**かを見る。
/// 本家は `Node#==` で構造比較するので、綴りの違いは差にならない。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod binary_operator_with_identical_operands {
    use super::*;

    const COP: &str = "Lint/BinaryOperatorWithIdenticalOperands";

    #[test]
    fn comparison_operators_with_the_same_operands_are_reported() {
        expect_offense(
            COP,
            r#"
            x = a == a
                ^^^^^^ Binary operator `==` has identical operands.
            "#,
        );
        expect_offense(
            COP,
            r#"
            x = a.b <=> a.b
                ^^^^^^^^^^^ Binary operator `<=>` has identical operands.
            "#,
        );
    }

    /// `&&` / `||` / `and` / `or` は `on_and` / `on_or` の担当。演算子はソースの綴りで出る。
    #[test]
    fn logical_operators_report_the_spelling_that_was_used() {
        expect_offense(
            COP,
            r#"
            x = (a and a)
                 ^^^^^^^ Binary operator `and` has identical operands.
            "#,
        );
        expect_offense(
            COP,
            r#"
            x = a && a
                ^^^^^^ Binary operator `&&` has identical operands.
            "#,
        );
        // `a or a` は `(x = a) or a` と解釈されるので、左右は同じではない。
        expect_no_offenses(COP, "x = a or a\n");
    }

    /// 算術演算子は対象外。左右が違えばもちろん報告しない。
    #[test]
    fn arithmetic_and_differing_operands_are_left_alone() {
        expect_no_offenses(COP, "x = a + a\n");
        expect_no_offenses(COP, "x = a == b\n");
        expect_no_offenses(COP, "x = a&.b == a.b\n");
    }

    /// リテラルは本家のパーサが解決した**値**で比較される。綴りが違っても同じ値なら同じノード。
    #[test]
    fn literals_are_compared_by_the_value_the_parser_resolved() {
        for source in [
            "x = (?a == \"a\")\n",
            "x = ('a' == \"a\")\n",
            "x = (0x10 == 16)\n",
            "x = (:ruby == :\"ruby\")\n",
            "x = (-0.0 <=> 0.0)\n",
            "x = (?\\C-a == \"\\1\")\n",
        ] {
            CopCase::new(
                COP,
                source,
                vec![Annotation::new(
                    1,
                    6,
                    source.trim_end().len() - 6,
                    format!(
                        "Binary operator `{}` has identical operands.",
                        if source.contains("<=>") { "<=>" } else { "==" }
                    ),
                )],
            )
            .run();
        }
        // `1` と `1.0` は別のリテラル。
        expect_no_offenses(COP, "x = (1 == 1.0)\n");
    }

    /// ドット付きの演算子呼び出しも `binary_operation?` を満たす。
    #[test]
    fn the_dotted_spelling_of_an_operator_is_still_a_binary_operation() {
        expect_offense(
            COP,
            r#"
            x = obj.<=>(obj)
                ^^^^^^^^^^^^ Binary operator `<=>` has identical operands.
            "#,
        );
        // 演算子でないメソッドは対象外。`scope.or(scope)` は比較ではない。
        expect_no_offenses(COP, "x = obj.or(obj)\n");
    }
}

/// `Lint/EmptyFile` / `Lint/EmptyWhen`。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod empty_file_and_when {
    use super::*;

    const EMPTY_FILE: &str = "Lint/EmptyFile";
    const EMPTY_WHEN: &str = "Lint/EmptyWhen";
    const WHEN_MSG: &str = "Avoid `when` branches without a body.";

    /// `add_global_offense` はファイル先頭の長さ 0 のレンジ。
    #[test]
    fn only_a_file_with_nothing_in_it_is_reported() {
        CopCase::new(
            EMPTY_FILE,
            "",
            vec![Annotation::new(1, 1, 0, "Empty file detected.")],
        )
        .lengths(&[0])
        .severity(Severity::Warning)
        .correctable(false)
        .run();
        // `AllowComments` は既定で真なので、コメントだけのファイルは対象外。
        expect_no_offenses(EMPTY_FILE, "# just a comment\n");
        expect_no_offenses(EMPTY_FILE, "\n\n");
    }

    /// `when` のレンジは最後の条件で終わる。`;` や `then` は本体の側に属する。
    #[test]
    fn the_reported_range_stops_at_the_last_condition() {
        CopCase::new(
            EMPTY_WHEN,
            "case a\nwhen 1 then\nend\n",
            vec![Annotation::new(2, 1, 6, WHEN_MSG)],
        )
        .severity(Severity::Warning)
        .correctable(false)
        .run();
        CopCase::new(
            EMPTY_WHEN,
            "case c\nwhen foo, bar;\nend\n",
            vec![Annotation::new(2, 1, 13, WHEN_MSG)],
        )
        .run();
    }

    /// `AllowComments` が既定で真なので、コメントのある枝は見逃される。枝の範囲は
    /// 次の枝が始まる行の**手前**まで。
    #[test]
    fn a_branch_that_explains_itself_is_allowed() {
        expect_no_offenses(
            EMPTY_WHEN,
            "case y\nwhen 2 then\n  # why\nwhen 3\n  z\nend\n",
        );
        expect_no_offenses(EMPTY_WHEN, "case w\nwhen 4\n  # why\nend\n");
        // 次の枝の行にあるコメントはその枝のもの。
        CopCase::new(
            EMPTY_WHEN,
            "case y\nwhen 2\nwhen 3 # why\n  z\nend\n",
            vec![Annotation::new(2, 1, 6, WHEN_MSG)],
        )
        .run();
    }
}

/// `Lint/InheritException`。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod inherit_exception {
    use super::*;

    const COP: &str = "Lint/InheritException";
    const MSG: &str = "Inherit from `StandardError` instead of `Exception`.";

    #[test]
    fn a_superclass_or_a_class_new_argument_is_reported() {
        expect_offense(
            COP,
            r#"
            class C < Exception; end
                      ^^^^^^^^^ Inherit from `StandardError` instead of `Exception`.
            "#,
        );
        expect_offense(
            COP,
            r#"
            E = Class.new(Exception)
                          ^^^^^^^^^ Inherit from `StandardError` instead of `Exception`.
            "#,
        );
        // `::` を付けても同じ定数。
        CopCase::new(
            COP,
            "class D < ::Exception; end\n",
            vec![Annotation::new(1, 11, 11, MSG)],
        )
        .run();
    }

    /// 名前空間の付いた `Exception` は別の定数。`Class` 以外のコンストラクタも対象外。
    #[test]
    fn a_namespaced_constant_is_a_different_class() {
        expect_no_offenses(COP, "class C < Foo::Exception; end\n");
        expect_no_offenses(COP, "class C < StandardError; end\n");
        expect_no_offenses(COP, "E = Foo::Class.new(Exception)\n");
        expect_no_offenses(COP, "E = Class.new(Exception, 1)\n");
    }

    /// 同じ本体で先に `Exception` が定義されていれば、修飾無しの `Exception` はそれを指す。
    /// `::` を書いた場合はトップレベルの `Exception` なので報告される。
    #[test]
    fn a_locally_defined_exception_shadows_the_built_in_one() {
        expect_no_offenses(COP, "class Exception; end\nclass C < Exception; end\n");
        CopCase::new(
            COP,
            "class Exception; end\nclass C < ::Exception; end\n",
            vec![Annotation::new(2, 11, 11, MSG)],
        )
        .run();
    }

    /// autocorrect は定数を置き換える。`EnforcedStyle: runtime_error` なら `RuntimeError`。
    #[test]
    fn the_correction_replaces_the_constant() {
        expect_correction(
            COP,
            "class C < Exception; end\n",
            "class C < StandardError; end\n",
        );
        CopCase::new(COP, "class C < Exception; end\n", Vec::new())
            .without_offense_check()
            .config("Lint/InheritException:\n  EnforcedStyle: runtime_error\n")
            .corrected("class C < RuntimeError; end\n")
            .run();
    }
}

/// `Lint/RaiseException`。`raise` / `fail` が `Exception` を投げるのを禁じる。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod raise_exception {
    use super::*;

    const COP: &str = "Lint/RaiseException";
    const MSG: &str = "Use `StandardError` over `Exception`.";

    #[test]
    fn the_class_itself_and_a_new_instance_are_both_reported() {
        expect_offense(
            COP,
            r#"
            raise Exception, 'boom'
                  ^^^^^^^^^ Use `StandardError` over `Exception`.
            "#,
        );
        expect_offense(
            COP,
            r#"
            fail Exception.new('boom')
                 ^^^^^^^^^ Use `StandardError` over `Exception`.
            "#,
        );
        // `Exception.new` を渡す形は引数 1 個のときだけ。
        expect_no_offenses(COP, "raise Exception.new('a'), 'b'\n");
        expect_no_offenses(COP, "raise Foo::Exception\n");
        expect_no_offenses(COP, "obj.raise Exception\n");
    }

    /// `AllowedImplicitNamespaces` (既定は `Gem`) の中では、修飾無しの `Exception` は
    /// そのモジュールのものを指しうるので見逃す。`::` を書けば別。
    #[test]
    fn an_allowed_namespace_hides_the_unqualified_name() {
        expect_no_offenses(COP, "module Gem\n  raise Exception\nend\n");
        expect_no_offenses(
            COP,
            "module Gem\n  module Inner\n    raise Exception\n  end\nend\n",
        );
        CopCase::new(
            COP,
            "module Gem\n  raise ::Exception\nend\n",
            vec![Annotation::new(2, 9, 11, MSG)],
        )
        .run();
        CopCase::new(
            COP,
            "module Other\n  raise Exception\nend\n",
            vec![Annotation::new(2, 9, 9, MSG)],
        )
        .run();
    }

    /// autocorrect は `::` の有無を保つ。
    #[test]
    fn the_correction_keeps_the_leading_colons() {
        expect_correction(COP, "raise Exception\n", "raise StandardError\n");
        expect_correction(COP, "raise ::Exception\n", "raise ::StandardError\n");
    }
}
/// `Lint/HashCompareByIdentity`。`object_id` を鍵にしたハッシュ操作を禁じる。
///
/// 期待値はすべて本家 1.89.0 の実測。
mod hash_compare_by_identity {
    use super::*;
    /// `Style/EmptyMethod`: 空の定義は 1 行にまとめる。
    ///
    /// 期待値は本家 1.89.0 の `--only Style/EmptyMethod` と `-A` の実測。
    mod empty_method {
        use super::*;

        /// `Layout/EmptyLines` と `Layout/EmptyLineBetweenDefs`。
        ///
        /// 期待値は本家 1.89.0 の `--only <cop> --format json` と `-A` の実測。
        mod layout_empty_line_runs {
            use super::*;

            /// 空行が 2 行続いたら 2 行目以降を報告する。文字列やヒアドキュメントの中の
            /// 空行は行ごとに token を持つので対象外。`=begin` ブロックは token が 1 個
            /// しかないため、中の空行は報告される。
            #[test]
            fn only_runs_of_blank_lines_outside_a_literal_are_reported() {
                expect_no_offenses("Layout/EmptyLines", "a = 1\n\nb = 2\n");
                expect_no_offenses("Layout/EmptyLines", "x = \"a\n\n\nb\"\n");
                expect_no_offenses("Layout/EmptyLines", "y = <<~FOO\n  a\n\n\n  b\nFOO\n");
                CopCase::new(
                    "Layout/EmptyLines",
                    "a = 1\n\n\n\nb = 2\n",
                    vec![
                        Annotation::new(3, 1, 0, "Extra blank line detected."),
                        Annotation::new(4, 1, 0, "Extra blank line detected."),
                    ],
                )
                .run();
                CopCase::new(
                    "Layout/EmptyLines",
                    "a = 1\n=begin\n\n\n=end\nb = 2\n",
                    vec![Annotation::new(4, 1, 0, "Extra blank line detected.")],
                )
                .run();
            }

            /// `__END__` の後ろは字句解析されないので、data セクションの空行は数えない。
            #[test]
            fn a_data_section_holds_no_tokens() {
                expect_no_offenses("Layout/EmptyLines", "x = 1\n__END__\n\n\ntext\n");
            }

            /// コメントも token なので、コメント行の後ろの空行 2 行は報告される。
            #[test]
            fn comments_count_as_tokens() {
                CopCase::new(
                    "Layout/EmptyLines",
                    "a = 1\n# c\n\n\n# d\nb = 2\n",
                    vec![Annotation::new(4, 1, 0, "Extra blank line detected.")],
                )
                .run();
            }

            #[test]
            fn empty_lines_correction_leaves_one_blank_line() {
                expect_correction(
                    "Layout/EmptyLines",
                    "a = 1\n\n\n\nb = 2\n",
                    "a = 1\n\nb = 2\n",
                );
            }

            /// 1 行 def が隣り合っているのは既定で許され、間にコメントを挟んだ空行 2 群は
            /// 「複数の空行グループ」として見逃される。
            #[test]
            fn adjacent_one_liners_and_split_blank_runs_are_left_alone() {
                expect_no_offenses("Layout/EmptyLineBetweenDefs", "def a; end\ndef b; end\n");
                expect_no_offenses(
                    "Layout/EmptyLineBetweenDefs",
                    "def a\nend\n\n# c\n\ndef b\nend\n",
                );
                expect_no_offenses("Layout/EmptyLineBetweenDefs", "def a\nend\n\ndef b\nend\n");
            }

            /// class / module の定義も既定で対象。`class << self` は sclass なので対象外。
            #[test]
            fn classes_and_modules_are_checked_but_singleton_classes_are_not() {
                CopCase::new(
                    "Layout/EmptyLineBetweenDefs",
                    "class A\nend\nclass B\nend\n",
                    vec![Annotation::new(
                        3,
                        1,
                        7,
                        "Expected 1 empty line between class definitions; found 0.",
                    )],
                )
                .run();
                expect_no_offenses(
                    "Layout/EmptyLineBetweenDefs",
                    "class Foo\n  class << self\n  end\n  class << Foo\n  end\nend\n",
                );
            }

            /// 空行が多すぎるときは削り、足りないときは足す。
            #[test]
            fn between_defs_correction_adds_and_removes_blank_lines() {
                expect_correction(
                    "Layout/EmptyLineBetweenDefs",
                    "def a\nend\ndef b\nend\n\n\n\ndef c\nend\n",
                    "def a\nend\n\ndef b\nend\n\ndef c\nend\n",
                );
            }
        }

        /// `Layout/SpaceInLambdaLiteral`、`Layout/EmptyLinesAroundAttributeAccessor`、
        /// `Layout/DotPosition`。
        ///
        /// 期待値は本家 1.89.0 の `--only <cop> --format json` と `-A` の実測。
        mod layout_lambda_accessor_dot {
            use super::*;

            /// 引数の無い `-> () { }` は上流では引数が空なので対象外。空白が無ければ何も
            /// 言わない。括弧の無い `-> x, y { }` の空白も報告される。
            #[test]
            fn only_a_lambda_with_parameters_is_measured() {
                expect_no_offenses("Layout/SpaceInLambdaLiteral", "a = ->(x) { x }\n");
                expect_no_offenses("Layout/SpaceInLambdaLiteral", "a = -> () { 1 }\n");
                expect_no_offenses("Layout/SpaceInLambdaLiteral", "a = lambda { |x| x }\n");
                CopCase::new(
                    "Layout/SpaceInLambdaLiteral",
                    "a = -> x, y { x }\n",
                    vec![Annotation::new(
                        1,
                        7,
                        1,
                        "Do not use spaces between `->` and `(` in lambda literals.",
                    )],
                )
                .run();
            }

            #[test]
            fn lambda_correction_removes_the_whole_run_of_spaces() {
                expect_correction(
                    "Layout/SpaceInLambdaLiteral",
                    "f = ->  (x, y) { x }\n",
                    "f = ->(x, y) { x }\n",
                );
            }

            /// 次に来るのが別のアクセサ、`alias`、`AllowedMethods` のメソッドなら 1 群として
            /// 扱う。引数の無い `attr_reader` はそもそもアクセサではない。
            #[test]
            fn an_accessor_group_needs_no_blank_line_inside_it() {
                expect_no_offenses(
                    "Layout/EmptyLinesAroundAttributeAccessor",
                    "class Foo\n  attr_reader :a\n  attr_writer :b\n\n  def c; end\nend\n",
                );
                expect_no_offenses(
                    "Layout/EmptyLinesAroundAttributeAccessor",
                    "class Foo\n  attr_reader :a\n  alias :b :a\n\n  def c; end\nend\n",
                );
                expect_no_offenses(
                    "Layout/EmptyLinesAroundAttributeAccessor",
                    "class Foo\n  attr_reader :a\n  private :a\n\n  def c; end\nend\n",
                );
                expect_no_offenses(
                    "Layout/EmptyLinesAroundAttributeAccessor",
                    "class Foo\n  attr_reader :a\nend\n",
                );
            }

            #[test]
            fn accessor_correction_inserts_a_blank_line_after_the_accessor() {
                expect_correction(
                    "Layout/EmptyLinesAroundAttributeAccessor",
                    "class Foo\n  attr_reader :a\n  def b; end\nend\n",
                    "class Foo\n  attr_reader :a\n\n  def b; end\nend\n",
                );
            }

            /// メソッド名が receiver と同じ行にあるか、間に空行やコメントの行があるものは
            /// 対象外。`::` の呼び出しも見ない。
            #[test]
            fn dot_position_skips_same_line_calls_and_gaps() {
                expect_no_offenses("Layout/DotPosition", "x = foo\n  .bar\n");
                expect_no_offenses("Layout/DotPosition", "x = foo.bar\n");
                expect_no_offenses("Layout/DotPosition", "x = foo.\n\n  bar\n");
                expect_no_offenses("Layout/DotPosition", "x = Foo::bar\n");
            }

            #[test]
            fn dot_position_correction_moves_the_dot_to_the_method_name() {
                expect_correction(
                    "Layout/DotPosition",
                    "x = foo.\n  bar.\n  baz\n",
                    "x = foo\n  .bar\n  .baz\n",
                );
            }
        }

        /// `Layout/ElseAlignment` と end 系 3 cop。
        ///
        /// 期待値は本家 1.89.0 の `--only <cop> --format json` と `-A` の実測。
        mod layout_else_and_end_alignment {
            use super::*;

            /// `elsif` は外側の `if` に、`case` の `else` は最後の `when` に、`rescue` の
            /// `else` は本体の持ち主に揃える。
            #[test]
            fn each_kind_of_else_has_its_own_base() {
                CopCase::new(
                    "Layout/ElseAlignment",
                    "if a\n  b\n elsif c\n  d\nend\n",
                    vec![Annotation::new(3, 2, 5, "Align `elsif` with `if`.")],
                )
                .run();
                CopCase::new(
                    "Layout/ElseAlignment",
                    "case x\nwhen 1\n  a\n   else\n  b\nend\n",
                    vec![Annotation::new(4, 4, 4, "Align `else` with `when`.")],
                )
                .run();
                CopCase::new(
                    "Layout/ElseAlignment",
                    "begin\n  a\nrescue\n  b\n   else\n  c\nend\n",
                    vec![Annotation::new(5, 4, 4, "Align `else` with `begin`.")],
                )
                .run();
                CopCase::new(
                    "Layout/ElseAlignment",
                    "foo do\n  a\nrescue\n  b\n   else\n  c\nend\n",
                    vec![Annotation::new(5, 4, 4, "Align `else` with `foo`.")],
                )
                .run();
                CopCase::new(
                    "Layout/ElseAlignment",
                    "private def bar\n  a\nrescue\n  b\n   else\n  c\nend\n",
                    vec![Annotation::new(5, 4, 4, "Align `else` with `private`.")],
                )
                .run();
            }

            #[test]
            fn else_alignment_correction_moves_only_the_keyword_line() {
                expect_correction(
                    "Layout/ElseAlignment",
                    "if a\n  b\n   else\n  c\nend\n",
                    "if a\n  b\nelse\n  c\nend\n",
                );
            }

            /// `end` がキーワードと同じ行にあるか同じ桁なら報告しない。ループの `end` は
            /// 文法上 body の中にあるが、上流ではループ自身のものとして数える。
            #[test]
            fn an_end_on_the_keyword_line_or_column_is_aligned() {
                expect_no_offenses("Layout/EndAlignment", "if a\n  b\nend\n");
                expect_no_offenses("Layout/EndAlignment", "x = if a then b end\n");
                CopCase::new(
                    "Layout/EndAlignment",
                    "x = while a\n  b\n  end\n",
                    vec![Annotation::new(
                        3,
                        3,
                        3,
                        "`end` at 3, 2 is not aligned with `while` at 1, 4.",
                    )],
                )
                .severity(Severity::Warning)
                .run();
            }

            #[test]
            fn end_alignment_correction_reindents_the_end() {
                expect_correction(
                    "Layout/EndAlignment",
                    "if a\n  b\n    end\n",
                    "if a\n  b\nend\n",
                );
                expect_correction(
                    "Layout/BeginEndAlignment",
                    "x = begin\n  1\n      end\n",
                    "x = begin\n  1\nend\n",
                );
                expect_correction(
                    "Layout/DefEndAlignment",
                    "private def foo\n  1\n  end\n",
                    "private def foo\n  1\nend\n",
                );
            }

            /// `Layout/BeginEndAlignment` は既定で行頭に、`Layout/DefEndAlignment` は既定で
            /// 修飾子の行頭に揃える。修飾子の無い `def` は `def` そのもの。
            #[test]
            fn the_end_family_defaults_to_the_start_of_the_line() {
                expect_no_offenses("Layout/BeginEndAlignment", "x = begin\n  1\nend\n");
                expect_no_offenses("Layout/DefEndAlignment", "private def foo\n  1\nend\n");
                expect_no_offenses("Layout/DefEndAlignment", "def foo\n  1\nend\n");
                CopCase::new(
                    "Layout/DefEndAlignment",
                    "  def foo\n    1\n end\n",
                    vec![Annotation::new(
                        3,
                        2,
                        3,
                        "`end` at 3, 1 is not aligned with `def` at 1, 2.",
                    )],
                )
                .severity(Severity::Warning)
                .run();
            }
        }

        /// `Layout/AccessModifierIndentation` と `Layout/CaseIndentation`。
        ///
        /// 期待値は本家 1.89.0 の `--only <cop> --format json` と `-A` の実測。
        mod layout_modifier_and_case_indentation {
            use super::*;

            /// 修飾子は `end` の桁 + 字下げ幅に置かれる。引数を取った修飾子は「bare」では
            /// ないので対象外、本体が 1 文しかない class も対象外。
            #[test]
            fn only_a_bare_modifier_in_a_multi_statement_body_is_measured() {
                expect_no_offenses(
                    "Layout/AccessModifierIndentation",
                    "class Foo\n  def a; end\n\n  private\n\n  def b; end\nend\n",
                );
                expect_no_offenses(
                    "Layout/AccessModifierIndentation",
                    "class Foo\n  def a; end\n\nprivate :a\n\n  def b; end\nend\n",
                );
                expect_no_offenses(
                    "Layout/AccessModifierIndentation",
                    "class Foo\nprivate\nend\n",
                );
                CopCase::new(
                    "Layout/AccessModifierIndentation",
                    "foo do\n  def a; end\n\nprivate\n\n  def b; end\nend\n",
                    vec![Annotation::new(
                        4,
                        1,
                        7,
                        "Indent access modifiers like `private`.",
                    )],
                )
                .run();
            }

            #[test]
            fn modifier_correction_shifts_the_modifier_line() {
                expect_correction(
                    "Layout/AccessModifierIndentation",
                    "class Foo\n  def a; end\n\nprivate\n\n  def b; end\nend\n",
                    "class Foo\n  def a; end\n\n  private\n\n  def b; end\nend\n",
                );
            }

            /// `when` / `in` は既定で `case` と同じ桁。1 行の `case` は対象外。
            #[test]
            fn branches_line_up_with_the_case_keyword() {
                expect_no_offenses("Layout/CaseIndentation", "case x\nwhen 1\n  a\nend\n");
                expect_no_offenses("Layout/CaseIndentation", "case x\nin 1\n  a\nend\n");
                expect_no_offenses("Layout/CaseIndentation", "x = case a; when 1 then 2; end\n");
                CopCase::new(
                    "Layout/CaseIndentation",
                    "case x\n  in 1\n  a\nend\n",
                    vec![Annotation::new(2, 3, 2, "Indent `in` as deep as `case`.")],
                )
                .run();
            }

            /// 行の途中に書かれた `when` には字下げが無いので、報告はしても corrector は
            /// 空のままになる。
            #[test]
            fn a_branch_sharing_its_line_with_code_is_not_correctable() {
                CopCase::new(
                    "Layout/CaseIndentation",
                    "case x\nwhen 1 then a; when 2 then b\nend\n",
                    vec![Annotation::new(
                        2,
                        16,
                        4,
                        "Indent `when` as deep as `case`.",
                    )],
                )
                .correctable(false)
                .run();
            }

            #[test]
            fn case_indentation_correction_reindents_every_branch() {
                expect_correction(
                    "Layout/CaseIndentation",
                    "case x\n  when 1\n  a\n  when 2\n  b\nend\n",
                    "case x\nwhen 1\n  a\nwhen 2\n  b\nend\n",
                );
            }
        }

        const COP: &str = "Lint/HashCompareByIdentity";
        const MSG: &str = "Use `Hash#compare_by_identity` instead of using `object_id` for keys.";

        /// `[]` と `[]=` は角括弧でもドットでも同じ send。`[]=` は代入式全体を指す。
        #[test]
        fn both_spellings_of_every_restricted_method_are_reported() {
            expect_offense(
                COP,
                r#"
            hash[foo.object_id] = 1
            ^^^^^^^^^^^^^^^^^^^^^^^ Use `Hash#compare_by_identity` instead of using `object_id` for keys.
            "#,
            );
            CopCase::new(
                COP,
                "hash[foo.object_id]\n",
                vec![Annotation::new(1, 1, 19, MSG)],
            )
            .run();
            CopCase::new(
                COP,
                "hash.has_key?(foo.object_id)\n",
                vec![Annotation::new(1, 1, 28, MSG)],
            )
            .run();
            CopCase::new(
                COP,
                "hash&.fetch(foo.object_id)\n",
                vec![Annotation::new(1, 1, 26, MSG)],
            )
            .run();
            CopCase::new(
                COP,
                "hash.[]=(foo.object_id, 1)\n",
                vec![Annotation::new(1, 1, 26, MSG)],
            )
            .run();
            // 多重代入の中では `[]=` に値が渡らないので、send は角括弧で終わる。
            CopCase::new(
                COP,
                "hash[foo.object_id], x = 1, 2\n",
                vec![Annotation::new(1, 1, 19, MSG)],
            )
            .run();
            // ブロックは send の外。
            CopCase::new(
                COP,
                "hash.fetch(foo.object_id) { 1 }\n",
                vec![Annotation::new(1, 1, 25, MSG)],
            )
            .run();
        }

        /// 鍵は `(send _ :object_id)` ちょうど。`&.`、引数付き、ブロック付きはどれも別の節点。
        #[test]
        fn only_a_plain_object_id_call_counts() {
            expect_no_offenses(COP, "hash.key?(foo&.object_id)\n");
            expect_no_offenses(COP, "hash.key?(foo.object_id { })\n");
            expect_no_offenses(COP, "hash.key?(foo.object_id(1))\n");
            expect_no_offenses(COP, "hash.key?(foo.object_id2)\n");
            expect_no_offenses(COP, "hash.size(foo.object_id)\n");
            // 引数無しの括弧は upstream でも引数無しの send。
            CopCase::new(
                COP,
                "hash.key?(foo.object_id())\n",
                vec![Annotation::new(1, 1, 26, MSG)],
            )
            .run();
        }

        /// レシーバ無しの `object_id` は、ローカル変数でなければ `(send nil :object_id)`。
        #[test]
        fn a_bare_name_counts_only_while_it_is_not_a_local_variable() {
            CopCase::new(
                COP,
                "hash.key?(object_id)\n",
                vec![Annotation::new(1, 1, 20, MSG)],
            )
            .run();
            expect_no_offenses(COP, "object_id = 1\nhash.key?(object_id)\n");
        }
    }

    /// `Lint/SelfAssignment`。左辺と右辺が同じものを指す代入を禁じる。
    ///
    /// 期待値はすべて本家 1.89.0 の実測。
    mod self_assignment {
        use super::*;

        const COP: &str = "Lint/SelfAssignment";
        const MSG: &str = "Self-assignment detected.";

        #[test]
        fn every_kind_of_variable_is_reported() {
            expect_offense(
                COP,
                r#"
            foo = foo
            ^^^^^^^^^ Self-assignment detected.
            "#,
            );
            CopCase::new(COP, "@foo = @foo\n", vec![Annotation::new(1, 1, 11, MSG)]).run();
            CopCase::new(COP, "@@foo = @@foo\n", vec![Annotation::new(1, 1, 13, MSG)]).run();
            CopCase::new(COP, "$foo = $foo\n", vec![Annotation::new(1, 1, 11, MSG)]).run();
            CopCase::new(COP, "foo ||= foo\n", vec![Annotation::new(1, 1, 11, MSG)]).run();
            CopCase::new(COP, "foo &&= foo\n", vec![Annotation::new(1, 1, 11, MSG)]).run();
            expect_no_offenses(COP, "foo = bar\n");
            expect_no_offenses(COP, "foo = foo.dup\n");
            // `+=` は or/and 代入ではないので対象外。
            expect_no_offenses(COP, "obj.attr += obj.attr\n");
        }

        /// 定数は名前空間まで一致して初めて同じ定数。
        #[test]
        fn a_constant_has_to_agree_on_its_namespace() {
            CopCase::new(COP, "Foo = Foo\n", vec![Annotation::new(1, 1, 9, MSG)]).run();
            CopCase::new(COP, "A::B = A::B\n", vec![Annotation::new(1, 1, 11, MSG)]).run();
            CopCase::new(COP, "::Foo = ::Foo\n", vec![Annotation::new(1, 1, 13, MSG)]).run();
            CopCase::new(COP, "Foo ||= Foo\n", vec![Annotation::new(1, 1, 11, MSG)]).run();
            expect_no_offenses(COP, "A::B = B\n");
            expect_no_offenses(COP, "B = A::B\n");
        }

        /// 多重代入は右辺が配列で、位置ごとに同じ変数を読み返しているときだけ。
        #[test]
        fn a_multiple_assignment_matches_position_by_position() {
            CopCase::new(
                COP,
                "foo, bar = foo, bar\n",
                vec![Annotation::new(1, 1, 19, MSG)],
            )
            .run();
            CopCase::new(
                COP,
                "foo, bar = [foo, bar]\n",
                vec![Annotation::new(1, 1, 21, MSG)],
            )
            .run();
            expect_no_offenses(COP, "foo, bar = bar, foo\n");
            expect_no_offenses(COP, "a, b = *c\n");
            expect_no_offenses(COP, "a, b = c\n");
            // 定数と setter は `ASSIGNMENT_TYPE_TO_RHS_TYPE` に載っていない。
            expect_no_offenses(COP, "A, B = A, B\n");
            expect_no_offenses(COP, "obj.a, obj.b = obj.a, obj.b\n");
        }

        /// 鍵の代入は、鍵自身がメソッド呼び出しでないときだけ。二度目が同じ答えとは限らない。
        #[test]
        fn a_key_assignment_needs_a_key_that_is_not_a_call() {
            CopCase::new(
                COP,
                "hash['foo'] = hash['foo']\n",
                vec![Annotation::new(1, 1, 25, MSG)],
            )
            .run();
            CopCase::new(
                COP,
                "hash['a'] ||= hash['a']\n",
                vec![Annotation::new(1, 1, 23, MSG)],
            )
            .run();
            CopCase::new(
                COP,
                "hash.[]=(1, hash.[](1))\n",
                vec![Annotation::new(1, 1, 23, MSG)],
            )
            .run();
            expect_no_offenses(COP, "hash[foo] = hash[foo]\n");
            expect_no_offenses(COP, "hash[foo] ||= hash[foo]\n");
            // 引数がローカル変数なら呼び出しではない。
            CopCase::new(
                COP,
                "foo = 1\nhash[foo] = hash[foo]\n",
                vec![Annotation::new(2, 1, 21, MSG)],
            )
            .run();
            expect_no_offenses(COP, "hash['a'] = hash['b']\n");
        }

        /// setter は同名の reader を、引数無しで、同じレシーバに対して呼んでいるときだけ。
        #[test]
        fn an_attribute_assignment_needs_the_matching_reader() {
            CopCase::new(
                COP,
                "obj.attr = obj.attr\n",
                vec![Annotation::new(1, 1, 19, MSG)],
            )
            .run();
            CopCase::new(
                COP,
                "self.foo = self.foo\n",
                vec![Annotation::new(1, 1, 19, MSG)],
            )
            .run();
            CopCase::new(
                COP,
                "obj&.attr = obj&.attr\n",
                vec![Annotation::new(1, 1, 21, MSG)],
            )
            .run();
            CopCase::new(
                COP,
                "obj.attr ||= obj.attr\n",
                vec![Annotation::new(1, 1, 21, MSG)],
            )
            .run();
            expect_no_offenses(COP, "obj.attr = obj.attr2\n");
            expect_no_offenses(COP, "obj.attr ||= obj.attr(1)\n");
            // 括弧を挟むと右辺は `begin` になり、呼び出しではなくなる。
            expect_no_offenses(COP, "obj.attr=(obj.attr)\n");
        }
    }
    /// `Lint/EmptyInterpolation`。何も差し込まない `#{}` を禁じる。
    ///
    /// 期待値はすべて本家 1.89.0 の実測。
    mod empty_interpolation {
        use super::*;

        const COP: &str = "Lint/EmptyInterpolation";
        const MSG: &str = "Empty interpolation detected.";

        /// `nil` と空文字列リテラルは取り除かれてから「残ったか」を見る。
        #[test]
        fn a_child_that_contributes_nothing_still_counts_as_empty() {
            expect_offense(COP, "x = \"#{}\"\n     ^^^ Empty interpolation detected.\n");
            CopCase::new(COP, "x = \"#{ }\"\n", vec![Annotation::new(1, 6, 4, MSG)]).run();
            CopCase::new(COP, "x = \"#{nil}\"\n", vec![Annotation::new(1, 6, 6, MSG)]).run();
            CopCase::new(COP, "x = \"#{''}\"\n", vec![Annotation::new(1, 6, 5, MSG)]).run();
            // `;` は `begin` の子ではなく区切り。
            CopCase::new(COP, "x = \"#{;}\"\n", vec![Annotation::new(1, 6, 4, MSG)]).run();
            expect_no_offenses(COP, "x = \"#{1}\"\n");
            expect_no_offenses(COP, "x = \"#{'a'}\"\n");
        }

        /// dstr 以外の補間も同じ。`%W`/`%I` の中だけは要素そのものなので見逃す。
        #[test]
        fn every_interpolating_literal_is_inspected_except_percent_arrays() {
            CopCase::new(COP, "x = :\"#{}\"\n", vec![Annotation::new(1, 7, 3, MSG)]).run();
            CopCase::new(COP, "x = /#{}/\n", vec![Annotation::new(1, 6, 3, MSG)]).run();
            CopCase::new(COP, "x = `#{}`\n", vec![Annotation::new(1, 6, 3, MSG)]).run();
            CopCase::new(COP, "x = [\"#{}\"]\n", vec![Annotation::new(1, 7, 3, MSG)]).run();
            expect_no_offenses(COP, "x = %W[#{}]\n");
            expect_no_offenses(COP, "x = %I[#{}]\n");
        }

        /// autocorrect は補間ごと消す。
        #[test]
        fn the_correction_removes_the_interpolation() {
            expect_correction(COP, "x = \"a#{}b#{nil}c\"\n", "x = \"abc\"\n");
            expect_correction(COP, "x = :\"#{}\"\n", "x = :\"\"\n");
        }
    }

    /// `Lint/FloatComparison`。浮動小数点数の等値比較を禁じる。
    ///
    /// 期待値はすべて本家 1.89.0 の実測。
    mod float_comparison {
        use super::*;

        const COP: &str = "Lint/FloatComparison";
        const MSG_EQ: &str = "Avoid equality comparisons of floats as they are unreliable.";
        const MSG_NE: &str = "Avoid inequality comparisons of floats as they are unreliable.";
        const MSG_CASE: &str =
            "Avoid float literal comparisons in case statements as they are unreliable.";

        #[test]
        fn the_four_equality_methods_are_reported_with_two_messages() {
            expect_offense(
                COP,
                r#"
            x == 0.1
            ^^^^^^^^ Avoid equality comparisons of floats as they are unreliable.
            "#,
            );
            CopCase::new(COP, "x != 0.1\n", vec![Annotation::new(1, 1, 8, MSG_NE)]).run();
            CopCase::new(
                COP,
                "x.eql?(0.1)\n",
                vec![Annotation::new(1, 1, 11, MSG_EQ)],
            )
            .run();
            CopCase::new(
                COP,
                "x.equal?(0.1)\n",
                vec![Annotation::new(1, 1, 13, MSG_EQ)],
            )
            .run();
            CopCase::new(COP, "0.1 == x\n", vec![Annotation::new(1, 1, 8, MSG_EQ)]).run();
            // 引数がちょうど 1 個のときだけ。
            expect_no_offenses(COP, "x.eql?(0.1, 2)\n");
            expect_no_offenses(COP, "x == 1\n");
            expect_no_offenses(COP, "x == y\n");
        }

        /// ゼロと `nil` はどちらの側にあっても正確に比べられる。
        #[test]
        fn a_zero_or_nil_on_either_side_is_exempt() {
            expect_no_offenses(COP, "x == 0.0\n");
            expect_no_offenses(COP, "x != 0.0\n");
            expect_no_offenses(COP, "x == nil\n");
            CopCase::new(COP, "-0.1 == x\n", vec![Annotation::new(1, 1, 9, MSG_EQ)]).run();
        }

        /// 浮動小数点数を返すと分かる式も対象。丸めは引数次第、`angle` は符号次第。
        #[test]
        fn an_expression_known_to_produce_a_float_counts() {
            CopCase::new(
                COP,
                "x.to_f == y\n",
                vec![Annotation::new(1, 1, 11, MSG_EQ)],
            )
            .run();
            CopCase::new(
                COP,
                "Float(x) == y\n",
                vec![Annotation::new(1, 1, 13, MSG_EQ)],
            )
            .run();
            CopCase::new(
                COP,
                "x.fdiv(2) == y\n",
                vec![Annotation::new(1, 1, 14, MSG_EQ)],
            )
            .run();
            CopCase::new(
                COP,
                "(x + 0.1) == y\n",
                vec![Annotation::new(1, 1, 14, MSG_EQ)],
            )
            .run();
            CopCase::new(COP, "x == (0.1)\n", vec![Annotation::new(1, 1, 10, MSG_EQ)]).run();
            CopCase::new(
                COP,
                "1.0.abs == x\n",
                vec![Annotation::new(1, 1, 12, MSG_EQ)],
            )
            .run();
            CopCase::new(
                COP,
                "1.0.ceil(2) == x\n",
                vec![Annotation::new(1, 1, 16, MSG_EQ)],
            )
            .run();
            expect_no_offenses(COP, "1.0.ceil == x\n");
            CopCase::new(
                COP,
                "-1.0.angle == x\n",
                vec![Annotation::new(1, 1, 15, MSG_EQ)],
            )
            .run();
            expect_no_offenses(COP, "1.0.angle == x\n");
            expect_no_offenses(COP, "x.round == y\n");
        }

        /// `case` の `when` は条件ひとつずつ、別のメッセージで報告する。
        #[test]
        fn a_when_condition_is_reported_on_its_own() {
            CopCase::new(
                COP,
                "case v\nwhen 1.0\n  a\nwhen 0.0\n  b\nwhen x.to_f\n  c\nend\n",
                vec![
                    Annotation::new(2, 6, 3, MSG_CASE),
                    Annotation::new(6, 6, 6, MSG_CASE),
                ],
            )
            .run();
        }
    }

    /// `Lint/Loop`。`begin ... end while` を `Kernel#loop` に導く。
    ///
    /// 期待値はすべて本家 1.89.0 の実測。
    mod loop_construct {
        use super::*;

        const COP: &str = "Lint/Loop";
        const MSG: &str =
            "Use `Kernel#loop` with `break` rather than `begin/end/until`(or `while`).";

        /// 本体が `begin ... end` のときだけ。指すのは後置キーワード。
        #[test]
        fn only_a_post_condition_loop_over_a_begin_block_is_reported() {
            expect_offense(
                COP,
                r#"
            begin
              a
            end while b
                ^^^^^ Use `Kernel#loop` with `break` rather than `begin/end/until`(or `while`).
            "#,
            );
            CopCase::new(
                COP,
                "begin\n  a\nend until b\n",
                vec![Annotation::new(3, 5, 5, MSG)],
            )
            .run();
            expect_no_offenses(COP, "a while b\n");
            expect_no_offenses(COP, "while b\n  a\nend\n");
            // 代入を挟むと本体は `begin` そのものではなくなる。
            expect_no_offenses(COP, "x = begin\n  a\nend while b\n");
        }

        /// autocorrect は `loop do` へ書き換え、`end` の直前へ `break` 行を字下げ付きで足す。
        #[test]
        fn the_correction_rewrites_the_block_and_inserts_a_break() {
            expect_correction(
                COP,
                "begin\n  a\nend while b\n",
                "loop do\n  a\nbreak unless b\nend\n",
            );
            expect_correction(
                COP,
                "begin\n  a\nend until b\n",
                "loop do\n  a\nbreak if b\nend\n",
            );
            // 字下げは while_post 節点自身の桁。
            expect_correction(
                COP,
                "def m\n  begin\n    a\n  end while c\nend\n",
                "def m\n  loop do\n    a\n  break unless c\n  end\nend\n",
            );
            expect_correction(
                COP,
                "begin; a; end while c\n",
                "loop do; a; break unless c\nend\n",
            );
        }
    }

    /// `Lint/NonLocalExitFromIterator`。値を返さない `return` でイテレータを抜けるのを禁じる。
    ///
    /// 期待値はすべて本家 1.89.0 の実測。
    mod non_local_exit_from_iterator {
        use super::*;

        const COP: &str = "Lint/NonLocalExitFromIterator";
        const MSG: &str = "Non-local exit from iterator, without return value. \
                       `next`, `break`, `Array#find`, `Array#any?`, etc. is preferred.";

        /// レシーバ付きの呼び出しに渡した、引数を取るブロックの中だけ。
        #[test]
        fn a_valueless_return_inside_a_chained_block_with_arguments_is_reported() {
            expect_offense(
                COP,
                r#"
            foo.each { |x| return }
                           ^^^^^^ Non-local exit from iterator, without return value. `next`, `break`, `Array#find`, `Array#any?`, etc. is preferred.
            "#,
            );
            CopCase::new(
                COP,
                "Foo.bar.baz { |x| return }\n",
                vec![Annotation::new(1, 19, 6, MSG)],
            )
            .run();
            // 値を返す `return` は意図した脱出。
            expect_no_offenses(COP, "foo.each { |x| return 1 }\n");
            // レシーバが無い呼び出しは連鎖ではない。
            expect_no_offenses(COP, "each { |x| return }\n");
            // 引数を取らないブロックは外側へ委ねる。
            expect_no_offenses(COP, "foo.each { return }\n");
            expect_no_offenses(COP, "foo.each { || return }\n");
        }

        /// 自前のスコープを開くもの (`def` / lambda) と `define_method` は探索を止める。
        #[test]
        fn a_scope_of_its_own_stops_the_search() {
            expect_no_offenses(COP, "lambda { |x| return }\n");
            expect_no_offenses(COP, "->(x) { return }\n");
            expect_no_offenses(COP, "define_method(:x) { |y| return }\n");
            expect_no_offenses(COP, "define_singleton_method(:x) { |y| return }\n");
            expect_no_offenses(COP, "obj.define_method(:x) { |y| return }\n");
            expect_no_offenses(COP, "foo.each do |x|\n  def m\n    return\n  end\nend\n");
            expect_no_offenses(COP, "foo.each do |x|\n  ->(y) { return }\nend\n");
        }

        /// 引数無しのブロックは外側のブロックへ判定を渡す。
        #[test]
        fn a_block_without_arguments_passes_the_question_outwards() {
            expect_no_offenses(COP, "transaction do\n  return unless c\nend\n");
            expect_no_offenses(
                COP,
                "transaction do\n  find_each do |item|\n    return if item.a\n  end\nend\n",
            );
            CopCase::new(
                COP,
                "foo.map { |x| foo.each { |y| return } }\n",
                vec![Annotation::new(1, 30, 6, MSG)],
            )
            .run();
        }
    }

    /// `Lint/StructNewOverride`。`Struct.new` の member が既存メソッドを覆うのを警告する。
    ///
    /// 期待値はすべて本家 1.89.0 の実測。
    mod struct_new_override {
        use super::*;

        const COP: &str = "Lint/StructNewOverride";

        fn message(quoted: &str, name: &str) -> String {
            format!("`{quoted}` member overrides `Struct#{name}` and it may be unexpected.")
        }

        #[test]
        fn every_member_naming_a_struct_method_is_reported() {
            expect_offense(
                COP,
                r#"
            Bad = Struct.new(:members, :clone)
                             ^^^^^^^^ `:members` member overrides `Struct#members` and it may be unexpected.
                                       ^^^^^^ `:clone` member overrides `Struct#clone` and it may be unexpected.
            "#,
            );
            CopCase::new(
                COP,
                "U = ::Struct.new(:size)\n",
                vec![Annotation::new(1, 18, 5, message(":size", "size"))],
            )
            .run();
            // 演算子名も member になりうる。
            CopCase::new(
                COP,
                "Z = Struct.new(:<=>)\n",
                vec![Annotation::new(1, 16, 4, message(":<=>", "<=>"))],
            )
            .run();
            expect_no_offenses(COP, "Good = Struct.new(:id, :name)\n");
        }

        /// 先頭の文字列は struct の名前で member ではない。名前空間付きと `&.` は別の呼び出し。
        #[test]
        fn the_leading_name_argument_and_other_receivers_are_skipped() {
            expect_no_offenses(COP, "W = Struct.new(\"count\")\n");
            CopCase::new(
                COP,
                "S = Struct.new(\"Name\", :count)\n",
                vec![Annotation::new(1, 24, 6, message(":count", "count"))],
            )
            .run();
            expect_no_offenses(COP, "V = Foo::Struct.new(:size)\n");
            expect_no_offenses(COP, "Y = Struct&.new(:count)\n");
            expect_no_offenses(COP, "A = Struct.new(*args)\n");
        }

        /// 引用符付きシンボルはメッセージだけ `inspect` の形になる。
        #[test]
        fn a_quoted_symbol_reports_the_plain_name() {
            CopCase::new(
                COP,
                "X = Struct.new(:\"count\")\n",
                vec![Annotation::new(1, 16, 8, message(":count", "count"))],
            )
            .run();
        }
    }
    /// `Lint/DisjunctiveAssignmentInConstructor`。コンストラクタ冒頭の `||=` を禁じる。
    ///
    /// 期待値はすべて本家 1.89.0 の実測。
    mod disjunctive_assignment_in_constructor {
        use super::*;

        const COP: &str = "Lint/DisjunctiveAssignmentInConstructor";
        const MSG: &str = "Unnecessary disjunctive assignment. Use plain assignment.";

        /// 走査は先頭から続く `||=` の並びだけ。別種の式で打ち切る。
        #[test]
        fn the_run_of_disjunctive_assignments_at_the_top_is_reported() {
            expect_offense(
                COP,
                r#"
            class C
              def initialize
                @a ||= 1
                   ^^^ Unnecessary disjunctive assignment. Use plain assignment.
                @b ||= 2
                   ^^^ Unnecessary disjunctive assignment. Use plain assignment.
                @c = 3
                @d ||= 4
              end
            end
            "#,
            );
            CopCase::new(
                COP,
                "class E\n  def initialize\n    @a ||= 1\n  end\nend\n",
                vec![Annotation::new(3, 8, 3, MSG)],
            )
            .run();
            // 左辺がインスタンス変数でないときは報告しないが、走査は続く。
            CopCase::new(
                COP,
                "class D\n  def initialize(x)\n    x ||= 1\n    @a ||= 2\n  end\nend\n",
                vec![Annotation::new(4, 8, 3, MSG)],
            )
            .run();
            expect_no_offenses(
                COP,
                "class H\n  def initialize\n    @a, @b = 1, 2\n    @c ||= 3\n  end\nend\n",
            );
        }

        /// `rescue` の付いた本体は `begin` ではなくなるので、1 行目から対象外。
        #[test]
        fn a_rescue_or_a_singleton_definition_is_out_of_scope() {
            expect_no_offenses(
                COP,
                "class F\n  def initialize\n    @a ||= 1\n  rescue\n    nil\n  end\nend\n",
            );
            expect_no_offenses(
                COP,
                "class G\n  def self.initialize\n    @a ||= 1\n  end\nend\n",
            );
        }

        /// autocorrect は演算子だけを `=` に置き換える。
        #[test]
        fn the_correction_replaces_the_operator() {
            expect_correction(
                COP,
                "class C\n  def initialize\n    @a ||= 1\n  end\nend\n",
                "class C\n  def initialize\n    @a = 1\n  end\nend\n",
            );
        }
    }

    /// `Lint/ParenthesesAsGroupedExpression`。`foo (a)` の空白を禁じる。
    ///
    /// 期待値はすべて本家 1.89.0 の実測。
    mod parentheses_as_grouped_expression {
        use super::*;

        const COP: &str = "Lint/ParenthesesAsGroupedExpression";

        fn message(argument: &str) -> String {
            format!("`{argument}` interpreted as grouped expression.")
        }

        /// 指すのはセレクタと括弧の間の空白そのもの。
        #[test]
        fn the_space_between_the_selector_and_the_parenthesis_is_reported() {
            expect_offense(
                COP,
                r#"
            puts (1 + 2)
                ^ `(1 + 2)` interpreted as grouped expression.
            "#,
            );
            CopCase::new(
                COP,
                "foo   (a)\n",
                vec![Annotation::new(1, 4, 3, message("(a)"))],
            )
            .run();
            CopCase::new(
                COP,
                "obj.foo (a)\n",
                vec![Annotation::new(1, 8, 1, message("(a)"))],
            )
            .run();
            CopCase::new(
                COP,
                "foo&.bar (a)\n",
                vec![Annotation::new(1, 9, 1, message("(a)"))],
            )
            .run();
            expect_no_offenses(COP, "puts(1 + 2)\n");
            expect_no_offenses(COP, "x = (1 + 2)\n");
        }

        /// 引数が 1 個で、その引数が括弧で始まるときだけ。連鎖した呼び出しは対象外。
        #[test]
        fn a_chained_call_or_a_second_argument_takes_it_out_of_scope() {
            expect_no_offenses(COP, "puts (a).to_s\n");
            expect_no_offenses(COP, "foo (a).b(1)\n");
            expect_no_offenses(COP, "foo (a) + 1\n");
            expect_no_offenses(COP, "foo (a), b\n");
            // 演算子と setter は空白を置くのが普通の書き方。
            expect_no_offenses(COP, "foo.== (a)\n");
            expect_no_offenses(COP, "x.foo= (a)\n");
            // `yield` と `return` は send ではない。
            expect_no_offenses(COP, "yield (a)\n");
            expect_no_offenses(COP, "return (a)\n");
        }

        /// autocorrect は空白を消すだけ。
        #[test]
        fn the_correction_removes_the_space() {
            expect_correction(COP, "puts (1 + 2)\n", "puts(1 + 2)\n");
            expect_correction(COP, "foo   (a)\n", "foo(a)\n");
        }
    }

    /// `Lint/ReturnInVoidContext`。戻り値が捨てられるメソッドでの `return 値` を禁じる。
    ///
    /// 期待値はすべて本家 1.89.0 の実測。
    mod return_in_void_context {
        use super::*;

        const COP: &str = "Lint/ReturnInVoidContext";

        fn message(method: &str) -> String {
            format!("Do not return a value in `{method}`.")
        }

        #[test]
        fn a_constructor_and_a_setter_are_the_two_void_contexts() {
            expect_offense(
                COP,
                r#"
            class C
              def initialize
                return 1
                ^^^^^^ Do not return a value in `initialize`.
              end
            end
            "#,
            );
            CopCase::new(
                COP,
                "class C\n  def foo=(v)\n    return 1\n  end\nend\n",
                vec![Annotation::new(3, 5, 6, message("foo="))],
            )
            .run();
            expect_no_offenses(COP, "class C\n  def bar\n    return 1\n  end\nend\n");
            // 値を返さない `return` は対象外。
            expect_no_offenses(COP, "class C\n  def baz=(v)\n    return\n  end\nend\n");
        }

        /// スコープを移すブロックの中は別のメソッドの本体になる。
        #[test]
        fn a_scope_changing_block_takes_the_return_out_of_the_void_context() {
            expect_no_offenses(
                COP,
                "class C\n  def initialize\n    define_method(:x) { return 1 }\n  end\nend\n",
            );
            expect_no_offenses(
                COP,
                "class C\n  def initialize\n    lambda { return 1 }\n  end\nend\n",
            );
            expect_no_offenses(
                COP,
                "class C\n  def initialize\n    ->() { return 1 }\n  end\nend\n",
            );
            CopCase::new(
                COP,
                "class C\n  def initialize\n    [1].each { return 3 }\n  end\nend\n",
                vec![Annotation::new(3, 16, 6, message("initialize"))],
            )
            .run();
        }
    }

    /// `Layout/Multiline*BraceLayout` の 4 本。期待値は本家 1.89.0 の
    /// `--only <cop> --format json` と `-A` の実測。
    mod layout_multiline_brace_layout {
        use super::*;

        /// symmetrical では開き括弧が第 1 要素と同じ行なら閉じ括弧も最終要素と同じ行に来る。
        /// 補正は閉じ括弧を消して最終要素の直後へ入れ直す。
        #[test]
        fn method_call_brace_follows_the_opening_brace() {
            CopCase::annotated(
            "Layout/MultilineMethodCallBraceLayout",
            r#"
            foo(a,
              b
            )
            ^ Closing method call brace must be on the same line as the last argument when opening brace is on the same line as the first argument.
            "#,
        )
        .run();
            expect_correction(
                "Layout/MultilineMethodCallBraceLayout",
                "foo(a,\n  b\n)\n",
                "foo(a,\n  b)\n",
            );
            // 開き括弧が別行なら閉じ括弧も別行。逆向きの補正は改行を入れるだけ。
            CopCase::annotated(
            "Layout/MultilineMethodCallBraceLayout",
            r#"
            foo(
              a,
              b)
               ^ Closing method call brace must be on the line after the last argument when opening brace is on a separate line from the first argument.
            "#,
        )
        .run();
            expect_correction(
                "Layout/MultilineMethodCallBraceLayout",
                "foo(\n  a,\n  b)\n",
                "foo(\n  a,\n  b\n)\n",
            );
            expect_no_offenses("Layout/MultilineMethodCallBraceLayout", "foo(a,\n  b)\n");
            expect_no_offenses(
                "Layout/MultilineMethodCallBraceLayout",
                "foo(\n  a,\n  b\n)\n",
            );
        }

        /// 括弧を書いていない呼び出しと、引数のない呼び出しは暗黙リテラル扱いで対象外。
        /// `super(...)` は本家では `send` ではないので `on_send` が発火しない。
        #[test]
        fn method_call_ignores_implicit_and_empty_and_super() {
            expect_no_offenses("Layout/MultilineMethodCallBraceLayout", "foo a,\n  b\n");
            expect_no_offenses("Layout/MultilineMethodCallBraceLayout", "foo(\n)\n");
            expect_no_offenses(
                "Layout/MultilineMethodCallBraceLayout",
                "def m\n  super(\"a\" \\\n        \"b\"\n       )\nend\n",
            );
            // 添字読みは本家でも `send` だが `loc.begin` を持たないので暗黙リテラル。
            expect_no_offenses("Layout/MultilineMethodCallBraceLayout", "a[1,\n  2\n]\n");
        }

        /// 最終引数がヒアドキュメントを抱えていると、閉じ括弧を上げるとコードが壊れるので
        /// 本家は何も報告しない。
        #[test]
        fn method_call_leaves_a_trailing_heredoc_alone() {
            expect_no_offenses(
                "Layout/MultilineMethodCallBraceLayout",
                "foo(a,\n  <<~X\n    hi\n  X\n)\n",
            );
        }

        /// 第 1 引数がヒアドキュメントの呼び出しに繋いだメソッドは閉じ括弧と一緒に動く。
        /// 本家は `insert_before` で括弧、`insert_after` でチェインを同じ空レンジへ入れる。
        #[test]
        fn method_call_moves_the_chained_method_with_the_brace() {
            expect_correction(
                "Layout/MultilineMethodCallBraceLayout",
                "foo(<<~X,\n  hi\nX\n  b\n).bar\n",
                "foo(<<~X,\n  hi\nX\n  b).bar\n",
            );
        }

        /// 配列は `%w` / `%i` も対象。閉じ括弧の直前の要素行にコメントがあり、かつ
        /// チェインまたは引数になっているものは報告だけで補正しない。
        #[test]
        fn array_brace_covers_percent_literals_and_comments() {
            CopCase::annotated(
            "Layout/MultilineArrayBraceLayout",
            r#"
            x = %w[
              a
              b ]
                ^ The closing array brace must be on the line after the last array element when the opening brace is on a separate line from the first array element.
            "#,
        )
        .run();
            CopCase::annotated(
            "Layout/MultilineArrayBraceLayout",
            r#"
            yy = [1,
              2 # c
            ].freeze
            ^ The closing array brace must be on the same line as the last array element when the opening brace is on the same line as the first array element.
            "#,
        )
        .correctable(false)
        .run();
            // チェインも引数もしていなければ補正する。閉じ括弧は最終要素の直後へ移り、
            // 行末コメントはその場に残る。
            expect_correction(
                "Layout/MultilineArrayBraceLayout",
                "yy = [1,\n  2 # c\n]\n",
                "yy = [1,\n  2] # c\n",
            );
            // 末尾のカンマは最終要素の一部として扱われ、閉じ括弧はその後ろへ入る。
            expect_correction(
                "Layout/MultilineArrayBraceLayout",
                "[1,\n  2,\n]\n",
                "[1,\n  2,]\n",
            );
        }

        /// ブレース付きハッシュだけが対象で、`foo(a: 1)` のような暗黙ハッシュは対象外。
        #[test]
        fn hash_brace_needs_braces() {
            CopCase::annotated(
            "Layout/MultilineHashBraceLayout",
            r#"
            { a: 1,
              b: 2
            }
            ^ Closing hash brace must be on the same line as the last hash element when opening brace is on the same line as the first hash element.
            "#,
        )
        .run();
            expect_correction(
                "Layout/MultilineHashBraceLayout",
                "{ a: 1,\n  b: 2\n}\n",
                "{ a: 1,\n  b: 2}\n",
            );
            expect_no_offenses("Layout/MultilineHashBraceLayout", "foo(a: 1,\n  b: 2\n)\n");
        }

        /// 定義側は仮引数リストがリテラル。括弧なしの `def foo a, b` は暗黙リテラル。
        #[test]
        fn method_definition_brace_reports_the_parameter_list() {
            CopCase::annotated(
            "Layout/MultilineMethodDefinitionBraceLayout",
            r#"
            def foo(a,
              b
            )
            ^ Closing method definition brace must be on the same line as the last parameter when opening brace is on the same line as the first parameter.
            end
            "#,
        )
        .run();
            expect_correction(
                "Layout/MultilineMethodDefinitionBraceLayout",
                "def foo(a,\n  b\n)\nend\n",
                "def foo(a,\n  b)\nend\n",
            );
            expect_no_offenses(
                "Layout/MultilineMethodDefinitionBraceLayout",
                "def foo a,\n  b\nend\n",
            );
        }
    }

    /// トークン列の隣接だけで決まる空白の cop 群。期待値は本家 1.89.0 の
    /// `--only <cop> --format json` と `-A` の実測。
    mod layout_token_spacing {
        use super::*;

        #[test]
        fn space_before_semicolon() {
            CopCase::annotated(
                "Layout/SpaceBeforeSemicolon",
                r#"
            foo ; bar
               ^ Space found before semicolon.
            a = 1  ;
                 ^^ Space found before semicolon.
            "#,
            )
            .run();
            expect_correction(
                "Layout/SpaceBeforeSemicolon",
                "foo ; bar\na = 1  ;\n",
                "foo; bar\na = 1;\n",
            );
            // 行頭のセミコロンは直前のトークンが別行なので対象外。ブロックの `{` は
            // `Layout/SpaceInsideBlockBraces` が空白を求めるので免除される。
            expect_no_offenses("Layout/SpaceBeforeSemicolon", "foo\n  ;\n");
            expect_no_offenses("Layout/SpaceBeforeSemicolon", "foo { ; }\n");
            expect_no_offenses("Layout/SpaceBeforeSemicolon", "x = \"a ; b\"\n");
        }

        /// 2 個以上の空白だけが対象で、単語の末尾のエスケープ空白は単語の一部。
        #[test]
        fn space_inside_array_percent_literal() {
            const MESSAGE: &str = "Use only a single space inside array percent literal.";
            CopCase::new(
                "Layout/SpaceInsideArrayPercentLiteral",
                "x = %w[a  b   c]\n",
                vec![
                    Annotation::new(1, 9, 2, MESSAGE),
                    Annotation::new(1, 12, 3, MESSAGE),
                ],
            )
            .run();
            expect_correction(
                "Layout/SpaceInsideArrayPercentLiteral",
                "x = %w[a  b   c]\n",
                "x = %w[a b c]\n",
            );
            expect_no_offenses("Layout/SpaceInsideArrayPercentLiteral", "x = %w[a b]\n");
            // 先頭と末尾の空白は要素の間ではない。
            expect_no_offenses(
                "Layout/SpaceInsideArrayPercentLiteral",
                "x = %w[\n  a\n  b\n]\n",
            );
            expect_no_offenses("Layout/SpaceInsideArrayPercentLiteral", "x = %w[a\\  b]\n");
            expect_no_offenses("Layout/SpaceInsideArrayPercentLiteral", "x = [a,  b]\n");
        }

        /// 1 つの `#{}` につき corrector は 1 回しか回らないので、2 件目の offense は
        /// 補正不能になる。
        #[test]
        fn space_inside_string_interpolation() {
            const COP: &str = "Layout/SpaceInsideStringInterpolation";
            const MESSAGE: &str = "Do not use space inside string interpolation.";
            CopCase::new(
                COP,
                "q = \"#{ a } #{b } #{ c}\"\n",
                vec![
                    Annotation::new(1, 8, 1, MESSAGE),
                    Annotation::new(1, 10, 1, MESSAGE),
                    Annotation::new(1, 16, 1, MESSAGE),
                    Annotation::new(1, 21, 1, MESSAGE),
                ],
            )
            .run();
            expect_correction(
                COP,
                "q = \"#{ a } #{b } #{ c}\"\n",
                "q = \"#{a} #{b} #{c}\"\n",
            );
            expect_no_offenses(COP, "q = \"#{a}\"\n");
            // 中身のない `#{}` と `#{ }` はトークンが隣り合うので「中」が無い。
            expect_no_offenses(COP, "q = \"#{}\"\n");
            expect_no_offenses(COP, "q = \"#{ }\"\n");
            // 複数行の `#{}` は対象外。
            expect_no_offenses(COP, "q = \"#{ a +\n  b }\"\n");
        }

        /// 既定は中に空白 1 つ、空のブレースだけは空白なし。空白の「過剰」は
        /// この cop の担当ではない。
        #[test]
        fn space_inside_hash_literal_braces() {
            const COP: &str = "Layout/SpaceInsideHashLiteralBraces";
            CopCase::new(
                COP,
                "h = {a: 1}\n",
                vec![
                    Annotation::new(1, 5, 1, "Space inside { missing."),
                    Annotation::new(1, 10, 1, "Space inside } missing."),
                ],
            )
            .run();
            expect_correction(COP, "h = {a: 1}\n", "h = { a: 1 }\n");
            expect_no_offenses(COP, "h = { a: 1 }\n");
            // 空白の数はこの cop の担当ではない。
            expect_no_offenses(COP, "h = {  a: 1  }\n");
            expect_no_offenses(COP, "h = {}\n");
            expect_no_offenses(COP, "h = {\n  a: 1,\n}\n");
            // 空のブレースの中身は、空白だけなら報告される。
            CopCase::annotated(
                COP,
                r#"
            h5 = {  }
                  ^^ Space inside empty hash literal braces detected.
            "#,
            )
            .run();
            expect_correction(COP, "h5 = {  }\n", "h5 = {}\n");
            // ブレース無しのハッシュには内側が無い。
            expect_no_offenses(COP, "foo(a: 1)\n");
        }
    }

    /// 例外処理キーワードとヒアドキュメントのインデント。期待値は本家 1.89.0 の
    /// `--only <cop> --format json` と `-A` の実測。
    mod layout_exception_and_heredoc {
        use super::*;

        #[test]
        fn empty_lines_around_exception_handling_keywords() {
            const COP: &str = "Layout/EmptyLinesAroundExceptionHandlingKeywords";
            CopCase::new(
                COP,
                "def foo\n  a\n\nrescue\n\n  b\nend\n",
                vec![
                    Annotation::new(3, 1, 0, "Extra empty line detected before the `rescue`."),
                    Annotation::new(5, 1, 0, "Extra empty line detected after the `rescue`."),
                ],
            )
            .locations(&[(3, 1, 4, 1), (5, 1, 6, 1)])
            .lengths(&[1, 1])
            .run();
            expect_correction(
                COP,
                "def foo\n  a\n\nrescue\n\n  b\nend\n",
                "def foo\n  a\nrescue\n  b\nend\n",
            );
            expect_no_offenses(COP, "def foo\n  a\nrescue\n  b\nend\n");
            // `class` の本体に付いた `rescue` は本家が見に行かない。
            expect_no_offenses(COP, "class C\n  a\n\nrescue\n\n  b\nend\n");
        }

        /// `<<~` は本体のインデントを、`<<-` と `<<` は記法そのものを直す。
        #[test]
        fn heredoc_indentation() {
            const COP: &str = "Layout/HeredocIndentation";
            CopCase::new(
                COP,
                "def m\n  x = <<~X\n      hi\n    X\nend\n",
                vec![Annotation::new(
                    3,
                    1,
                    8,
                    "Use 2 spaces for indentation in a heredoc.",
                )],
            )
            .locations(&[(3, 1, 4, 1)])
            .lengths(&[9])
            .run();
            expect_correction(
                COP,
                "def m\n  x = <<~X\n      hi\n    X\nend\n",
                "def m\n  x = <<~X\n    hi\n    X\nend\n",
            );
            CopCase::new(
                COP,
                "def m\n  y = <<-Y\nhello\n  Y\nend\n",
                vec![Annotation::new(
                    3,
                    1,
                    5,
                    "Use 2 spaces for indentation in a heredoc by using `<<~` instead of `<<-`.",
                )],
            )
            .locations(&[(3, 1, 4, 1)])
            .lengths(&[6])
            .run();
            expect_correction(
                COP,
                "def m\n  y = <<-Y\nhello\n  Y\nend\n",
                "def m\n  y = <<~Y\n    hello\n  Y\nend\n",
            );
            // 本体に既にインデントがある `<<` 系は対象外。空の本体も同じ。
            expect_no_offenses(COP, "def m\n  z = <<Z\n  qq\nZ\nend\n");
            expect_no_offenses(COP, "x = <<~X\nX\n");
        }
    }

    const COP: &str = "Style/EmptyMethod";

    #[test]
    fn a_multiline_empty_definition_is_reported_whole() {
        expect_offense(
            COP,
            r#"
            def foo(bar)
            ^^^^^^^^^^^^ Put empty method definitions on a single line.
            end
            "#,
        );
        expect_correction(COP, "def foo(bar)\nend\n", "def foo(bar); end\n");
        expect_correction(COP, "def self.foo bar\nend\n", "def self.foo bar; end\n");
        // 空の括弧は本家も落とす。
        expect_correction(COP, "def foo()\nend\n", "def foo; end\n");
    }

    /// 本体を持つ定義と、`def` / `end` と同じ行にコメントがある定義は対象外。
    /// `contains_comment?` は行で見るので、行末コメントも免除になる。
    #[test]
    fn a_body_or_a_comment_on_either_line_exempts_the_definition() {
        expect_no_offenses(COP, "def foo; end\n");
        expect_no_offenses(COP, "def foo\n  1\nend\n");
        expect_no_offenses(COP, "def foo\n  # note\nend\n");
        expect_no_offenses(COP, "def foo # note\nend\n");
        expect_no_offenses(COP, "def foo\nend # note\n");
        // `rescue` は本体なので空ではない。
        expect_no_offenses(COP, "def foo\nrescue\nend\n");
    }

    /// `expanded` では逆に 1 行の定義を報告し、`end` を `def` の桁に置く。
    #[test]
    fn the_expanded_style_puts_the_end_on_its_own_line() {
        CopCase::annotated(
            COP,
            r#"
            class Foo
              def bar; end
              ^^^^^^^^^^^^ Put the `end` of empty method definitions on the next line.
            end
            "#,
        )
        .config("Style/EmptyMethod:\n  EnforcedStyle: expanded\n")
        .corrected("class Foo\n  def bar\n  end\nend\n")
        .run();
    }
}

/// `Style/NumericLiteralPrefix`: 基数の接頭辞は小文字、10 進数は接頭辞なし。
///
/// 期待値は本家 1.89.0 の `--only Style/NumericLiteralPrefix` と `-A` の実測。
mod numeric_literal_prefix {
    use super::*;

    const COP: &str = "Style/NumericLiteralPrefix";

    #[test]
    fn each_base_has_its_own_message() {
        expect_offense(
            COP,
            r#"
            a = 0O1234
                ^^^^^^ Use 0o for octal literals.
            "#,
        );
        expect_correction(COP, "a = 0X12AB\n", "a = 0x12AB\n");
        expect_correction(COP, "a = 0B10101\n", "a = 0b10101\n");
        expect_correction(COP, "a = 0D1234\n", "a = 1234\n");
        expect_correction(COP, "a = 01234\n", "a = 0o1234\n");
    }

    /// `integer_part` は符号を落としてから `e` / `.` の手前で切る。符号付きは
    /// 本家でも `sub` が効かず、報告だけで内容が変わらない。
    #[test]
    fn a_sign_is_part_of_the_literal_and_leaves_it_unchanged() {
        expect_offense(
            COP,
            r#"
            a = -0X1F
                ^^^^^ Use 0x for hexadecimal literals.
            "#,
        );
        // 補正は元と同じ文字列を書き戻すので、本家もここで無限ループを検出する。
        // 突き合わせられるのは報告と correctable まで。
        // 空白が挟まると `integer_part` が接頭辞に届かない。
        expect_no_offenses(COP, "a = - 0X1F\n");
        // `0XE1` は `E` で切られて `0X` になり、桁が残らない。
        expect_no_offenses(COP, "a = 0XE1\n");
        expect_no_offenses(COP, "a = 0Xab\n");
        expect_no_offenses(COP, "a = 0X1_F\n");
        expect_no_offenses(COP, "a = 0\n");
        expect_no_offenses(COP, "a = 0o17\n");
    }

    /// `zero_only` では `0o` の側が報告される。
    #[test]
    fn the_zero_only_style_reverses_the_octal_rule() {
        CopCase::annotated(
            COP,
            r#"
            a = 0o1234
                ^^^^^^ Use 0 for octal literals.
            "#,
        )
        .config("Style/NumericLiteralPrefix:\n  EnforcedOctalStyle: zero_only\n")
        .corrected("a = 01234\n")
        .run();
    }
}

/// `Style/PerlBackrefs`: `$1` ではなく `Regexp.last_match` を使う。
///
/// 期待値は本家 1.89.0 の `--only Style/PerlBackrefs` と `-A` の実測。
mod perl_backrefs {
    use super::*;

    const COP: &str = "Style/PerlBackrefs";

    #[test]
    fn numbered_and_named_references_are_reported() {
        expect_offense(
            COP,
            r#"
            puts $1
                 ^^ Prefer `Regexp.last_match(1)` over `$1`.
            "#,
        );
        expect_correction(COP, "puts $&\n", "puts Regexp.last_match(0)\n");
        expect_correction(COP, "puts $MATCH\n", "puts Regexp.last_match(0)\n");
        expect_correction(COP, "puts $`\n", "puts Regexp.last_match.pre_match\n");
        expect_correction(COP, "puts $'\n", "puts Regexp.last_match.post_match\n");
        expect_correction(
            COP,
            "puts $POSTMATCH\n",
            "puts Regexp.last_match.post_match\n",
        );
    }

    /// `$+` は置き換え先が無いので対象外。ほかのグローバル変数も同じ。
    #[test]
    fn references_without_an_equivalent_are_left_alone() {
        expect_no_offenses(COP, "puts $+\n");
        expect_no_offenses(COP, "puts $LAST_PAREN_MATCH\n");
        expect_no_offenses(COP, "puts $0\n");
        expect_no_offenses(COP, "puts $stdout\n");
    }

    /// 波括弧なしの補間は補正で波括弧を補う。クラス／モジュールの中では
    /// 定数を根から綴る。
    #[test]
    fn braces_and_the_root_prefix_are_supplied_by_the_correction() {
        expect_correction(
            COP,
            "x = \"a#$1b\"\n",
            "x = \"a#{Regexp.last_match(1)}b\"\n",
        );
        expect_correction(
            COP,
            "x = \"a#{$1}b\"\n",
            "x = \"a#{Regexp.last_match(1)}b\"\n",
        );
        CopCase::annotated(
            COP,
            r#"
            class Foo
              def bar
                $1
                ^^ Prefer `::Regexp.last_match(1)` over `$1`.
              end
            end
            "#,
        )
        .corrected("class Foo\n  def bar\n    ::Regexp.last_match(1)\n  end\nend\n")
        .run();
    }
}

/// `Style/StringConcatenation`: `+` での連結より補間。
///
/// 期待値は本家 1.89.0 の `--only Style/StringConcatenation` と `-A` の実測。
mod string_concatenation {
    use super::*;

    const COP: &str = "Style/StringConcatenation";

    #[test]
    fn a_literal_on_either_side_is_reported_at_the_whole_chain() {
        expect_offense(
            COP,
            r#"
            a = 'x' + y + 'z'
                ^^^^^^^^^^^^^ Prefer string interpolation to string concatenation.
            "#,
        );
        expect_correction(COP, "a = 'x' + y\n", "a = \"x#{y}\"\n");
        expect_correction(COP, "a = y + 'x'\n", "a = \"#{y}x\"\n");
        expect_correction(COP, "a = ?a + y\n", "a = \"a#{y}\"\n");
        expect_correction(COP, "a = 'x'.+(y)\n", "a = \"x#{y}\"\n");
    }

    /// 単引用符の中身は `\\` `\"` `#{` だけを逃がし、二重引用符の中身は
    /// `inspect` で書き戻す。
    #[test]
    fn each_quoting_escapes_what_the_interpolation_would_read() {
        expect_correction(
            COP,
            "a = 'has \"quotes\"' + y\n",
            "a = \"has \\\"quotes\\\"#{y}\"\n",
        );
        expect_correction(
            COP,
            "a = 'interp #{x}' + y\n",
            "a = \"interp \\#{x}#{y}\"\n",
        );
        // 補間の中の文字列はその値だけが残る。
        expect_correction(COP, "a = 'x' + \"#{'q'}\"\n", "a = \"xq\"\n");
        // `\xFF` は文字にならないバイトなので、`inspect` と同じく書かれた通りに戻す。
        expect_correction(
            COP,
            "a = (bytes + \"\\xFF\").unpack('R')\n",
            "a = (\"#{bytes}\\xFF\").unpack('R')\n",
        );
        // UTF-8 のソースでは、組み合わせて文字になるバイト列は文字として戻る。
        expect_correction(COP, "x = \"\\xE5\\xBE\\x8C\" + y\n", "x = \"後#{y}\"\n");
        // バイナリ宣言のあるソースでは 1 バイトが 1 文字なので、繋がらない。
        expect_correction(
            COP,
            "# coding: ASCII-8BIT\nx = \"\\xE5\\xBE\\x8C\" + y\n",
            "# coding: ASCII-8BIT\nx = \"\\xE5\\xBE\\x8C#{y}\"\n",
        );
    }

    /// 1 行に収まらない文字列は `str` ではなく `dstr` なので対象外。行末の
    /// `+` は `Style/LineEndConcatenation` の担当。
    #[test]
    fn a_literal_spread_over_lines_is_not_a_plain_string() {
        expect_no_offenses(COP, "a = \"one\ntwo\" + b\n");
        expect_no_offenses(COP, "a = 'x' +\n    'y'\n");
        expect_no_offenses(COP, "a = \"a#{z}\" + y\n");
        // 演算子に見えて演算ではない `return +\"\"`。
        expect_no_offenses(COP, "def m\n  return +\"\"\nend\n");
    }

    /// ヒアドキュメントは補正できないが報告はされる。内側の連結は外側が
    /// 直したあとなので、このパスでは補正しない。
    #[test]
    fn heredocs_and_already_corrected_ancestors_are_reported_without_a_correction() {
        CopCase::annotated(
            COP,
            r#"
            a = 'x' + <<~X
                ^^^^^^^^^^ Prefer string interpolation to string concatenation.
              body
            X
            "#,
        )
        .correctable(false)
        .run();
        CopCase::annotated(
            COP,
            r#"
            a = ('x' + y) + 'z'
                ^^^^^^^^^^^^^^^ Prefer string interpolation to string concatenation.
                 ^^^^^^^ Prefer string interpolation to string concatenation.
            "#,
        )
        .without_offense_check()
        .corrected("a = \"#{\"x#{y}\"}z\"\n")
        .run();
    }
}

/// `Style/Lambda`: 1 行なら `->`、複数行なら `lambda`。
///
/// 期待値は本家 1.89.0 の `--only Style/Lambda` と `-A` の実測。
mod lambda {
    use super::*;

    const COP: &str = "Style/Lambda";

    #[test]
    fn the_selector_alone_is_reported() {
        expect_offense(
            COP,
            r#"
            a = lambda { |x| x }
                ^^^^^^ Use the `-> { ... }` lambda literal syntax for single line lambdas.
            "#,
        );
        expect_offense(
            COP,
            r#"
            a = ->(x) do
                ^^ Use the `lambda` method for multiline lambdas.
              x
            end
            "#,
        );
        expect_no_offenses(COP, "a = ->(x) { x }\n");
        expect_no_offenses(COP, "a = lambda do |x|\n  x\nend\n");
        // 受け手がついた `lambda` は綴りが一致しないので対象外。
        expect_no_offenses(COP, "a = Foo.lambda { |x| x }\n");
    }

    #[test]
    fn the_method_form_becomes_a_literal_with_parenthesized_parameters() {
        expect_correction(COP, "a = lambda { |x| x }\n", "a = ->(x) { x }\n");
        expect_correction(COP, "a = lambda { 1 }\n", "a = -> { 1 }\n");
        expect_correction(COP, "a = lambda { |x; y| x }\n", "a = ->(x; y) { x }\n");
        // 引数のない `||` は引数無しと同じ扱い。
        expect_correction(COP, "a = lambda { || 1 }\n", "a = -> { || 1 }\n");
    }

    #[test]
    fn the_literal_form_moves_its_parameters_into_the_block() {
        expect_correction(
            COP,
            "a = ->(x) do\n  x\nend\n",
            "a = lambda do |x|\n  x\nend\n",
        );
        expect_correction(
            COP,
            "a = -> x do\n  x\nend\n",
            "a = lambda do |x|\n  x\nend\n",
        );
        // `->do` と `->(x)do` は `lambdado` にならないよう空白を補う。
        expect_correction(COP, "a = ->do\n  1\nend\n", "a = lambda do\n  1\nend\n");
        expect_correction(
            COP,
            "a = ->(x)do\n  x\nend\n",
            "a = lambda do |x|\n  x\nend\n",
        );
        expect_correction(COP, "a = ->() do\n  1\nend\n", "a = lambda do\n  1\nend\n");
    }

    /// 括弧なしの呼び出しの引数だったときだけ、`do ... end` を波括弧に替える。
    #[test]
    fn a_block_handed_to_an_unparenthesized_call_becomes_braces() {
        expect_correction(
            COP,
            "foo ->(x) do\n  x\nend\n",
            "foo lambda { |x|\n  x\n}\n",
        );
        expect_correction(
            COP,
            "foo(->(x) do\n  x\nend)\n",
            "foo(lambda do |x|\n  x\nend)\n",
        );
        expect_correction(
            COP,
            "a = ->(x) do\n  x\nend.call\n",
            "a = lambda do |x|\n  x\nend.call\n",
        );
    }
}

/// `Style/NumericPredicate`: `== 0` より `zero?`。
///
/// 期待値は本家 1.89.0 の `--only Style/NumericPredicate` と `-A` の実測。
mod numeric_predicate {
    use super::*;

    const COP: &str = "Style/NumericPredicate";

    #[test]
    fn comparisons_against_zero_are_reported_either_way_round() {
        expect_offense(
            COP,
            r#"
            a = foo == 0
                ^^^^^^^^ Use `foo.zero?` instead of `foo == 0`.
            "#,
        );
        expect_correction(COP, "a = 0 > foo\n", "a = foo.negative?\n");
        expect_correction(COP, "a = 0 < foo\n", "a = foo.positive?\n");
        expect_correction(COP, "a = bar.baz > 0\n", "a = bar.baz.positive?\n");
        // `-0` も `0` と同じ整数リテラル。
        expect_correction(COP, "a = foo == -0\n", "a = foo.zero?\n");
    }

    /// 演算子呼び出しは括弧で包んでからでないと述語を繋げられない。
    #[test]
    fn an_operator_call_gains_parentheses() {
        expect_correction(COP, "a = b + c == 0\n", "a = (b + c).zero?\n");
        expect_correction(COP, "a = b[c] == 0\n", "a = (b[c]).zero?\n");
        // 既に括弧のある呼び出しはそのまま。
        expect_correction(COP, "a = b.+(c) == 0\n", "a = b.+(c).zero?\n");
        expect_correction(COP, "a = (b + c) == 0\n", "a = (b + c).zero?\n");
        expect_correction(COP, "a = -b == 0\n", "a = -b.zero?\n");
    }

    /// グローバル変数、`!=`、浮動小数点数は対象外。
    #[test]
    fn what_is_not_a_numeric_comparison() {
        expect_no_offenses(COP, "a = $x == 0\n");
        expect_no_offenses(COP, "a = 0 == $x\n");
        expect_no_offenses(COP, "a = foo != 0\n");
        expect_no_offenses(COP, "a = foo == 0.0\n");
        expect_no_offenses(COP, "a = foo == 1\n");
    }

    /// `comparison` では逆に述語を比較へ書き戻す。`!` の下では括弧が要る。
    #[test]
    fn the_comparison_style_writes_the_predicate_back_out() {
        CopCase::annotated(
            COP,
            r#"
            a = foo.zero?
                ^^^^^^^^^ Use `foo == 0` instead of `foo.zero?`.
            "#,
        )
        .config("Style/NumericPredicate:\n  EnforcedStyle: comparison\n")
        .corrected("a = foo == 0\n")
        .run();
        CopCase::annotated(
            COP,
            r#"
            a = !foo.negative?
                 ^^^^^^^^^^^^^ Use `(foo < 0)` instead of `foo.negative?`.
            "#,
        )
        .config("Style/NumericPredicate:\n  EnforcedStyle: comparison\n")
        .corrected("a = !(foo < 0)\n")
        .run();
    }
}
/// `Style/RescueStandardError`: 既定では例外クラスを省いた `rescue` を報告する。
///
/// 期待値は本家 1.89.0 の `--only Style/RescueStandardError` と `-A` の実測。
mod rescue_standard_error {
    use super::*;

    const COP: &str = "Style/RescueStandardError";

    #[test]
    fn a_bare_rescue_is_reported_at_the_keyword() {
        expect_offense(
            COP,
            r#"
            begin
              foo
            rescue
            ^^^^^^ Avoid rescuing without specifying an error class.
              bar
            end
            "#,
        );
        expect_correction(
            COP,
            "begin\n  foo\nrescue\n  bar\nend\n",
            "begin\n  foo\nrescue StandardError\n  bar\nend\n",
        );
        // 変数だけを受ける `rescue => e` もクラスを名指ししていない。
        expect_correction(
            COP,
            "begin\n  foo\nrescue => e\n  bar\nend\n",
            "begin\n  foo\nrescue StandardError => e\n  bar\nend\n",
        );
    }

    /// 修飾子の `rescue` とクラスを名指しした `rescue` は対象外。
    #[test]
    fn a_modifier_or_a_named_class_is_left_alone() {
        expect_no_offenses(COP, "x = foo rescue nil\n");
        expect_no_offenses(COP, "begin\n  foo\nrescue Foo\n  bar\nend\n");
        expect_no_offenses(COP, "begin\n  foo\nrescue StandardError\n  bar\nend\n");
    }

    /// `implicit` では逆に `StandardError` だけを名指しした `rescue` を報告する。
    #[test]
    fn the_implicit_style_takes_the_class_back_off() {
        CopCase::annotated(
            COP,
            r#"
            begin
              foo
            rescue StandardError
            ^^^^^^^^^^^^^^^^^^^^ Omit the error class when rescuing `StandardError` by itself.
              bar
            end
            "#,
        )
        .config("Style/RescueStandardError:\n  EnforcedStyle: implicit\n")
        .corrected("begin\n  foo\nrescue\n  bar\nend\n")
        .run();
        CopCase::new(
            COP,
            "begin\n  foo\nrescue StandardError, Foo\n  bar\nend\n",
            vec![],
        )
        .config("Style/RescueStandardError:\n  EnforcedStyle: implicit\n")
        .run();
    }
}

/// `Style/HashAsLastArrayItem`: 配列の最後のハッシュは波括弧で包む。
///
/// 期待値は本家 1.89.0 の `--only Style/HashAsLastArrayItem` と `-A` の実測。
mod hash_as_last_array_item {
    use super::*;

    const COP: &str = "Style/HashAsLastArrayItem";

    #[test]
    fn the_trailing_pairs_are_reported_as_one_hash() {
        expect_offense(
            COP,
            r#"
            a = [1, 2, one: 1, two: 2]
                       ^^^^^^^^^^^^^^ Wrap hash in `{` and `}`.
            "#,
        );
        expect_correction(
            COP,
            "a = [1, 2, one: 1, two: 2]\n",
            "a = [1, 2, {one: 1, two: 2}]\n",
        );
        expect_correction(COP, "a = [one: 1]\n", "a = [{one: 1}]\n");
    }

    /// 直前の要素もハッシュなら、複数のハッシュが並ぶ配列とみなして触らない。
    /// `**` で始まるハッシュ、角括弧でない配列も対象外。
    #[test]
    fn what_the_cop_leaves_alone() {
        expect_no_offenses(COP, "a = [1, 2, { one: 1 }]\n");
        expect_no_offenses(COP, "a = [{ one: 1 }, { two: 2 }]\n");
        expect_no_offenses(COP, "a = [1, { one: 1 }, two: 2]\n");
        expect_no_offenses(COP, "a = [1, **opts]\n");
        expect_no_offenses(COP, "a = %w[x y]\n");
        expect_no_offenses(COP, "a = [1, {}]\n");
    }

    /// 配列と行が違うときは、波括弧を独立した行に置いて字下げを合わせる。
    #[test]
    fn a_hash_on_its_own_lines_gets_the_braces_on_theirs() {
        expect_correction(
            COP,
            "a = [1,\n     one: 1,\n     two: 2]\n",
            "a = [1,\n     {\n     one: 1,\n     two: 2\n     }]\n",
        );
    }

    /// `no_braces` では逆に波括弧を落とし、続く読点も片付ける。
    #[test]
    fn the_no_braces_style_removes_them() {
        CopCase::annotated(
            COP,
            r#"
            a = [1, { one: 1 }]
                    ^^^^^^^^^^ Omit the braces around the hash.
            "#,
        )
        .config("Style/HashAsLastArrayItem:\n  EnforcedStyle: no_braces\n")
        .corrected("a = [1,  one: 1 ]\n")
        .run();
    }
}
/// `Style/GlobalStdStream`: `STDOUT` ではなく `$stdout`。
///
/// 期待値は本家 1.89.0 の `--only Style/GlobalStdStream` と `-A` の実測。
mod global_std_stream {
    use super::*;

    const COP: &str = "Style/GlobalStdStream";

    #[test]
    fn the_three_streams_are_reported_bare_or_from_the_root() {
        expect_offense(
            COP,
            r#"
            STDOUT.puts 'a'
            ^^^^^^ Use `$stdout` instead of `STDOUT`.
            "#,
        );
        expect_correction(COP, "::STDERR.puts 'b'\n", "$stderr.puts 'b'\n");
        expect_correction(COP, "STDIN.gets\n", "$stdin.gets\n");
    }

    /// 名前空間つきの定数は別物。`$stdout = STDOUT` はその代入そのものなので残す。
    #[test]
    fn a_qualified_constant_and_the_defining_assignment_are_left_alone() {
        expect_no_offenses(COP, "Foo::STDOUT.puts 'c'\n");
        expect_no_offenses(COP, "$stdout = STDOUT\n");
        // 代入される側の定数は `casgn` で、`const` ノードにならない。
        expect_no_offenses(COP, "STDOUT = io\n");
        expect_no_offenses(COP, "STDERR = new_io file\n");
        // 名前が食い違う代入と、根から綴った右辺は対象。
        expect_correction(COP, "$stderr = STDOUT\n", "$stderr = $stdout\n");
        expect_correction(COP, "$stdout = ::STDOUT\n", "$stdout = $stdout\n");
    }
}

/// `Style/PreferredHashMethods`: 既定では `has_key?` より `key?`。
///
/// 期待値は本家 1.89.0 の `--only Style/PreferredHashMethods` と `-A` の実測。
mod preferred_hash_methods {
    use super::*;

    const COP: &str = "Style/PreferredHashMethods";

    #[test]
    fn the_verbose_predicates_are_reported_at_the_selector() {
        expect_offense(
            COP,
            r#"
            h.has_key?(:a)
              ^^^^^^^^ Use `Hash#key?` instead of `Hash#has_key?`.
            "#,
        );
        expect_correction(COP, "h.has_value?(1)\n", "h.value?(1)\n");
        expect_correction(COP, "has_key? :a\n", "key? :a\n");
        expect_correction(COP, "h&.has_key?(:a)\n", "h&.key?(:a)\n");
    }

    /// 引数がちょうど 1 つでなければ `Hash` の述語ではない。
    #[test]
    fn the_argument_count_has_to_be_one() {
        expect_no_offenses(COP, "h.has_key?\n");
        expect_no_offenses(COP, "h.has_key?(:a, :b)\n");
        expect_no_offenses(COP, "h.key?(:a)\n");
    }

    /// `verbose` では逆向きになる。
    #[test]
    fn the_verbose_style_reverses_the_rule() {
        CopCase::annotated(
            COP,
            r#"
            h.key?(:a)
              ^^^^ Use `Hash#has_key?` instead of `Hash#key?`.
            "#,
        )
        .config("Style/PreferredHashMethods:\n  EnforcedStyle: verbose\n")
        .corrected("h.has_key?(:a)\n")
        .run();
    }
}

/// コメントとインデントの Layout cop。期待値は本家 1.89.0 の
/// `--only <cop> --format json` と `-A` の実測。
mod layout_comments_and_indentation {
    use super::*;

    /// 連続する空コメントは 1 塊として判定され、説明を挟むと余白コメントになるので
    /// どれも報告されない。行を占有するコメントは行ごと、行末のものは前後の空白ごと消える。
    #[test]
    fn empty_comment() {
        const COP: &str = "Layout/EmptyComment";
        CopCase::annotated(
            COP,
            r#"
            #
            ^ Source code comment is empty.
            class Foo
            end
            "#,
        )
        .run();
        expect_correction(COP, "#\nclass Foo\nend\n", "class Foo\nend\n");
        // 余白コメントは既定で許され、罫線コメントも `#` 1 個ではないので対象外。
        expect_no_offenses(COP, "#\n# Description of `Foo` class.\n#\nclass Foo\nend\n");
        expect_no_offenses(COP, "def m\n  ###\nend\n");
        CopCase::new(
            COP,
            "x = 1 # \n",
            vec![Annotation::new(1, 7, 2, "Source code comment is empty.")],
        )
        .run();
        expect_correction(COP, "x = 1 # \n", "x = 1\n");
    }

    /// `AllowBorderComment: false` にすると `#` の並びも空コメントになる。
    #[test]
    fn empty_comment_without_border_comments() {
        CopCase::annotated(
            "Layout/EmptyComment",
            r#"
            ###
            ^^^ Source code comment is empty.
            "#,
        )
        .config("Layout/EmptyComment:\n  AllowBorderComment: false\n")
        .corrected("")
        .run();
    }

    /// 閉じ括弧が行頭に無いときだけ報告する。直前がセミコロンなら見送る。
    #[test]
    fn block_end_newline() {
        const COP: &str = "Layout/BlockEndNewline";
        CopCase::new(
            COP,
            "blah do |i|\n  foo(i) end\n",
            vec![Annotation::new(
                2,
                10,
                3,
                "Expression at 2, 10 should be on its own line.",
            )],
        )
        .run();
        expect_correction(
            COP,
            "blah do |i|\n  foo(i) end\n",
            "blah do |i|\n  foo(i)\nend\n",
        );
        expect_correction(COP, "blah { |i|\n  foo(i) }\n", "blah { |i|\n  foo(i)\n}\n");
        expect_no_offenses(COP, "blah do |i|\n  foo(i)\nend\n");
        expect_no_offenses(COP, "blah { |i| foo(i) }\n");
        // 最後の文の後ろがセミコロンなら本家は見送る。
        expect_no_offenses(COP, "blah do |i|\n  foo(i); end\n");
    }

    /// 演算子の前後どちらの空白でも報告し、複数行のリテラルは 1 行に畳んでから測る。
    #[test]
    fn space_inside_range_literal() {
        const COP: &str = "Layout/SpaceInsideRangeLiteral";
        CopCase::annotated(
            COP,
            r#"
            x = 1 .. 3
                ^^^^^^ Space inside range literal.
            "#,
        )
        .corrected("x = 1..3\n")
        .run();
        expect_correction(COP, "y = 'a' ...'z'\n", "y = 'a'...'z'\n");
        expect_no_offenses(COP, "x = 1..3\n");
        expect_no_offenses(COP, "x = 1...3\n");
        // 条件に書いた範囲はフリップフロップで、範囲リテラルの cop は見に行かない。
        expect_no_offenses(COP, "if a .. b\n  c\nend\n");
        expect_no_offenses(COP, "d while e .. f\n");
    }

    #[test]
    fn space_after_not() {
        const COP: &str = "Layout/SpaceAfterNot";
        CopCase::annotated(
            COP,
            r#"
            y = ! foo
                ^^^^^ Do not leave space between `!` and its argument.
            "#,
        )
        .corrected("y = !foo\n")
        .run();
        expect_no_offenses(COP, "y = !foo\n");
        expect_no_offenses(COP, "y = !(foo)\n");
        expect_no_offenses(COP, "y = not foo\n");
    }

    /// 既定は spaces なので、行頭のタブを報告して空白へ直す。文字列リテラルの中は対象外だが、
    /// ヒアドキュメントの終端行のインデントはコードとして数える。
    #[test]
    fn indentation_style() {
        const COP: &str = "Layout/IndentationStyle";
        CopCase::new(
            COP,
            "def x\n\ty = 1\nend\n",
            vec![Annotation::new(2, 1, 1, "Tab detected in indentation.")],
        )
        .run();
        expect_correction(COP, "def x\n\ty = 1\nend\n", "def x\n  y = 1\nend\n");
        expect_no_offenses(COP, "def x\n  y = 1\nend\n");
        expect_no_offenses(COP, "x = <<~X\n\thi\nX\n");
        CopCase::new(
            COP,
            "x = <<~X\n  hi\n\tX\n",
            vec![Annotation::new(3, 1, 1, "Tab detected in indentation.")],
        )
        .run();
    }

    /// tabs では行頭の空白の方が報告される。
    #[test]
    fn indentation_style_with_tabs() {
        CopCase::annotated(
            "Layout/IndentationStyle",
            r#"
            def x
              y = 1
            ^^ Space detected in indentation.
            end
            "#,
        )
        .config("Layout/IndentationStyle:\n  EnforcedStyle: tabs\n")
        .corrected("def x\n\ty = 1\nend\n")
        .run();
    }

    #[test]
    fn initial_indentation() {
        const COP: &str = "Layout/InitialIndentation";
        CopCase::new(
            COP,
            "  x = 1\n  y = 2\n",
            vec![Annotation::new(
                1,
                3,
                1,
                "Indentation of first line in file detected.",
            )],
        )
        .run();
        // `expect_correction` はソースを dedent するので、行頭の字下げそのものを見る
        // このケースだけは `CopCase` を直に組む。
        CopCase::new(COP, "  x = 1\n  y = 2\n", Vec::new())
            .without_offense_check()
            .corrected("x = 1\n  y = 2\n")
            .run();
        expect_no_offenses(COP, "x = 1\n  y = 2\n");
        // 先頭のコメントはトークンとして数えないので、次の行の字下げが見られる。
        expect_no_offenses(COP, "# c\nx = 1\n");
    }
}

/// `Layout/EmptyLinesAround*Body` の 5 本。期待値は本家 1.89.0 の
/// `--only <cop> --format json` と `-A` の実測。
mod layout_empty_lines_around_bodies {
    use super::*;

    /// 既定はどれも `no_empty_lines` で、本体の前後の空行を 1 行ずつ落とす。
    #[test]
    fn extra_empty_lines_at_both_ends() {
        for (cop, kind, source) in [
            (
                "Layout/EmptyLinesAroundClassBody",
                "class",
                "class C\n\n  def m; end\n\nend\n",
            ),
            (
                "Layout/EmptyLinesAroundModuleBody",
                "module",
                "module M\n\n  X = 1\n\nend\n",
            ),
            (
                "Layout/EmptyLinesAroundMethodBody",
                "method",
                "def foo\n\n  1\n\nend\n",
            ),
            (
                "Layout/EmptyLinesAroundBeginBody",
                "`begin`",
                "begin\n\n  y\n\nend\n",
            ),
            (
                "Layout/EmptyLinesAroundBlockBody",
                "block",
                "foo do\n\n  z\n\nend\n",
            ),
        ] {
            CopCase::new(
                cop,
                source,
                vec![
                    Annotation::new(
                        2,
                        1,
                        0,
                        format!("Extra empty line detected at {kind} body beginning."),
                    ),
                    Annotation::new(
                        4,
                        1,
                        0,
                        format!("Extra empty line detected at {kind} body end."),
                    ),
                ],
            )
            .locations(&[(2, 1, 3, 1), (4, 1, 5, 1)])
            .lengths(&[1, 1])
            .run();
        }
    }

    #[test]
    fn corrections_take_one_line_off_each_end() {
        expect_correction(
            "Layout/EmptyLinesAroundClassBody",
            "class C\n\n  def m; end\n\nend\n",
            "class C\n  def m; end\nend\n",
        );
        expect_correction(
            "Layout/EmptyLinesAroundBlockBody",
            "foo do\n\n  z\n\nend\n",
            "foo do\n  z\nend\n",
        );
        expect_no_offenses(
            "Layout/EmptyLinesAroundClassBody",
            "class C\n  def m; end\nend\n",
        );
        expect_no_offenses("Layout/EmptyLinesAroundMethodBody", "def foo\n  1\nend\n");
    }

    /// 空の本体は 1 行だけの空行が始端と終端の両方になるが、`add_offense` が同じレンジを
    /// 一度しか受けないので始端の側だけが残る。
    #[test]
    fn an_empty_body_is_reported_once() {
        CopCase::new(
            "Layout/EmptyLinesAroundMethodBody",
            "def foo\n\nend\n",
            vec![Annotation::new(
                2,
                1,
                0,
                "Extra empty line detected at method body beginning.",
            )],
        )
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .run();
    }

    /// 受け側が複数行でも、ブロックは `{` と `}` が同じ行なら単一行として扱われる。
    #[test]
    fn a_block_opened_on_the_last_line_of_its_receiver_is_single_line() {
        expect_no_offenses(
            "Layout/EmptyLinesAroundBlockBody",
            "X = [\n  1,\n].map { |p| p }\n\nY = 1\n",
        );
    }

    /// エンドレスメソッドは `=` の次の行が空いているときだけ報告する。
    #[test]
    fn an_endless_method_reports_the_line_after_the_assignment() {
        CopCase::new(
            "Layout/EmptyLinesAroundMethodBody",
            "def foo =\n\n  1\n",
            vec![Annotation::new(
                2,
                1,
                0,
                "Extra empty line detected at method body beginning.",
            )],
        )
        .target_ruby("3.0")
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .corrected("def foo =\n  1\n")
        .run();
        CopCase::new(
            "Layout/EmptyLinesAroundMethodBody",
            "def foo = 1\n",
            Vec::new(),
        )
        .target_ruby("3.0")
        .run();
    }

    /// `empty_lines_special` は最初の定義の前の空行を要求し、本体の終端にも空行を求める。
    #[test]
    fn the_special_style_defers_to_the_first_definition() {
        CopCase::new(
            "Layout/EmptyLinesAroundClassBody",
            "class D\n  X = 1\n  def m; end\nend\n",
            vec![
                Annotation::new(3, 1, 1, "Empty line missing before first def definition"),
                Annotation::new(4, 1, 1, "Empty line missing at class body end."),
            ],
        )
        .locations(&[(3, 1, 3, 1), (4, 1, 4, 1)])
        .lengths(&[1, 1])
        .config("Layout/EmptyLinesAroundClassBody:\n  EnforcedStyle: empty_lines_special\n")
        .corrected("class D\n  X = 1\n\n  def m; end\n\nend\n")
        .run();
    }
}

/// 句読点まわりの空白と行頭コメント。期待値は本家 1.89.0 の
/// `--only <cop> --format json` と `-A` の実測。
mod layout_punctuation_spacing {
    use super::*;

    #[test]
    fn space_after_colon() {
        const COP: &str = "Layout/SpaceAfterColon";
        CopCase::new(
            COP,
            "def f(a:, b:2)\n  {a:3}\nend\n",
            vec![
                Annotation::new(1, 12, 1, "Space missing after colon."),
                Annotation::new(2, 5, 1, "Space missing after colon."),
            ],
        )
        .corrected("def f(a:, b: 2)\n  {a: 3}\nend\n")
        .run();
        expect_no_offenses(COP, "def f(a:, b: 2)\n  {a: 3}\nend\n");
        // `=>` で書いたペアにはコロンが無く、値を省いた `{ x: }` も対象外。
        expect_no_offenses(COP, "h = {:a=>1}\n");
        CopCase::new("Layout/SpaceAfterColon", "x = 1\nh = {x:}\n", Vec::new())
            .target_ruby("3.1")
            .run();
    }

    #[test]
    fn space_after_method_name() {
        const COP: &str = "Layout/SpaceAfterMethodName";
        CopCase::annotated(
            COP,
            r#"
            def g (x); end
                 ^ Do not put a space between a method name and the opening parenthesis.
            "#,
        )
        .corrected("def g(x); end\n")
        .run();
        expect_no_offenses(COP, "def g(x); end\n");
        // 括弧の無い引数リストは対象外。
        expect_no_offenses(COP, "def g x; end\n");
    }

    #[test]
    fn space_before_comma() {
        const COP: &str = "Layout/SpaceBeforeComma";
        CopCase::annotated(
            COP,
            r#"
            h = [1 , 2]
                  ^ Space found before comma.
            "#,
        )
        .corrected("h = [1, 2]\n")
        .run();
        expect_no_offenses(COP, "h = [1, 2]\n");
        // 行頭のカンマは直前のトークンが別行なので対象外。
        expect_no_offenses(COP, "h = [1\n, 2]\n");
        expect_no_offenses(COP, "x = \"a , b\"\n");
    }

    #[test]
    fn space_after_semicolon() {
        const COP: &str = "Layout/SpaceAfterSemicolon";
        CopCase::annotated(
            COP,
            r#"
            k = 1;l = 2
                 ^ Space missing after semicolon.
            "#,
        )
        .corrected("k = 1; l = 2\n")
        .run();
        expect_no_offenses(COP, "k = 1; l = 2\n");
        // 連続したセミコロン、閉じ括弧、補間の終わりは空白を要らない。
        expect_no_offenses(COP, "k = 1;;\n");
        expect_no_offenses(COP, "x = (1;)\n");
        expect_no_offenses(COP, "x = \"#{1;}\"\n");
        expect_no_offenses(COP, "k = 1;\nl = 2\n");
    }

    #[test]
    fn leading_comment_space() {
        const COP: &str = "Layout/LeadingCommentSpace";
        CopCase::annotated(
            COP,
            r#"
            #comment
            ^^^^^^^^ Missing space after `#`.
            "#,
        )
        .corrected("# comment\n")
        .run();
        expect_no_offenses(COP, "# comment\n");
        // 罫線・`#=`・`#++` は対象外で、1 行目の shebang も許される。
        expect_no_offenses(COP, "####\n");
        expect_no_offenses(COP, "#=begin\n");
        expect_no_offenses(COP, "#++\n");
        expect_no_offenses(COP, "#!/usr/bin/env ruby\nx = 1\n");
        // ヒアドキュメント本文の `#` は文法上コメントに見えるがコメントではない。
        expect_no_offenses(COP, "x = <<~MSG\n  a #{1}#b\nMSG\n");
    }

    #[test]
    fn leading_empty_lines() {
        const COP: &str = "Layout/LeadingEmptyLines";
        CopCase::new(
            COP,
            "\n\nx = 1\n",
            vec![Annotation::new(
                3,
                1,
                1,
                "Unnecessary blank line at the beginning of the source.",
            )],
        )
        .corrected("x = 1\n")
        .run();
        expect_no_offenses(COP, "x = 1\n");
        // 先頭のコメントもトークンなので、その前の空行が対象になる。
        CopCase::new(
            COP,
            "\n# c\nx = 1\n",
            vec![Annotation::new(
                2,
                1,
                3,
                "Unnecessary blank line at the beginning of the source.",
            )],
        )
        .corrected("# c\nx = 1\n")
        .run();
    }
}

/// 代入の右辺の字下げと条件の位置。期待値は本家 1.89.0 の
/// `--only <cop> --format json` と `-A` の実測。
mod layout_assignment_and_condition {
    use super::*;

    /// 右辺が自分の行から始まるときだけ見る。基準は代入の左端 + インデント幅。
    #[test]
    fn assignment_indentation() {
        const COP: &str = "Layout/AssignmentIndentation";
        CopCase::new(
            COP,
            "value =\nif foo\n  1\nend\n",
            vec![Annotation::new(
                2,
                1,
                6,
                "Indent the first line of the right-hand-side of a multi-line assignment.",
            )],
        )
        .locations(&[(2, 1, 4, 3)])
        .lengths(&[14])
        .corrected("value =\n  if foo\n    1\n  end\n")
        .run();
        expect_no_offenses(COP, "value =\n  if foo\n    1\n  end\n");
        // 右辺が演算子と同じ行にあるものは対象外。
        expect_no_offenses(COP, "value = if foo\n  1\nend\n");
    }

    /// キーワードと違う行から始まる条件だけを見る。行をまたぐだけの条件は対象外。
    #[test]
    fn condition_position() {
        const COP: &str = "Layout/ConditionPosition";
        CopCase::new(
            COP,
            "if\n  x\n  puts 1\nend\n",
            vec![Annotation::new(
                2,
                3,
                1,
                "Place the condition on the same line as `if`.",
            )],
        )
        .severity(Severity::Warning)
        .corrected("if x\n  puts 1\nend\n")
        .run();
        CopCase::new(
            "Layout/ConditionPosition",
            "if a &&\n   b\n  puts 1\nend\n",
            Vec::new(),
        )
        .run();
        expect_no_offenses(COP, "puts 1 if x\n");
        expect_no_offenses(COP, "x ? 1 : 2\n");
    }
}

/// `Proc.new` / クラス変数 / 真偽値の既定引数 / stabby lambda / `.()` / 否定 `if` /
/// クォート付きシンボル / `when x;` / `kind_of?` / `$stderr.puts` の回帰。
mod style_conventions {
    use super::*;

    #[test]
    fn proc_new_needs_a_block_and_a_top_level_receiver() {
        expect_offense(
            "Style/Proc",
            r#"
            p = Proc.new { |n| n }
                ^^^^^^^^ Use `proc` instead of `Proc.new`.
            "#,
        );
        expect_correction(
            "Style/Proc",
            "q = ::Proc.new do |n| n end\n",
            "q = proc do |n| n end\n",
        );
        // ブロックの無い `Proc.new` は proc リテラルではない。
        expect_no_offenses("Style/Proc", "r = Proc.new\n");
        expect_no_offenses("Style/Proc", "s = Proc.new(1) { |n| n }\n");
        expect_no_offenses("Style/Proc", "t = Foo::Proc.new { |n| n }\n");
    }

    #[test]
    fn class_vars_reports_assignment_and_the_reflective_setter() {
        expect_offense(
            "Style/ClassVars",
            r#"
            @@test = 10
            ^^^^^^ Replace class var @@test with a class instance var.
            "#,
        );
        expect_offense(
            "Style/ClassVars",
            r#"
            class_variable_set(:@@test, 10)
                               ^^^^^^^ Replace class var :@@test with a class instance var.
            "#,
        );
        expect_offense(
            "Style/ClassVars",
            r#"
            begin
              x
            rescue => @@error
                      ^^^^^^^ Replace class var @@error with a class instance var.
              y
            end
            "#,
        );
        // 読み出しは対象外。
        expect_no_offenses("Style/ClassVars", "def read\n  @@test\nend\n");
        expect_no_offenses("Style/ClassVars", "class_variable_get(:@@test)\n");
    }

    /// 文法は `a = nil, b = false` を 1 個の多重代入と読むので、上流の `optarg` 2 個へ
    /// 戻してから既定値を見る必要がある。
    #[test]
    fn optional_boolean_parameter_splits_a_misread_default_run() {
        expect_offense(
            "Style/OptionalBooleanParameter",
            r#"
            def tag(name = nil, open = false, escape = true)
                                ^^^^^^^^^^^^ Prefer keyword arguments for arguments with a boolean default value; use `open: false` instead of `open = false`.
                                              ^^^^^^^^^^^^^ Prefer keyword arguments for arguments with a boolean default value; use `escape: true` instead of `escape = true`.
              name
            end
            "#,
        );
        expect_no_offenses(
            "Style/OptionalBooleanParameter",
            "def respond_to_missing?(name, include_private = false)\n  name\nend\n",
        );
        expect_no_offenses(
            "Style/OptionalBooleanParameter",
            "def m(bar: false)\n  bar\nend\n",
        );
    }

    #[test]
    fn stabby_lambda_parentheses_wraps_bare_arguments() {
        expect_offense(
            "Style/StabbyLambdaParentheses",
            r#"
            f = ->a, b { a + b }
                  ^^^^ Wrap stabby lambda arguments with parentheses.
            "#,
        );
        expect_correction(
            "Style/StabbyLambdaParentheses",
            "f = ->a, b { a + b }\n",
            "f = ->(a, b) { a + b }\n",
        );
        expect_no_offenses("Style/StabbyLambdaParentheses", "g = ->(a) { a }\n");
        expect_no_offenses("Style/StabbyLambdaParentheses", "h = -> { 1 }\n");
        expect_no_offenses("Style/StabbyLambdaParentheses", "i = ->() { 1 }\n");
    }

    #[test]
    fn lambda_call_prefers_the_written_selector() {
        expect_offense(
            "Style/LambdaCall",
            r#"
            h = f.(1, 2)
                ^^^^^^^^ Prefer the use of `f.call(1, 2)` over `f.(1, 2)`.
            "#,
        );
        expect_correction("Style/LambdaCall", "h = f.(1, 2)\n", "h = f.call(1, 2)\n");
        expect_no_offenses("Style/LambdaCall", "i = f.call(1, 2)\n");
        // 引数リストの中のコメントは書き換えで消えるので手当てしない。
        expect_no_offenses("Style/LambdaCall", "j = f.( # why\n  1\n)\n");
    }

    #[test]
    fn negated_if_covers_both_forms_but_not_an_else() {
        expect_offense(
            "Style/NegatedIf",
            r#"
            z if !w
            ^^^^^^^ Favor `unless` over `if` for negative conditions.
            "#,
        );
        expect_correction("Style/NegatedIf", "z if !w\n", "z unless w\n");
        expect_correction(
            "Style/NegatedIf",
            "if !x\n  y\nend\n",
            "unless x\n  y\nend\n",
        );
        expect_correction(
            "Style/NegatedIf",
            "if (!a)\n  b\nend\n",
            "unless (a)\n  b\nend\n",
        );
        expect_no_offenses("Style/NegatedIf", "if !s\n  t\nelse\n  u\nend\n");
        expect_no_offenses("Style/NegatedIf", "unless !v\n  u\nend\n");
        expect_no_offenses("Style/NegatedIf", "if !!s\n  t\nend\n");
    }

    #[test]
    fn symbol_literal_drops_quotes_only_from_word_like_names() {
        expect_offense(
            "Style/SymbolLiteral",
            r##"
            :"foo"
            ^^^^^^ Do not use strings for word-like symbol literals.
            "##,
        );
        expect_correction("Style/SymbolLiteral", ":'bar'\n", ":bar\n");
        expect_no_offenses("Style/SymbolLiteral", ":\"foo bar\"\n");
        expect_no_offenses("Style/SymbolLiteral", ":\"1foo\"\n");
        expect_no_offenses("Style/SymbolLiteral", ":foo\n");
    }

    #[test]
    fn when_then_replaces_a_semicolon_on_a_single_line() {
        expect_offense(
            "Style/WhenThen",
            r#"
            case n
            when 1, 2; puts 1
                     ^ Do not use `when 1, 2;`. Use `when 1, 2 then` instead.
            end
            "#,
        );
        expect_correction(
            "Style/WhenThen",
            "case n\nwhen 1; puts 1\nend\n",
            "case n\nwhen 1 then puts 1\nend\n",
        );
        expect_no_offenses("Style/WhenThen", "case n\nwhen 2 then puts 2\nend\n");
        expect_no_offenses("Style/WhenThen", "case n\nwhen 1;\n  puts 1\nend\n");
    }

    #[test]
    fn class_check_renames_the_selector_only() {
        expect_offense(
            "Style/ClassCheck",
            r#"
            n.kind_of?(Integer)
              ^^^^^^^^ Prefer `Object#is_a?` over `Object#kind_of?`.
            "#,
        );
        expect_correction(
            "Style/ClassCheck",
            "n&.kind_of?(Integer)\n",
            "n&.is_a?(Integer)\n",
        );
        expect_no_offenses("Style/ClassCheck", "n.is_a?(Integer)\n");
    }

    #[test]
    fn stderr_puts_needs_a_stream_receiver_and_an_argument() {
        expect_offense(
            "Style/StderrPuts",
            r#"
            $stderr.puts "oops"
            ^^^^^^^^^^^^ Use `warn` instead of `$stderr.puts` to allow such output to be disabled.
            "#,
        );
        expect_correction(
            "Style/StderrPuts",
            "STDERR.puts(\"bad\")\n",
            "warn(\"bad\")\n",
        );
        expect_no_offenses("Style/StderrPuts", "$stderr.puts\n");
        expect_no_offenses("Style/StderrPuts", "$stdout.puts 'x'\n");
    }
}

/// 書式文字列 / `%` リテラル / `raise` の引数 / 長さ 0 判定 / `method_missing` の回帰。
mod style_formatting {
    use super::*;

    #[test]
    fn format_string_covers_every_spelling_of_the_call() {
        expect_offense(
            "Style/FormatString",
            r#"
            puts sprintf('%10s', 'foo')
                 ^^^^^^^ Favor `format` over `sprintf`.
            "#,
        );
        expect_offense(
            "Style/FormatString",
            r#"
            puts '%10s' % 'foo'
                        ^ Favor `format` over `String#%`.
            "#,
        );
        expect_correction(
            "Style/FormatString",
            "puts '%10s' % 'foo'\n",
            "puts format('%10s', 'foo')\n",
        );
        expect_correction(
            "Style/FormatString",
            "puts '%s' % [1, 2]\n",
            "puts format('%s', 1, 2)\n",
        );
        // 文法が `\"%s\"%[a, b]` を 2 つ目の `%` リテラルとして読むので、上流の
        // `(send (str) :% (array ...))` へ戻してから報告する。
        expect_correction(
            "Style/FormatString",
            "puts '%s'%[a, b]\n",
            "puts format('%s', a, b)\n",
        );
        // 引数が配列かもしれない変数なら、たたみ込むと出力が変わるので補正しない。
        CopCase::annotated(
            "Style/FormatString",
            r#"
            puts '%s' % x
                      ^ Favor `format` over `String#%`.
            "#,
        )
        .correctable(false)
        .run();
        expect_no_offenses("Style/FormatString", "puts format('%10s', 'foo')\n");
        expect_no_offenses("Style/FormatString", "puts foo % bar\n");
    }

    #[test]
    fn percent_literal_cops_agree_on_the_opening_delimiter() {
        expect_offense(
            "Style/BarePercentLiterals",
            r#"
            a = %Q(hi)
                ^^^ Use `%` instead of `%Q`.
            "#,
        );
        expect_correction("Style/BarePercentLiterals", "a = %Q(hi)\n", "a = %(hi)\n");
        expect_no_offenses("Style/BarePercentLiterals", "a = %(hi)\n");
        expect_correction("Style/PercentQLiterals", "b = %Q(hi)\n", "b = %q(hi)\n");
        // `%q` と `%Q` で意味が変わる本文は残す。
        expect_no_offenses("Style/PercentQLiterals", "b = %Q(a\\nb)\n");
        expect_correction("Style/RedundantCapitalW", "d = %W[a b]\n", "d = %w[a b]\n");
        expect_no_offenses("Style/RedundantCapitalW", "e = %W[a #\u{7b}b}]\n");
        expect_correction("Style/RedundantPercentQ", "c = %q(hi)\n", "c = 'hi'\n");
        expect_correction(
            "Style/RedundantPercentQ",
            "f = %q(don't)\n",
            "f = \"don't\"\n",
        );
        expect_no_offenses("Style/RedundantPercentQ", "g = %q(it's \"here\")\n");
    }

    #[test]
    fn raise_args_explodes_a_constructed_exception() {
        expect_offense(
            "Style/RaiseArgs",
            r#"
            raise RuntimeError.new('msg')
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Provide an exception class and message as arguments to `raise`.
            "#,
        );
        expect_correction(
            "Style/RaiseArgs",
            "raise RuntimeError.new('msg')\n",
            "raise RuntimeError, 'msg'\n",
        );
        // 引数の無い `new` も潰せる。
        expect_correction(
            "Style/RaiseArgs",
            "raise StandardError.new\n",
            "raise StandardError\n",
        );
        // `operator_keyword?` は `||` も含むので括弧が要る。
        expect_correction(
            "Style/RaiseArgs",
            "x || raise(KeyError.new('k'))\n",
            "x || raise(KeyError, 'k')\n",
        );
        expect_no_offenses("Style/RaiseArgs", "raise RuntimeError, 'msg'\n");
        expect_no_offenses("Style/RaiseArgs", "raise Foo.new(bar: 1)\n");
    }

    #[test]
    fn zero_length_predicate_reads_both_the_predicate_and_the_comparisons() {
        expect_offense(
            "Style/ZeroLengthPredicate",
            r#"
            x = [].size.zero?
                   ^^^^^^^^^^ Use `empty?` instead of `size.zero?`.
            "#,
        );
        expect_correction(
            "Style/ZeroLengthPredicate",
            "x = [].size.zero?\n",
            "x = [].empty?\n",
        );
        expect_correction(
            "Style/ZeroLengthPredicate",
            "y = a.length == 0\n",
            "y = a.empty?\n",
        );
        expect_correction(
            "Style/ZeroLengthPredicate",
            "z = a.size > 0\n",
            "z = !a.empty?\n",
        );
        expect_correction(
            "Style/ZeroLengthPredicate",
            "w = 0 == a.size\n",
            "w = a.empty?\n",
        );
        expect_correction(
            "Style/ZeroLengthPredicate",
            "v = a.size < 1\n",
            "v = a.empty?\n",
        );
        // ファイルの大きさは要素数ではない。
        expect_no_offenses(
            "Style/ZeroLengthPredicate",
            "u = File.stat('f').size == 0\n",
        );
        expect_no_offenses("Style/ZeroLengthPredicate", "t = a.size == 1\n");
    }

    #[test]
    fn missing_respond_to_missing_looks_in_the_same_scope() {
        expect_offense(
            "Style/MissingRespondToMissing",
            r#"
            class Q
              def method_missing(name)
              ^^^^^^^^^^^^^^^^^^^^^^^^ When using `method_missing`, define `respond_to_missing?`.
                nil
              end
            end
            "#,
        );
        expect_no_offenses(
            "Style/MissingRespondToMissing",
            "class R\n  def method_missing(name)\n    nil\n  end\n\n  def respond_to_missing?(name, p = false)\n    true\n  end\nend\n",
        );
    }
}

/// `Array#*` / 文字リテラル / `RuntimeError` の冗長指定 / 短い補間の回帰。
mod style_literals_and_calls {
    use super::*;

    #[test]
    fn array_join_needs_an_array_literal_and_a_string() {
        expect_offense(
            "Style/ArrayJoin",
            r#"
            a = [1, 2] * ', '
                       ^ Favor `Array#join` over `Array#*`.
            "#,
        );
        expect_correction(
            "Style/ArrayJoin",
            "a = [1, 2] * ', '\n",
            "a = [1, 2].join(', ')\n",
        );
        expect_correction(
            "Style/ArrayJoin",
            "b = %w[x y] * '-'\n",
            "b = %w[x y].join('-')\n",
        );
        expect_no_offenses("Style/ArrayJoin", "c = [1, 2] * 3\n");
        expect_no_offenses("Style/ArrayJoin", "d = x * ', '\n");
    }

    #[test]
    fn character_literal_picks_the_quote_from_what_it_holds() {
        expect_offense(
            "Style/CharacterLiteral",
            r#"
            c = ?a
                ^^ Do not use the character literal - use string literal instead.
            "#,
        );
        expect_correction("Style/CharacterLiteral", "c = ?a\n", "c = 'a'\n");
        expect_correction("Style/CharacterLiteral", "d = ?\\n\n", "d = \"\\n\"\n");
        expect_no_offenses("Style/CharacterLiteral", "e = 'a'\n");
    }

    #[test]
    fn redundant_exception_covers_both_spellings() {
        expect_offense(
            "Style/RedundantException",
            r#"
            raise RuntimeError, 'msg'
            ^^^^^^^^^^^^^^^^^^^^^^^^^ Redundant `RuntimeError` argument can be removed.
            "#,
        );
        expect_correction(
            "Style/RedundantException",
            "raise RuntimeError, 'msg'\n",
            "raise 'msg'\n",
        );
        expect_correction(
            "Style/RedundantException",
            "raise RuntimeError.new('msg')\n",
            "raise 'msg'\n",
        );
        // 文字列以外は `to_s` を挟む。括弧付きの呼び出しは括弧のまま。
        expect_correction(
            "Style/RedundantException",
            "fail RuntimeError, msg\n",
            "fail msg.to_s\n",
        );
        expect_correction(
            "Style/RedundantException",
            "raise(RuntimeError, 'msg')\n",
            "raise('msg')\n",
        );
        expect_no_offenses("Style/RedundantException", "raise ArgumentError, 'ok'\n");
    }

    /// 上流の parser が変数そのものを dstr の子に置くのは短い綴りだけで、`#{...}` は
    /// `begin` に包まれるため対象外。
    #[test]
    fn variable_interpolation_only_sees_the_short_spelling() {
        expect_offense(
            "Style/VariableInterpolation",
            r##"
            e = "#@foo"
                  ^^^^ Replace interpolated variable `@foo` with expression `#{@foo}`.
            "##,
        );
        expect_correction(
            "Style/VariableInterpolation",
            "f = \"#$bar\"\n",
            "f = \"#{$bar}\"\n",
        );
        expect_no_offenses("Style/VariableInterpolation", "g = \"#\u{7b}@baz}\"\n");
    }
}

/// Lint 部門の後発 cop。期待値は本家 1.89.0 の `--only <cop> --format json` の実測から。
mod lint_late_additions {
    use super::*;

    #[test]
    fn erb_new_arguments_rewrites_every_legacy_position_at_once() {
        CopCase::annotated(
            "Lint/ErbNewArguments",
            r#"
            ERB.new(str, nil, '-', '@output')
                         ^^^ Passing safe_level with the 2nd argument of `ERB.new` is deprecated. Do not use it, and specify other arguments as keyword arguments.
                              ^^^ Passing trim_mode with the 3rd argument of `ERB.new` is deprecated. Use keyword argument like `ERB.new(str, trim_mode: '-')` instead.
                                   ^^^^^^^^^ Passing eoutvar with the 4th argument of `ERB.new` is deprecated. Use keyword argument like `ERB.new(str, eoutvar: '@output')` instead.
            "#,
        )
        .corrected("ERB.new(str, trim_mode: '-', eoutvar: '@output')\n")
        .run();
    }

    #[test]
    fn erb_new_arguments_accepts_the_keyword_form() {
        expect_no_offenses("Lint/ErbNewArguments", "ERB.new(str, trim_mode: '-')\n");
    }

    /// `Dir.glob` sorts from Ruby 3.0 on, so the cop is off above its maximum target version.
    #[test]
    fn non_deterministic_require_order_is_gated_on_the_target_version() {
        CopCase::new(
            "Lint/NonDeterministicRequireOrder",
            "Dir.glob('./lib/*.rb').each do |file|\n  require file\nend\n".to_owned(),
            Vec::new(),
        )
        .target_ruby("3.0")
        .run();
    }

    #[test]
    fn non_deterministic_require_order_sorts_a_block_pass() {
        CopCase::annotated(
            "Lint/NonDeterministicRequireOrder",
            r#"
            Dir.glob('./lib/*.rb', &method(:require))
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Sort files before requiring them.
            "#,
        )
        .target_ruby("2.7")
        .corrected("Dir.glob('./lib/*.rb').sort.each(&method(:require))\n")
        .run();
    }

    #[test]
    fn safe_navigation_chain_accepts_a_chain_of_safe_calls() {
        expect_no_offenses("Lint/SafeNavigationChain", "foo&.bar&.baz\n");
    }

    /// `nil` answers `to_s`, so a call the receiver cannot fail is no chain to report.
    #[test]
    fn safe_navigation_chain_accepts_a_method_nil_responds_to() {
        expect_no_offenses("Lint/SafeNavigationChain", "foo&.bar.to_s\n");
    }

    #[test]
    fn safe_navigation_consistency_asks_for_the_operator_the_group_already_uses() {
        CopCase::annotated(
            "Lint/SafeNavigationConsistency",
            r#"
            foo&.bar || foo.baz
                           ^ Use `&.` for consistency with safe navigation.
            "#,
        )
        .run();
    }

    #[test]
    fn redundant_safe_navigation_reports_a_class_name_receiver() {
        CopCase::annotated(
            "Lint/RedundantSafeNavigation",
            r#"
            Foo&.bar
               ^^ Redundant safe navigation detected, use `.` instead.
            "#,
        )
        .run();
    }

    #[test]
    fn redundant_safe_navigation_accepts_an_ordinary_receiver() {
        expect_no_offenses("Lint/RedundantSafeNavigation", "foo&.bar\n");
    }

    #[test]
    fn redundant_splat_expansion_keeps_a_percent_literal_argument() {
        expect_no_offenses("Lint/RedundantSplatExpansion", "foo(*%w[a b])\n");
    }

    /// The four shapes the correction takes: drop the brackets, drop the star, wrap a scalar, and
    /// widen to the whole literal for an `Array.new`.
    #[test]
    fn redundant_splat_expansion_corrects_each_position() {
        expect_correction(
            "Lint/RedundantSplatExpansion",
            "foo(*[1, 2])\n",
            "foo(1, 2)\n",
        );
        expect_correction(
            "Lint/RedundantSplatExpansion",
            "x = *[1, 2]\n",
            "x = [1, 2]\n",
        );
        expect_correction("Lint/RedundantSplatExpansion", "x = *'a'\n", "x = ['a']\n");
        expect_correction(
            "Lint/RedundantSplatExpansion",
            "return *[1, 2]\n",
            "return [1, 2]\n",
        );
        expect_correction(
            "Lint/RedundantSplatExpansion",
            "case x\nwhen *[1, 2] then y\nend\n",
            "case x\nwhen 1, 2 then y\nend\n",
        );
        expect_correction(
            "Lint/RedundantSplatExpansion",
            "[*Array.new(3)]\n",
            "Array.new(3)\n",
        );
    }

    #[test]
    fn redundant_splat_expansion_accepts_an_empty_array() {
        expect_no_offenses("Lint/RedundantSplatExpansion", "foo(*[])\n");
    }

    #[test]
    fn shadowed_exception_reports_a_group_of_two_levels() {
        CopCase::annotated(
            "Lint/ShadowedException",
            r#"
            begin
              do_something
            rescue StandardError, RuntimeError
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Do not shadow rescued Exceptions.
              handle
            end
            "#,
        )
        .locations(&[(3, 1, 4, 8)])
        .run();
    }

    #[test]
    fn shadowed_exception_reports_clauses_written_out_of_order() {
        CopCase::annotated(
            "Lint/ShadowedException",
            r#"
            begin
              x
            rescue StandardError
            ^^^^^^^^^^^^^^^^^^^^ Do not shadow rescued Exceptions.
              y
            rescue ArgumentError
              z
            end
            "#,
        )
        .locations(&[(3, 1, 4, 3)])
        .run();
    }

    /// Two `Errno` classes are unrelated however they are ordered, so neither shadows the other.
    #[test]
    fn shadowed_exception_accepts_two_errno_classes() {
        expect_no_offenses(
            "Lint/ShadowedException",
            "begin\n  x\nrescue Errno::ENOENT, Errno::EACCES\n  y\nend\n",
        );
    }

    /// A name no constant answers to compares to nothing, which is what keeps an application's own
    /// exception classes from being read as a hierarchy.
    #[test]
    fn shadowed_exception_accepts_unresolvable_names() {
        expect_no_offenses(
            "Lint/ShadowedException",
            "begin\n  x\nrescue MyError\n  y\nrescue OtherError\n  z\nend\n",
        );
    }

    #[test]
    fn useless_setter_call_follows_the_variable_the_object_was_copied_into() {
        CopCase::annotated(
            "Lint/UselessSetterCall",
            r#"
            def foo
              x = Object.new
              y = x
              y.attr = 1
              ^ Useless setter call to local variable `y`.
            end
            "#,
        )
        .corrected("def foo\n  x = Object.new\n  y = x\n  y.attr = 1\n  y\nend\n")
        .run();
    }

    #[test]
    fn useless_setter_call_accepts_an_object_that_came_from_outside() {
        expect_no_offenses(
            "Lint/UselessSetterCall",
            "def foo(bar)\n  x = bar\n  x.attr = 1\nend\n",
        );
    }

    /// The permission is read off the file the source came from, so a source with no file behind
    /// it is left alone -- which is what upstream's `File.exist?` guard does.
    #[test]
    fn script_permission_reads_the_mode_of_the_file_on_disk() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("script.rb");
        let source = "#!/usr/bin/env ruby\nputs 1\n";
        std::fs::write(&path, source).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let case = CopCase::new("Lint/ScriptPermission", source.to_owned(), Vec::new())
            .path(path.to_str().unwrap());
        let report = case.inspect();
        assert_eq!(report.offenses.len(), 1);
        assert_eq!(
            report.offenses[0].message,
            "Script file script.rb doesn't have execute permission."
        );
        assert!(!report.offenses[0].is_correctable());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(case.inspect().offenses.is_empty());
    }
}

/// `Lint/RedundantCopDisableDirective`.
///
/// 本家はこの cop を `--only` と併用できないので、ケースは `--except` 側で選ぶ
/// ([`CopCase::without_only`])。期待値は本家 1.89.0 を
/// `--except <この cop 以外の全 cop>` で走らせた実測から取っている。
mod redundant_cop_disable_directive {
    use super::*;

    fn case(annotated: &str) -> CopCase {
        CopCase::annotated("Lint/RedundantCopDisableDirective", annotated).without_only()
    }

    /// 何も報告していない cop を無効化した block 形式は、コメントごと消える。
    /// ファイル先頭のコメントだけは末尾の改行も食う。
    #[test]
    fn a_leading_block_directive_takes_its_newline_with_it() {
        case(
            r#"
            # rubocop:disable Layout/LineLength
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of `Layout/LineLength`.
            x = 1
            # rubocop:enable Layout/LineLength
            "#,
        )
        .corrected("x = 1\n# rubocop:enable Layout/LineLength\n")
        .run();
    }

    /// 前の行が空でなければ、コメントは手前の改行ごと消える。
    #[test]
    fn a_block_directive_after_code_takes_the_preceding_newline() {
        case(
            r#"
            y = 0
            # rubocop:disable Layout/LineLength
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of `Layout/LineLength`.
            x = 1
            # rubocop:enable Layout/LineLength
            "#,
        )
        .corrected("y = 0\nx = 1\n# rubocop:enable Layout/LineLength\n")
        .run();
    }

    /// 前の行が空なら空行は残す。
    #[test]
    fn a_blank_line_before_the_directive_is_kept() {
        case(
            r#"
            y = 0

            # rubocop:disable Layout/LineLength
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of `Layout/LineLength`.
            x = 1
            # rubocop:enable Layout/LineLength
            "#,
        )
        .corrected("y = 0\n\nx = 1\n# rubocop:enable Layout/LineLength\n")
        .run();
    }

    /// 行末ディレクティブはその行の 1 行分だけを覆う。レンジはコメント本体ではなく
    /// ディレクティブがマッチした範囲。
    #[test]
    fn a_trailing_directive_reports_the_matched_range_only() {
        case(
            r#"
            y = 0
            x = 1 # rubocop:disable Layout/LineLength
                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of `Layout/LineLength`.
            "#,
        )
        .corrected("y = 0\nx = 1\n")
        .run();
    }

    /// ディレクティブの後ろに自由記述が残るなら、消すのではなく ` # ` に置き換える。
    /// 本家はこのとき手前の改行まで食うので、記述は前の行の末尾へ回る。
    #[test]
    fn a_free_comment_after_the_directive_is_left_behind() {
        case(
            r#"
            y = 0
            # rubocop:disable Layout/LineLength -- keep
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of `Layout/LineLength`.
            x = 1
            # rubocop:enable Layout/LineLength
            "#,
        )
        .corrected("y = 0 # -- keep\nx = 1\n# rubocop:enable Layout/LineLength\n")
        .run();
    }

    /// `disable all` は 1 件にまとまり、文言は `all cops`。
    #[test]
    fn disabling_everything_reports_one_offense_for_all_cops() {
        case(
            r#"
            y = 0
            # rubocop:disable all
            ^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of all cops.
            x = 1
            # rubocop:enable all
            "#,
        )
        .corrected("y = 0\nx = 1\n# rubocop:enable all\n")
        .run();
    }

    /// 列挙した cop が全部不要なら、コメント全体で 1 件。部門指定は `department` と読む。
    #[test]
    fn a_wholly_redundant_list_reports_the_comment_once() {
        case(r#"
            y = 0
            # rubocop:disable Layout, Style/StringLiterals
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of `Layout` department, `Style/StringLiterals`.
            x = 1
            # rubocop:enable Layout, Style/StringLiterals
            "#)
        .corrected("y = 0\nx = 1\n# rubocop:enable Layout, Style/StringLiterals\n")
        .run();
    }

    /// 知らない cop 名には綴りの近いものを添える。無ければ `(unknown cop)`。
    #[test]
    fn an_unknown_cop_name_gets_a_suggestion() {
        case(r#"
            # rubocop:disable Lint/Foo
            ^^^^^^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of `Lint/Foo` (did you mean `Lint/Loop`?).
            x = 1
            # rubocop:enable Lint/Foo
            "#)
        .corrected("x = 1\n# rubocop:enable Lint/Foo\n")
        .run();
    }

    /// 設定で無効な cop をファイル末尾まで無効化し直すのは `expected_final_disable?`
    /// で見送られる…が、`inject_disabled_cops_directives` が入れる `-Infinity` 始まりの
    /// レンジと連続するため、`each_already_disabled` 側が拾う。
    #[test]
    fn re_disabling_a_configuration_disabled_cop_is_still_reported() {
        case(
            r#"
            # rubocop:disable Style/Copyright
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of `Style/Copyright`.
            x = 1
            "#,
        )
        .corrected("x = 1\n")
        .run();
    }

    /// 実際に offense を抑止しているディレクティブは残す。
    #[test]
    fn a_directive_that_suppressed_something_is_left_alone() {
        CopCase::new(
            "Lint/RedundantCopDisableDirective",
            "x = \"a\" # rubocop:disable Style/StringLiterals\n",
            Vec::new(),
        )
        .cops(&["Lint/RedundantCopDisableDirective", "Style/StringLiterals"])
        .cop_names(&[])
        .without_only()
        .run();
    }

    /// 列挙のうち 1 件だけが不要なら、その cop 名とカンマだけを消す。
    #[test]
    fn only_the_redundant_entry_of_a_list_is_removed() {
        CopCase::new(
            "Lint/RedundantCopDisableDirective",
            "x = \"a\" # rubocop:disable Layout/LineLength, Style/StringLiterals\n",
            vec![Annotation::new(
                1,
                27,
                17,
                "Unnecessary disabling of `Layout/LineLength`.",
            )],
        )
        .cops(&[
            "Lint/RedundantCopDisableDirective",
            "Style/StringLiterals",
            "Layout/LineLength",
        ])
        .cop_names(&["Lint/RedundantCopDisableDirective"])
        .without_only()
        .correct_mode(sonicop::engine::CorrectMode::None)
        .run();
    }

    /// 行末に来る不要な cop は左のカンマごと消える。
    #[test]
    fn a_redundant_entry_at_the_end_of_a_list_eats_the_comma_on_its_left() {
        CopCase::new(
            "Lint/RedundantCopDisableDirective",
            "x = \"a\" # rubocop:disable Style/StringLiterals, Layout/LineLength\n",
            Vec::new(),
        )
        .cops(&[
            "Lint/RedundantCopDisableDirective",
            "Style/StringLiterals",
            "Layout/LineLength",
        ])
        .without_offense_check()
        .without_only()
        .corrected("x = \"a\" # rubocop:disable Style/StringLiterals\n")
        .run();
    }

    /// `--only` を渡した実行ではこの cop 自体を走らせない。
    #[test]
    fn the_cop_does_not_run_under_only() {
        expect_no_offenses(
            "Lint/RedundantCopDisableDirective",
            "# rubocop:disable Layout/LineLength\nx = 1\n# rubocop:enable Layout/LineLength\n",
        );
    }
}

/// 空白・整列まわりの Layout cop。期待値は本家 1.89.0 の
/// `--only <cop> --format json` と `-A` の実測。
mod layout_spacing_and_alignment {
    use super::*;

    /// `.` と `&.` と `::` の周りの空白。`::` はメソッド呼び出しでは見ない。
    #[test]
    fn space_around_method_call_operator() {
        const COP: &str = "Layout/SpaceAroundMethodCallOperator";
        expect_offense(
            COP,
            r#"
            foo. bar
                ^ Avoid using spaces around a method call operator.
            "#,
        );
        expect_correction(COP, "foo. bar\n", "foo.bar\n");
        expect_correction(COP, "foo &.bar\n", "foo&.bar\n");
        expect_correction(COP, "RuboCop:: Cop\n", "RuboCop::Cop\n");
        // 代入先の定数パスは本家では `casgn` なので `on_const` が呼ばれない。
        expect_no_offenses(COP, "Foo:: Bar = 1\n");
        // 行をまたぐチェーンは空白ではない。
        expect_no_offenses(COP, "foo\n  .bar\n");
        // `::` を使ったメソッド呼び出しは `dot?` でも `safe_navigation?` でもない。
        expect_no_offenses(COP, "foo:: bar\n");
    }

    /// 添字の括弧の内側。1 ノードにつき corrector は 1 回しか回らない。
    #[test]
    fn space_inside_reference_brackets() {
        const COP: &str = "Layout/SpaceInsideReferenceBrackets";
        CopCase::new(
            COP,
            "a[ :k ]\n",
            vec![
                Annotation::new(1, 3, 1, "Do not use space inside reference brackets."),
                Annotation::new(1, 6, 1, "Do not use space inside reference brackets."),
            ],
        )
        .corrected("a[:k]\n")
        .run();
        expect_no_offenses(COP, "b[]\n");
        expect_no_offenses(COP, "a[:k]\n");
        // 複数行の添字は空でない限り対象外。
        expect_no_offenses(COP, "a[\n  :k\n]\n");
    }

    /// `{` の左の空白。`do` ブロックは対象外。
    #[test]
    fn space_before_block_braces() {
        const COP: &str = "Layout/SpaceBeforeBlockBraces";
        expect_offense(
            COP,
            r#"
            7.times{}
                   ^ Space missing to the left of {.
            "#,
        );
        expect_correction(COP, "7.times{}\n", "7.times {}\n");
        expect_correction(COP, "x = [1].map{ |a| a }\n", "x = [1].map { |a| a }\n");
        expect_no_offenses(COP, "7.times {}\n");
        expect_no_offenses(COP, "7.times do\nend\n");
    }

    /// 既定引数の `=` の周り。値は 3 番目のトークンから始まる。
    #[test]
    fn space_around_equals_in_parameter_default() {
        const COP: &str = "Layout/SpaceAroundEqualsInParameterDefault";
        expect_offense(
            COP,
            r#"
            def m(a=1)
                   ^ Surrounding space missing in default value assignment.
            end
            "#,
        );
        expect_correction(COP, "def m(a=1)\nend\n", "def m(a = 1)\nend\n");
        expect_no_offenses(COP, "def m(a = 1)\nend\n");
        // キーワード引数は `kwoptarg` なので対象外。
        expect_no_offenses(COP, "def m(a: 1)\nend\n");
    }

    /// パイプの内側と閉じパイプの後ろ。
    #[test]
    fn space_around_block_parameters() {
        const COP: &str = "Layout/SpaceAroundBlockParameters";
        CopCase::new(
            COP,
            "[1].each { | a | a }\n",
            vec![
                Annotation::new(1, 13, 1, "Space before first block parameter detected."),
                Annotation::new(1, 15, 1, "Space after last block parameter detected."),
            ],
        )
        .corrected("[1].each { |a| a }\n")
        .run();
        expect_offense(
            COP,
            r#"
            [2].each { |b|b }
                         ^ Space after closing `|` missing.
            "#,
        );
        expect_correction(COP, "[2].each { |b|b }\n", "[2].each { |b| b }\n");
        expect_no_offenses(COP, "[1].each { |a, b| a }\n");
    }

    /// 行末コメントの手前の空白。ヒアドキュメント本文中の `#` は数えない。
    #[test]
    fn space_before_comment() {
        const COP: &str = "Layout/SpaceBeforeComment";
        expect_offense(
            COP,
            r#"
            y = 1#comment
                 ^^^^^^^^ Put a space before an end-of-line comment.
            "#,
        );
        expect_correction(COP, "y = 1#comment\n", "y = 1 #comment\n");
        expect_no_offenses(COP, "y = 1 #comment\n");
        expect_no_offenses(COP, "# comment\n");
        expect_no_offenses(COP, "x = <<~T\n  a#b\nT\n");
    }

    /// ヒアドキュメントの終端の字下げ。`loc.heredoc_end` は行頭から始まる。
    #[test]
    fn closing_heredoc_indentation() {
        const COP: &str = "Layout/ClosingHeredocIndentation";
        CopCase::new(
            COP,
            "def foo\n  <<~SQL\n    Hi\n      SQL\nend\n",
            vec![Annotation::new(
                4,
                1,
                9,
                "`SQL` is not aligned with `<<~SQL`.",
            )],
        )
        .corrected("def foo\n  <<~SQL\n    Hi\n  SQL\nend\n")
        .run();
        expect_no_offenses(COP, "def foo\n  <<~SQL\n    Hi\n  SQL\nend\n");
        // `<<EOS` の終端は行頭に置くしかないので見ない。
        expect_no_offenses(COP, "def foo\n  <<SQL\n    Hi\nSQL\nend\n");
    }

    /// 引数の周りの空行。消えるのは直前の 1 行だけ。
    #[test]
    fn empty_lines_around_arguments() {
        const COP: &str = "Layout/EmptyLinesAroundArguments";
        CopCase::new(
            COP,
            "foo(a,\n\n  b\n)\n",
            vec![Annotation::new(
                2,
                1,
                0,
                "Empty line detected around arguments.",
            )],
        )
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .corrected("foo(a,\n  b\n)\n")
        .run();
        expect_no_offenses(COP, "foo(a,\n  b\n)\n");
        expect_no_offenses(COP, "foo(a, b)\n");
    }

    /// ブロックの引数と本体の位置。2 つの検査は同時には発火しない。
    #[test]
    fn multiline_block_layout() {
        const COP: &str = "Layout/MultilineBlockLayout";
        CopCase::new(
            COP,
            "bar { |a,\n  b| a }\n",
            vec![Annotation::new(
                1,
                7,
                3,
                "Block argument expression is not on the same line as the block start.",
            )],
        )
        .locations(&[(1, 7, 2, 4)])
        .lengths(&[8])
        .corrected("bar { |a, b|\n  a }\n")
        .run();
        CopCase::new(
            COP,
            "baz {\n  |a| a }\n",
            vec![Annotation::new(
                2,
                3,
                3,
                "Block argument expression is not on the same line as the block start.",
            )],
        )
        .corrected("baz { |a|\n  a }\n")
        .run();
        expect_no_offenses(COP, "bar { |a, b|\n  a\n}\n");
        expect_no_offenses(COP, "bar { |a| a }\n");
    }

    /// `rescue` と `ensure` の位置。既定では行頭に揃える。
    #[test]
    fn rescue_ensure_alignment() {
        const COP: &str = "Layout/RescueEnsureAlignment";
        CopCase::new(
            COP,
            "def foo\n  bar\n  rescue StandardError\n  baz\n  ensure\n  qux\nend\n",
            vec![
                Annotation::new(
                    3,
                    3,
                    6,
                    "`rescue` at 3, 2 is not aligned with `def foo` at 1, 0.",
                ),
                Annotation::new(
                    5,
                    3,
                    6,
                    "`ensure` at 5, 2 is not aligned with `def foo` at 1, 0.",
                ),
            ],
        )
        .corrected("def foo\n  bar\nrescue StandardError\n  baz\nensure\n  qux\nend\n")
        .run();
        expect_no_offenses(COP, "def foo\n  bar\nrescue StandardError\n  baz\nend\n");
        // 修飾子の `rescue` は別のノードなので見ない。
        expect_no_offenses(COP, "def foo\n  bar rescue baz\nend\n");
    }

    /// 自分の行に書かれたコメントの字下げ。基準は次の非空行。
    #[test]
    fn comment_indentation() {
        const COP: &str = "Layout/CommentIndentation";
        CopCase::new(
            COP,
            "def a\n    # comment\n  b\nend\n",
            vec![Annotation::new(
                2,
                5,
                9,
                "Incorrect indentation detected (column 4 instead of 2).",
            )],
        )
        .corrected("def a\n  # comment\n  b\nend\n")
        .run();
        expect_no_offenses(COP, "def a\n  # comment\n  b\nend\n");
        // 行末コメントは対象外。
        expect_no_offenses(COP, "def a\n  b # comment\nend\n");
        // `end` の手前は 1 段深い方に揃える。
        expect_no_offenses(COP, "def a\n  b\n  # comment\nend\n");
    }

    /// キーワードの前後の空白。`(` を許すキーワードは決まっている。
    #[test]
    fn space_around_keyword() {
        const COP: &str = "Layout/SpaceAroundKeyword";
        expect_offense(
            COP,
            r#"
            if(x)
            ^^ Space after keyword `if` is missing.
            end
            "#,
        );
        expect_correction(COP, "if(x)\nend\n", "if (x)\nend\n");
        expect_correction(COP, "while(y)\nend\n", "while (y)\nend\n");
        expect_no_offenses(COP, "if (x)\nend\n");
        // `(` を続けてよいのは `ACCEPT_LEFT_PAREN` の 7 語だけで、`return` は入っていない。
        expect_no_offenses(COP, "def a\n  yield(1)\nend\n");
        expect_offense(
            COP,
            r#"
            def a
              return(1)
              ^^^^^^ Space after keyword `return` is missing.
            end
            "#,
        );
    }

    /// 閉じ括弧の字下げ。引数が無いときは 3 つの候補のどれかであればよい。
    #[test]
    fn closing_parenthesis_indentation() {
        const COP: &str = "Layout/ClosingParenthesisIndentation";
        CopCase::new(
            COP,
            "foo(a,\n  b\n    )\n",
            vec![Annotation::new(3, 5, 1, "Indent `)` to column 0 (not 4)")],
        )
        .corrected("foo(a,\n  b\n)\n")
        .run();
        expect_no_offenses(COP, "foo(a,\n  b\n)\n");
        expect_no_offenses(COP, "baz(\n  a\n)\n");
        expect_no_offenses(COP, "qux(\n)\n");
        expect_no_offenses(COP, "def foo(a,\n  b\n)\nend\n");
    }
}

/// `Layout/MultilineMethodCallIndentation` と `Layout/MultilineOperationIndentation`。
/// 期待値は本家 1.89.0 の `--only <cop> --format json` / `-A` 実測。
mod layout_multiline_indentation {
    use super::*;

    const CALL: &str = "Layout/MultilineMethodCallIndentation";
    const OPERATION: &str = "Layout/MultilineOperationIndentation";

    /// 既定の `aligned` は連鎖の先頭のドットに揃える。先頭行にドットが無ければ
    /// レシーバの開始位置が基準になる。
    #[test]
    fn aligned_style_measures_against_the_first_dot_of_the_chain() {
        expect_offense(
            CALL,
            r#"
            Thing.a
               .b
               ^^ Align `.b` with `.a` on line 1.
              .c
              ^^ Align `.c` with `.a` on line 1.
            "#,
        );
        expect_offense(
            CALL,
            r#"
            x = Thing
               .a
               ^^ Align `.a` with `Thing` on line 1.
            "#,
        );
        expect_no_offenses(CALL, "Thing.a\n     .b\n     .c\n");
    }

    /// `BlockNode#single_line?` はブロック自身の区切り記号で判定するので、
    /// `Thing\n  .a { |x| x }` は「1 行のブロック」であり、連鎖の基準は `.a` になる。
    #[test]
    fn a_single_line_block_in_the_chain_keeps_its_own_dot_as_the_base() {
        expect_offense(
            CALL,
            r#"
            Thing
              .a { |x| x }
                 .b
                 ^^ Align `.b` with `.a` on line 2.
            "#,
        );
    }

    /// ハッシュのペアの中では値の開始位置が基準。
    #[test]
    fn inside_a_hash_pair_the_value_is_the_base() {
        expect_offense(
            CALL,
            r#"
            h = { k: value
              .call }
              ^^^^^ Align `.call` with `value` on line 1.
            "#,
        );
    }

    /// 引数リストの括弧の中は `not_for_this_cop?` で対象外。
    #[test]
    fn calls_inside_an_argument_list_are_not_this_cops_business() {
        expect_no_offenses(CALL, "foo(bar\n     .baz)\n");
        expect_no_offenses(CALL, "x = \"#\u{7b}a\n  .b}\"\n");
    }

    /// ブロック付きの呼び出しは、自分の行とブロックの本体・`end` の行だけを動かす。
    #[test]
    fn a_call_with_a_block_moves_its_selector_line_and_the_block() {
        expect_correction(
            CALL,
            "obj\n.foo do |x|\n  x\n  end\n",
            "obj\n  .foo do |x|\n    x\n    end\n",
        );
        expect_correction(CALL, "Thing.a\n   .b\n", "Thing.a\n     .b\n");
    }

    /// `aligned` は `if` / `while` の条件と、行頭から始まる代入の右辺では
    /// 演算子を揃え、それ以外では字下げを見る。
    #[test]
    fn operands_are_aligned_in_conditions_and_indented_elsewhere() {
        expect_offense(
            OPERATION,
            r#"
            if a +
                b
                ^ Align the operands of a condition in an `if` statement spanning multiple lines.
              c
            end
            "#,
        );
        expect_offense(
            OPERATION,
            r#"
            def m
              a &&
              b
              ^ Use 2 (not 0) spaces for indenting an expression spanning multiple lines.
            end
            "#,
        );
        expect_offense(
            OPERATION,
            r#"
            x = a &&
              b
              ^ Align the operands of an expression in an assignment spanning multiple lines.
            "#,
        );
        expect_no_offenses(OPERATION, "if a +\n   b\n  c\nend\n");
    }

    /// ドット付きの呼び出しは `relevant_node?` で除かれるので、こちらの cop は
    /// 触らない。単項演算子も同じ。
    #[test]
    fn dotted_calls_and_unary_operators_belong_to_the_other_cop() {
        expect_no_offenses(OPERATION, "Thing.a\n   .b\n");
        expect_no_offenses(OPERATION, "x = !foo\n");
    }

    /// `super` / `yield` / `defined?` は本家では send ではない別のノードなので、
    /// 「メソッド呼び出しの引数か」を見る `argument_in_method_call` は止まらないし、
    /// `defined?(...)` の括弧は書かれた括弧グループでもない。
    #[test]
    fn super_and_defined_are_not_method_calls() {
        expect_offense(
            OPERATION,
            r#"
            class Z
              def m
                defined?(a &&
                b)
                ^ Use 2 (not 0) spaces for indenting an expression spanning multiple lines.
                super(c &&
                d)
                ^ Use 2 (not 0) spaces for indenting an expression spanning multiple lines.
                super e &&
                f
                ^ Use 2 (not 0) spaces for indenting an expression spanning multiple lines.
                yield g &&
                h
                ^ Use 2 (not 0) spaces for indenting an expression spanning multiple lines.
              end
            end
            "#,
        );
        // 通常の呼び出しの括弧の中は対象外のまま。
        expect_no_offenses(OPERATION, "def m\n  puts(i &&\n  j)\nend\n");
    }

    /// `x = *y` の右辺は本家では `array` に包まれるので、`part_of_assignment_rhs` は
    /// そこで打ち切られて代入は基準にならない。
    #[test]
    fn a_lone_splat_on_the_right_of_an_assignment_is_wrapped_in_an_array() {
        expect_no_offenses(CALL, "def m(y)\n  x = *y\n    .to_a\n  x\nend\n");
        expect_offense(
            CALL,
            r#"
            def m(y)
              x = y
                .to_a
                ^^^^^ Align `.to_a` with `y` on line 2.
              x
            end
            "#,
        );
    }

    /// `foo.(1)` は `loc.selector` を持たないので、ハッシュのペアの中で連鎖すると
    /// 本家の `first_dot_alignment_base` が `dot.join(nil)` で cop エラーになり、
    /// そのノードの offense は落ちる。同じ位置で報告してはいけない。
    #[test]
    fn an_implicit_call_in_a_hash_pair_drops_the_offense_like_upstream_does() {
        expect_no_offenses(CALL, "h = { k: obj.(1)\n          .b\n }\n");
        expect_no_offenses(CALL, "foo(k: obj.(1)\n        .b\n)\n");
    }

    /// 連鎖の途中に複数行ブロックがあるとき、レシーバが「レシーバなしの呼び出し」なら
    /// それ自身が基準になり、局所変数ならブロックの親が基準になる。tree-sitter は
    /// どちらも `identifier` なので、本家と同じく代入の有無で見分ける必要がある。
    #[test]
    fn a_bare_receiver_is_a_call_unless_the_name_is_a_local_variable() {
        expect_no_offenses(
            CALL,
            concat!(
                "foo\n",
                "  .bar(k: obj.a do |x|\n",
                "            x\n",
                "          end\n",
                "          .b\n",
                ")\n",
            ),
        );
        expect_offense(
            CALL,
            r#"
            foo = 1
            foo
              .bar(k: obj.a do |x|
              ^^^^ Align `.bar` with `.b` on line 6.
                        x
                      end
                      .b
            )
            "#,
        );
    }

    #[test]
    fn operation_correction_moves_the_right_operand() {
        expect_correction(
            OPERATION,
            "if a +\n    b\n  c\nend\n",
            "if a +\n   b\n  c\nend\n",
        );
        expect_correction(OPERATION, "x = a &&\n  b\n", "x = a &&\n    b\n");
    }
}

/// `Layout/FirstArgumentIndentation` / `Layout/FirstParameterIndentation` /
/// `Layout/ParameterAlignment`。期待値は本家 1.89.0 の
/// `--only <cop> --format json` と `-A` の実測。
mod layout_first_argument_and_parameters {
    use super::*;

    const FIRST_ARGUMENT: &str = "Layout/FirstArgumentIndentation";
    const FIRST_PARAMETER: &str = "Layout/FirstParameterIndentation";
    const PARAMETER: &str = "Layout/ParameterAlignment";
    const PREVIOUS_LINE: &str =
        "Indent the first argument one step more than the start of the previous line.";
    const NESTED: &str = "Bad indentation of the first argument.";
    const PARAMETER_MSG: &str =
        "Align the parameters of a method definition if they span more than one line.";
    const FIRST_PARAMETER_MSG: &str = "Use 2 spaces for indentation in method args, relative to \
                                       the start of the line where the left parenthesis is.";

    /// 既定は `special_for_inner_method_call_in_parentheses`。括弧付きの呼び出しの
    /// 引数になっている呼び出しだけが自分の桁を基準にし、それ以外は直前のコード行の
    /// 字下げを基準にする。
    #[test]
    fn the_base_is_the_previous_code_line_unless_the_call_is_an_inner_one() {
        CopCase::new(
            FIRST_ARGUMENT,
            concat!(
                "some_method(\n",
                "first_param,\n",
                "second_param)\n",
                "\n",
                "foo = some_method(nested_call(\n",
                "nested_first_param),\n",
                "second_param)\n",
                "\n",
                "some_method nested_call(\n",
                "nested_first_param),\n",
                "second_param\n",
            ),
            vec![
                Annotation::new(2, 1, 11, PREVIOUS_LINE),
                Annotation::new(
                    6,
                    1,
                    18,
                    "Indent the first argument one step more than `nested_call(`.",
                ),
                Annotation::new(10, 1, 18, PREVIOUS_LINE),
            ],
        )
        .run();
    }

    /// 外側の呼び出しの補正がすでにその範囲を動かしているので、内側の offense は
    /// メッセージが変わり corrector を持たない。
    #[test]
    fn an_argument_inside_a_span_already_being_moved_carries_no_correction() {
        let report = CopCase::new(
            FIRST_ARGUMENT,
            concat!(
                "foo = some_method(\n",
                "nested_call(\n",
                "nested_first_param),\n",
                "second_param)\n",
            ),
            vec![
                // 注記は 1 行分しか表せないので、行を跨ぐレンジは `locations` で見る。
                Annotation::new(2, 1, 12, PREVIOUS_LINE),
                Annotation::new(3, 1, 18, NESTED),
            ],
        )
        .locations(&[(2, 1, 3, 19), (3, 1, 3, 18)])
        .lengths(&[32, 18])
        .run();
        let correctable: Vec<bool> = report
            .offenses
            .iter()
            .map(sonicop::diagnostic::Offense::is_correctable)
            .collect();
        assert_eq!(correctable, vec![true, false]);
    }

    /// `on_super` と `on_csend` も同じ扱い。`[]` と `+` は dot 無しで書かれた
    /// 演算子なので対象外で、`&.` は `dot?` ではないから `obj&.+(...)` も外れる。
    /// 波括弧の無いハッシュ引数は 1 個の `hash` なので、最初の引数は run 全体。
    #[test]
    fn bare_operators_are_left_alone_but_super_and_safe_navigation_are_not() {
        CopCase::new(
            FIRST_ARGUMENT,
            concat!(
                "super(\n",
                "1)\n",
                "a[\n",
                "0]\n",
                "x = 1 +\n",
                "2\n",
                "obj&.meth(\n",
                "1)\n",
                "obj&.+(\n",
                "1)\n",
                "obj.+(\n",
                "1)\n",
                "foo(\n",
                "a: 1,\n",
                "b: 2)\n",
                "foo(*bar(\n",
                "1))\n",
            ),
            vec![
                Annotation::new(2, 1, 1, PREVIOUS_LINE),
                Annotation::new(8, 1, 1, PREVIOUS_LINE),
                Annotation::new(12, 1, 1, PREVIOUS_LINE),
                Annotation::new(14, 1, 5, PREVIOUS_LINE),
                Annotation::new(17, 1, 1, PREVIOUS_LINE),
            ],
        )
        .locations(&[
            (2, 1, 2, 1),
            (8, 1, 8, 1),
            (12, 1, 12, 1),
            (14, 1, 15, 4),
            (17, 1, 17, 1),
        ])
        .lengths(&[1, 1, 1, 10, 1])
        .run();
    }

    /// 直前の行がコメントだけの行なら、その 1 つ上のコード行が基準になり、
    /// メッセージもそう名乗る。
    #[test]
    fn a_comment_only_line_is_skipped_when_looking_for_the_previous_line() {
        CopCase::annotated(
            FIRST_ARGUMENT,
            r#"
            some_method(
            # comment
            first_param)
            ^^^^^^^^^^^ Indent the first argument one step more than the start of the previous line (not counting the comment).
            "#,
        )
        .run();
    }

    /// `Layout/ArgumentAlignment` が固定字下げなら、最初の引数もそちらの持ち物に
    /// なるのでこの cop は下りる。
    #[test]
    fn a_fixed_argument_indentation_stands_the_cop_down() {
        CopCase::new(
            FIRST_ARGUMENT,
            "some_method(\nfirst_param,\nsecond_param)\n",
            Vec::new(),
        )
        .config("Layout/ArgumentAlignment:\n  EnforcedStyle: with_fixed_indentation\n")
        .run();
    }

    /// `consistent_relative_to_receiver` は括弧の有無を問わず呼び出しの桁を基準にする。
    #[test]
    fn consistent_relative_to_receiver_measures_from_the_call() {
        CopCase::annotated(
            FIRST_ARGUMENT,
            r#"
            foo = some_method(
            first_param,
            ^^^^^^^^^^^ Indent the first argument one step more than `some_method(`.
            second_param)
            "#,
        )
        .config(
            "Layout/FirstArgumentIndentation:\n  EnforcedStyle: consistent_relative_to_receiver\n",
        )
        .run();
    }

    /// `special_for_inner_method_call` は括弧を要求しないので、演算子・添字・属性代入も
    /// 「呼び出しの引数」として基準になる。`&&` だけは本家では `and` node で send では
    /// ないため、直前の行に落ちる。
    #[test]
    fn special_for_inner_method_call_treats_every_operator_as_a_call() {
        CopCase::new(
            FIRST_ARGUMENT,
            concat!(
                "x && foo(\n",
                "1)\n",
                "y = 1 + bar(\n",
                "2)\n",
                "outer baz(\n",
                "3)\n",
                "a[qux(\n",
                "4)]\n",
                "z.attr = quux(\n",
                "5)\n",
            ),
            vec![
                Annotation::new(2, 1, 1, PREVIOUS_LINE),
                Annotation::new(
                    4,
                    1,
                    1,
                    "Indent the first argument one step more than `bar(`.",
                ),
                Annotation::new(
                    6,
                    1,
                    1,
                    "Indent the first argument one step more than `baz(`.",
                ),
                Annotation::new(
                    8,
                    1,
                    1,
                    "Indent the first argument one step more than `qux(`.",
                ),
                Annotation::new(
                    10,
                    1,
                    1,
                    "Indent the first argument one step more than `quux(`.",
                ),
            ],
        )
        .config(
            "Layout/FirstArgumentIndentation:\n  EnforcedStyle: special_for_inner_method_call\n",
        )
        .run();
    }

    /// 定義の仮引数は `Layout/FirstParameterIndentation` と
    /// `Layout/ParameterAlignment` の分担。前者は左括弧の行の字下げ基準、後者は
    /// 最初の仮引数の桁基準。
    #[test]
    fn the_first_parameter_and_the_rest_are_measured_separately() {
        CopCase::new(
            FIRST_PARAMETER,
            concat!(
                "def some_method(\n",
                "first_param,\n",
                "second_param)\n",
                "  123\n",
                "end\n",
            ),
            vec![Annotation::new(2, 1, 11, FIRST_PARAMETER_MSG)],
        )
        .run();
        CopCase::new(
            PARAMETER,
            concat!("def foo(bar,\n", "     baz)\n", "  123\n", "end\n"),
            vec![Annotation::new(2, 6, 3, PARAMETER_MSG)],
        )
        .run();
        expect_no_offenses(
            PARAMETER,
            concat!("def foo(bar,\n", "        baz)\n", "  123\n", "end\n"),
        );
        expect_no_offenses(
            FIRST_PARAMETER,
            concat!("def foo(\n", "  bar,\n", "  baz)\n", "  123\n", "end\n"),
        );
    }

    /// 括弧の無い定義には左括弧が無いので `Layout/FirstParameterIndentation` は
    /// 何も言わないが、`Layout/ParameterAlignment` は仮引数を揃える。
    #[test]
    fn a_definition_without_parentheses_only_reaches_the_alignment_cop() {
        expect_no_offenses(FIRST_PARAMETER, concat!("def foo a,\n", "  b\n", "end\n"));
        CopCase::new(
            PARAMETER,
            concat!("def foo a,\n", "  b\n", "end\n"),
            vec![Annotation::new(2, 3, 1, PARAMETER_MSG)],
        )
        .run();
    }

    /// 既定値付きの仮引数が連続すると、tree-sitter は 1 個の `optional_parameter` に
    /// 潰して既定値を多重代入として読む。上流の parser は `optarg` を人数分持つので、
    /// 畳まれた列をほどかないと 2 個目以降が見えなくなる。
    #[test]
    fn a_folded_run_of_defaulted_parameters_is_unfolded() {
        CopCase::new(
            PARAMETER,
            concat!("def foo(bar = nil,\n", "     baz = nil)\n", "end\n"),
            vec![Annotation::new(2, 6, 9, PARAMETER_MSG)],
        )
        .run();
        CopCase::new(
            PARAMETER,
            concat!(
                "def zz(a = nil, b = nil, c = nil,\n",
                "   d = nil)\n",
                "end\n"
            ),
            vec![Annotation::new(2, 4, 7, PARAMETER_MSG)],
        )
        .run();
        CopCase::new(
            FIRST_PARAMETER,
            concat!("def qux(\n", "bar = nil,\n", "baz = nil)\n", "end\n"),
            vec![Annotation::new(2, 1, 9, FIRST_PARAMETER_MSG)],
        )
        .run();
    }

    /// `align_parentheses` は左括弧そのものを基準にし、`with_fixed_indentation` は
    /// `def` の行の字下げに 1 段足した桁を基準にする。
    #[test]
    fn the_alternative_styles_change_the_base_and_the_message() {
        CopCase::annotated(
            FIRST_PARAMETER,
            r#"
            def some_method(
            first_param,
            ^^^^^^^^^^^ Use 2 spaces for indentation in method args, relative to the position of the opening parenthesis.
            second_param)
              123
            end
            "#,
        )
        .config("Layout/FirstParameterIndentation:\n  EnforcedStyle: align_parentheses\n")
        .run();
        CopCase::annotated(
            PARAMETER,
            r#"
            def some_method(
            first_param,
            ^^^^^^^^^^^ Use one level of indentation for parameters following the first line of a multi-line method definition.
            second_param)
            ^^^^^^^^^^^^ Use one level of indentation for parameters following the first line of a multi-line method definition.
              123
            end
            "#,
        )
        .config("Layout/ParameterAlignment:\n  EnforcedStyle: with_fixed_indentation\n")
        .run();
    }

    /// autocorrect は `AlignmentCorrector.correct` そのもので、node が跨ぐ行を
    /// まとめて横に動かす。内側の呼び出しは外側が動いた次の周回で揃う。
    #[test]
    fn correction_moves_every_line_the_reported_node_spans() {
        CopCase::new(
            FIRST_ARGUMENT,
            concat!(
                "some_method(\n",
                "first_param,\n",
                "second_param)\n",
                "\n",
                "foo = some_method(nested_call(\n",
                "nested_first_param),\n",
                "second_param)\n",
                "\n",
                "foo = some_method(\n",
                "nested_call(\n",
                "nested_first_param),\n",
                "second_param)\n",
                "\n",
                "some_method nested_call(\n",
                "nested_first_param),\n",
                "second_param\n",
            ),
            Vec::new(),
        )
        .without_offense_check()
        .corrected(concat!(
            "some_method(\n",
            "  first_param,\n",
            "second_param)\n",
            "\n",
            "foo = some_method(nested_call(\n",
            "                    nested_first_param),\n",
            "second_param)\n",
            "\n",
            "foo = some_method(\n",
            "  nested_call(\n",
            "    nested_first_param),\n",
            "second_param)\n",
            "\n",
            "some_method nested_call(\n",
            "  nested_first_param),\n",
            "second_param\n",
        ))
        .run();
    }

    /// 定義側の 2 つの cop は互いの結果の上で収束する。最初の仮引数が字下げされ、
    /// 残りがその桁に揃う。
    #[test]
    fn the_definition_cops_settle_on_each_other() {
        expect_correction(
            FIRST_PARAMETER,
            concat!(
                "def some_method(\n",
                "first_param,\n",
                "second_param)\n",
                "  123\n",
                "end\n",
            ),
            concat!(
                "def some_method(\n",
                "  first_param,\n",
                "second_param)\n",
                "  123\n",
                "end\n",
            ),
        );
        expect_correction(
            PARAMETER,
            concat!("def foo(bar,\n", "     baz)\n", "  123\n", "end\n"),
            concat!("def foo(bar,\n", "        baz)\n", "  123\n", "end\n"),
        );
    }
}

/// メソッド名と第一引数の間の空白。整列のための空白は許される。
mod layout_space_before_first_arg {
    use super::*;

    #[test]
    fn space_before_first_arg() {
        const COP: &str = "Layout/SpaceBeforeFirstArg";
        expect_offense(
            COP,
            r#"
            foo  1
               ^^ Put one space between the method name and the first argument.
            "#,
        );
        expect_correction(COP, "foo  1\n", "foo 1\n");
        expect_no_offenses(COP, "foo 1\n");
        expect_no_offenses(COP, "foo(1)\n");
        // 引数が次の行にあるものは対象外。
        expect_no_offenses(COP, "foo \\\n  1\n");
        // 演算子とセッターは `regular_method_call_with_arguments?` で落ちる。
        expect_no_offenses(COP, "a  +  b\n");
    }
}

/// Lint 部門の後発 cop (第 2 陣)。期待値は本家 1.89.0 の実出力から。
mod lint_late_additions_two {
    use super::*;

    /// `each` の最後の式は捨てられないので、ブロックの中は void ではない。
    #[test]
    fn void_leaves_the_last_expression_of_an_each_block_alone() {
        expect_no_offenses("Lint/Void", "[1].each do |x|\n  x\nend\n");
    }

    #[test]
    fn void_reports_the_last_expression_of_a_void_context() {
        CopCase::annotated(
            "Lint/Void",
            r#"
            def initialize
              @a = 1
              self
              ^^^^ `self` used in void context.
            end
            "#,
        )
        .run();
    }

    /// `def foo=` の最後の式は Ruby が捨てるが、返り値を当てにできるので免除される。
    #[test]
    fn void_leaves_the_last_expression_of_a_setter_alone() {
        expect_no_offenses("Lint/Void", "def foo=(v)\n  v\nend\n");
    }

    #[test]
    fn void_reports_a_literal_in_an_ensure_clause() {
        CopCase::annotated(
            "Lint/Void",
            r#"
            begin
              x
            ensure
              1
              ^ Literal `1` used in void context.
              2
              ^ Literal `2` used in void context.
            end
            "#,
        )
        .run();
    }

    #[test]
    fn format_parameter_mismatch_counts_percent_fields() {
        CopCase::annotated(
            "Lint/FormatParameterMismatch",
            r#"
            "%s" % [1, 2]
                 ^ Number of arguments (2) to `String#%` doesn't match the number of fields (1).
            "#,
        )
        .run();
    }

    #[test]
    fn format_parameter_mismatch_reports_mixed_sequence_types() {
        CopCase::annotated(
            "Lint/FormatParameterMismatch",
            r#"
            format("%1$s %s", 1, 2)
            ^^^^^^ Format string is invalid because formatting sequence types (numbered, named or unnumbered) are mixed.
            "#,
        )
        .run();
    }

    #[test]
    fn format_parameter_mismatch_accepts_a_matching_call() {
        expect_no_offenses("Lint/FormatParameterMismatch", "format(\"%s %s\", 1, 2)\n");
    }

    /// 括弧付きの引数リストは字句解析が迷わないので対象外。
    #[test]
    fn ambiguous_operator_accepts_parenthesized_arguments() {
        expect_no_offenses("Lint/AmbiguousOperator", "foo(*[])\n");
    }

    /// 演算子の右に空白があれば曖昧ではない。
    #[test]
    fn ambiguous_operator_accepts_a_spaced_operator() {
        expect_no_offenses("Lint/AmbiguousOperator", "foo * []\n");
    }

    #[test]
    fn ambiguous_operator_reports_yield_and_super() {
        CopCase::annotated(
            "Lint/AmbiguousOperator",
            r#"
            def m
              yield *[]
                    ^ Ambiguous splat operator. Parenthesize the method arguments if it's surely a splat operator, or add a whitespace to the right of the `*` if it should be a multiplication.
            end
            "#,
        )
        .corrected("def m\n  yield(*[])\nend\n")
        .run();
    }

    #[test]
    fn ambiguous_operator_names_the_keyword_splat() {
        CopCase::annotated(
            "Lint/AmbiguousOperator",
            r#"
            foo **{a: 1}
                ^^ Ambiguous keyword splat operator. Parenthesize the method arguments if it's surely a keyword splat operator, or add a whitespace to the right of the `**` if it should be an exponent.
            "#,
        )
        .run();
    }

    #[test]
    fn ambiguous_regexp_literal_accepts_a_division() {
        expect_no_offenses("Lint/AmbiguousRegexpLiteral", "foo / re / 1\n");
    }

    /// `# rubocop:disable all` はこの cop 自身も止めるので、報告は残らない。
    #[test]
    fn missing_cop_enable_directive_ignores_a_blanket_disable() {
        expect_no_offenses(
            "Lint/MissingCopEnableDirective",
            "# rubocop:disable all\nfoo = 1\n",
        );
    }

    #[test]
    fn missing_cop_enable_directive_names_a_department() {
        CopCase::annotated(
            "Lint/MissingCopEnableDirective",
            "# rubocop:disable Layout\n^^^^^^^^^^^^^^^^^^^^^^^^ Re-enable Layout department with `# rubocop:enable` after disabling it.\nfoo = 1\n",
        )
        .run();
    }

    #[test]
    fn missing_cop_enable_directive_accepts_a_closed_range() {
        expect_no_offenses(
            "Lint/MissingCopEnableDirective",
            "# rubocop:disable Layout/LineLength\nfoo = 1\n# rubocop:enable Layout/LineLength\n",
        );
    }

    #[test]
    fn redundant_cop_enable_directive_reports_only_the_second_enable() {
        CopCase::annotated(
            "Lint/RedundantCopEnableDirective",
            "x = 1\n# rubocop:disable Layout/LineLength\ny = 2\n# rubocop:enable Layout/LineLength\n# rubocop:enable Layout/LineLength\n                 ^^^^^^^^^^^^^^^^^ Unnecessary enabling of Layout/LineLength.\n",
        )
        .run();
    }

    #[test]
    fn redundant_cop_enable_directive_reports_the_extra_name_of_a_list() {
        CopCase::annotated(
            "Lint/RedundantCopEnableDirective",
            "# rubocop:disable Layout/LineLength, Lint/Void\ny = 2\n# rubocop:enable Layout/LineLength, Lint/Void, Style/IfUnlessModifier\n                                               ^^^^^^^^^^^^^^^^^^^^^^ Unnecessary enabling of Style/IfUnlessModifier.\n",
        )
        .corrected("# rubocop:disable Layout/LineLength, Lint/Void\ny = 2\n# rubocop:enable Layout/LineLength, Lint/Void\n")
        .run();
    }

    #[test]
    fn redundant_cop_enable_directive_names_all_cops() {
        CopCase::annotated(
            "Lint/RedundantCopEnableDirective",
            "# rubocop:enable all\n                 ^^^ Unnecessary enabling of all cops.\nfoo = 1\n",
        )
        .corrected("foo = 1\n")
        .run();
    }
}

/// `Style/BeginBlock` / `Style/EndBlock`: Perl 由来のブロック。
///
/// 期待値は本家 1.89.0 の `--only <cop>` と `-A` の実測。
mod begin_and_end_blocks {
    use super::*;

    #[test]
    fn the_keyword_is_reported_and_only_end_is_correctable() {
        expect_offense(
            "Style/BeginBlock",
            r#"
            BEGIN { test }
            ^^^^^ Avoid the use of `BEGIN` blocks.
            "#,
        );
        expect_correction(
            "Style/EndBlock",
            "END { puts 'x' }\n",
            "at_exit { puts 'x' }\n",
        );
        expect_no_offenses("Style/BeginBlock", "at_exit { puts 'x' }\n");
        expect_no_offenses("Style/EndBlock", "BEGIN { test }\n");
    }
}

/// `Style/BlockComments`: `=begin ... =end` を行コメントへ。
///
/// 期待値は本家 1.89.0 の `--only Style/BlockComments` と `-A` の実測。
mod block_comments {
    use super::*;

    const COP: &str = "Style/BlockComments";

    #[test]
    fn the_fences_go_and_every_line_gains_a_hash() {
        expect_correction(
            COP,
            "=begin\nMultiple lines\nof comments...\n=end\nx = 1\n",
            "# Multiple lines\n# of comments...\nx = 1\n",
        );
        // 空行は `#` だけの行になり、その次の行にも `# ` が付く。
        expect_correction(COP, "=begin\na\n\n\nb\n=end\n", "# a\n#\n# \n# b\n");
        expect_no_offenses(COP, "# a\n# b\n");
    }

    /// 本家は `=end` ではなくコメント末尾から 5 文字を数えるので、行末に何か
    /// 書いてあるとそちらが消える。`=begin\n=end` は範囲が逆転して本家自身が
    /// 落ちるため、何も報告しない。
    #[test]
    fn the_end_fence_is_measured_from_the_comment_end() {
        expect_correction(
            COP,
            "=begin extra\nfoo\n=end tail\nx=1\n",
            "# extra\n# foo\n# =end x=1\n",
        );
        expect_correction(COP, "=begin\nx\n=end", "nd");
        expect_no_offenses(COP, "=begin\n=end");
    }
}

/// `Style/ClassMethods`: クラス名ではなく `self` で特異メソッドを定義する。
///
/// 期待値は本家 1.89.0 の `--only Style/ClassMethods` と `-A` の実測。
mod class_methods {
    use super::*;

    const COP: &str = "Style/ClassMethods";

    #[test]
    fn the_name_part_of_the_receiver_is_reported() {
        expect_offense(
            COP,
            r#"
            class SomeClass
              def SomeClass.class_method
                  ^^^^^^^^^ Use `self.class_method` instead of `SomeClass.class_method`.
              end
            end
            "#,
        );
        expect_correction(
            COP,
            "module Foo\n  def Foo.bar; end\nend\n",
            "module Foo\n  def self.bar; end\nend\n",
        );
    }

    /// 名前が食い違うもの、`self` で書かれたもの、本体の直下にないものは対象外。
    #[test]
    fn only_a_direct_child_naming_this_very_class_counts() {
        expect_no_offenses(COP, "class Foo\n  def Bar.baz; end\nend\n");
        expect_no_offenses(COP, "class Foo\n  def self.baz; end\nend\n");
        expect_no_offenses(COP, "class Foo\n  if x\n    def Foo.baz; end\n  end\nend\n");
    }
}

/// `Style/ColonMethodCall` / `Style/ColonMethodDefinition`: `::` はメソッドに使わない。
///
/// 期待値は本家 1.89.0 の `--only <cop>` と `-A` の実測。
mod colon_methods {
    use super::*;

    #[test]
    fn the_operator_is_reported_and_becomes_a_dot() {
        expect_offense(
            "Style/ColonMethodCall",
            r#"
            Timeout::timeout(500) { do_something }
                   ^^ Do not use `::` for method calls.
            "#,
        );
        expect_correction(
            "Style/ColonMethodDefinition",
            "def self::bar\nend\n",
            "def self.bar\nend\n",
        );
    }

    /// 定数参照と JRuby の `Java::` は対象外。受け手のない呼び出しも同じ。
    #[test]
    fn constants_and_java_interop_are_left_alone() {
        expect_no_offenses("Style/ColonMethodCall", "Timeout::Error\n");
        expect_no_offenses("Style/ColonMethodCall", "Java::int\n");
        expect_no_offenses("Style/ColonMethodCall", "Java::com::example::Foo.bar\n");
        expect_no_offenses("Style/ColonMethodCall", "Timeout.timeout(500)\n");
        expect_no_offenses("Style/ColonMethodDefinition", "def self.bar\nend\n");
    }
}

/// `Style/DefWithParentheses`: 引数を取らない定義の `()` は書かない。
///
/// 期待値は本家 1.89.0 の `--only Style/DefWithParentheses` と `-A` の実測。
mod def_with_parentheses {
    use super::*;

    const COP: &str = "Style/DefWithParentheses";

    #[test]
    fn the_empty_parentheses_are_reported_and_removed() {
        expect_offense(
            COP,
            r#"
            def foo()
                   ^^ Omit the parentheses in defs when the method doesn't accept any arguments.
              do_something
            end
            "#,
        );
        expect_correction(COP, "def Baz.foo()\nend\n", "def Baz.foo\nend\n");
        // `;` が続けば 1 行でも外せる。
        expect_correction(COP, "def foo(); end\n", "def foo; end\n");
        CopCase::annotated(COP, "def foo() = do_something\n")
            .target_ruby("3.0")
            .without_offense_check()
            .corrected("def foo = do_something\n")
            .run();
    }

    /// 外すと構文エラーになる書き方は残る。
    #[test]
    fn the_parentheses_that_are_load_bearing_stay() {
        expect_no_offenses(COP, "def foo() do_something end\n");
        CopCase::annotated(COP, "def foo()=do_something\n")
            .target_ruby("3.0")
            .corrected("def foo()=do_something\n")
            .run();
        expect_no_offenses(COP, "def foo(a)\nend\n");
        expect_no_offenses(COP, "def foo\nend\n");
    }
}

/// `Style/EachForSimpleLoop`: 定数回の `(a..b).each` は `Integer#times`。
///
/// 期待値は本家 1.89.0 の `--only Style/EachForSimpleLoop` と `-A` の実測。
mod each_for_simple_loop {
    use super::*;

    const COP: &str = "Style/EachForSimpleLoop";

    #[test]
    fn the_call_is_replaced_by_the_number_of_iterations() {
        expect_offense(
            COP,
            r#"
            (1..5).each { }
            ^^^^^^^^^^^ Use `Integer#times` for a simple loop which iterates a fixed number of times.
            "#,
        );
        expect_correction(COP, "(0...10).each {}\n", "10.times {}\n");
        expect_correction(COP, "(1..5).each do\nend\n", "5.times do\nend\n");
    }

    /// ブロック引数を取るもの、範囲がリテラルでないもの、`each` でないものは対象外。
    #[test]
    fn a_block_taking_anything_or_a_non_literal_range_is_left_alone() {
        expect_no_offenses(COP, "(1..5).each { |n| }\n");
        expect_no_offenses(COP, "(1..n).each { }\n");
        expect_no_offenses(COP, "(1..5).map { }\n");
        expect_no_offenses(COP, "1..5\n");
        // `_1` を読むブロックは `numblock` で、本家の `on_block` は呼ばれない。
        expect_no_offenses(COP, "(1..5).each { _1 }\n");
    }
}

/// `Style/EmptyBlockParameter` / `Style/EmptyLambdaParameter`: 空の仮引数は書かない。
///
/// 期待値は本家 1.89.0 の `--only <cop>` と `-A` の実測。
mod empty_parameters {
    use super::*;

    #[test]
    fn the_empty_delimiters_are_reported_and_removed() {
        expect_offense(
            "Style/EmptyBlockParameter",
            r#"
            a do ||
                 ^^ Omit pipes for the empty block parameters.
              do_something
            end
            "#,
        );
        expect_correction(
            "Style/EmptyBlockParameter",
            "a { || do_something }\n",
            "a { do_something }\n",
        );
        expect_correction(
            "Style/EmptyLambdaParameter",
            "-> () { do_something }\n",
            "-> { do_something }\n",
        );
        expect_correction("Style/EmptyLambdaParameter", "->() { x }\n", "-> { x }\n");
    }

    /// 引数を取るもの、区切りを書いていないものは対象外。`->` は片方だけの担当。
    #[test]
    fn each_cop_keeps_to_its_own_kind_of_block() {
        expect_no_offenses("Style/EmptyBlockParameter", "a do\nend\n");
        expect_no_offenses("Style/EmptyBlockParameter", "a { |x| }\n");
        expect_no_offenses("Style/EmptyBlockParameter", "-> () { x }\n");
        expect_no_offenses("Style/EmptyLambdaParameter", "-> { x }\n");
        expect_no_offenses("Style/EmptyLambdaParameter", "lambda { || x }\n");
        expect_no_offenses("Style/EmptyLambdaParameter", "-> (a) { a }\n");
    }
}

/// `Style/UnlessElse`: `unless/else` は肯定形に書き換える。
///
/// 期待値は本家 1.89.0 の `--only Style/UnlessElse` と `-A` の実測。
mod unless_else {
    use super::*;

    const COP: &str = "Style/UnlessElse";

    #[test]
    fn the_two_branches_swap_and_the_keyword_flips() {
        expect_correction(
            COP,
            "unless foo\n  a\nelse\n  b\nend\n",
            "if foo\n  b\nelse\n  a\nend\n",
        );
        // `then` が書かれていれば本体はその後ろから始まる。
        expect_correction(
            COP,
            "unless foo then a else b end\n",
            "if foo then b else a end\n",
        );
    }

    #[test]
    fn an_unless_without_else_is_left_alone() {
        expect_no_offenses(COP, "unless foo\n  a\nend\n");
        expect_no_offenses(COP, "if foo\n  a\nelse\n  b\nend\n");
    }
}

/// `Style/WhileUntilDo`: 複数行の `while`/`until` に `do` は要らない。
///
/// 期待値は本家 1.89.0 の `--only Style/WhileUntilDo` と `-A` の実測。
mod while_until_do {
    use super::*;

    const COP: &str = "Style/WhileUntilDo";

    #[test]
    fn the_do_is_reported_and_removed_with_the_space_before_it() {
        expect_offense(
            COP,
            r#"
            while x.any? do
                         ^^ Do not use `do` with multi-line `while`.
              do_something(x.pop)
            end
            "#,
        );
        expect_correction(
            COP,
            "until x.empty? do\n  x.pop\nend\n",
            "until x.empty?\n  x.pop\nend\n",
        );
    }

    #[test]
    fn a_single_line_loop_and_one_without_do_are_left_alone() {
        expect_no_offenses(COP, "while x.any?\n  x.pop\nend\n");
        expect_no_offenses(COP, "x.pop while x.any?\n");
    }
}

/// `Style/MultilineIfThen` / `Style/MultilineWhenThen`: 複数行の `then` は冗長。
///
/// 期待値は本家 1.89.0 の `--only <cop>` と `-A` の実測。
mod multiline_then {
    use super::*;

    #[test]
    fn the_then_is_reported_under_the_keyword_that_owns_it() {
        expect_offense(
            "Style/MultilineIfThen",
            r#"
            if cond then
                    ^^^^ Do not use `then` for multi-line `if`.
              a
            end
            "#,
        );
        // `elsif` は本家では `if` ノードなので、自分のキーワードで報告される。
        expect_offense(
            "Style/MultilineIfThen",
            r#"
            if a
              x
            elsif b then
                    ^^^^ Do not use `then` for multi-line `elsif`.
              y
            end
            "#,
        );
        expect_correction(
            "Style/MultilineIfThen",
            "unless d then\n  w\nend\n",
            "unless d\n  w\nend\n",
        );
        expect_correction(
            "Style/MultilineWhenThen",
            "case foo\nwhen bar then\nend\n",
            "case foo\nwhen bar\nend\n",
        );
    }

    /// `then` と同じ行に本体があるなら残す。
    #[test]
    fn a_body_on_the_then_line_keeps_it() {
        expect_no_offenses("Style/MultilineIfThen", "if e then f\nend\n");
        expect_no_offenses("Style/MultilineIfThen", "if e\n  f\nend\n");
        expect_no_offenses(
            "Style/MultilineWhenThen",
            "case foo\nwhen bar then baz\nend\n",
        );
        expect_no_offenses(
            "Style/MultilineWhenThen",
            "case foo\nwhen bar\n  baz\nend\n",
        );
    }
}

/// `Style/NegatedWhile` / `Style/NegatedUnless`: 否定条件は逆のキーワードで。
///
/// 期待値は本家 1.89.0 の `--only <cop>` と `-A` の実測。
mod negated_conditionals {
    use super::*;

    #[test]
    fn the_keyword_flips_and_the_negation_goes() {
        expect_correction(
            "Style/NegatedWhile",
            "while !foo\n  bar\nend\n",
            "until foo\n  bar\nend\n",
        );
        expect_correction("Style/NegatedWhile", "bar until !foo\n", "bar while foo\n");
        expect_correction(
            "Style/NegatedUnless",
            "unless !foo\n  bar\nend\n",
            "if foo\n  bar\nend\n",
        );
        expect_correction("Style/NegatedUnless", "bar unless !foo\n", "bar if foo\n");
    }

    /// 二重否定と、否定が条件の一部でしかないものは対象外。`else` 付きも同じ。
    #[test]
    fn a_condition_that_is_not_a_single_negation_is_left_alone() {
        expect_no_offenses("Style/NegatedWhile", "bar while !foo && baz\n");
        expect_no_offenses("Style/NegatedWhile", "bar while !!foo\n");
        expect_no_offenses("Style/NegatedWhile", "bar while foo\n");
        expect_no_offenses("Style/NegatedUnless", "unless !foo\n  a\nelse\n  b\nend\n");
        expect_no_offenses("Style/NegatedUnless", "bar if !foo\n");
    }
}

/// `Style/Not`: `not` ではなく `!`。
///
/// 期待値は本家 1.89.0 の `--only Style/Not` と `-A` の実測。
mod not {
    use super::*;

    const COP: &str = "Style/Not";

    #[test]
    fn the_keyword_is_reported_and_becomes_a_bang() {
        expect_offense(
            COP,
            r#"
            x = (not something)
                 ^^^ Use `!` instead of `not`.
            "#,
        );
        expect_correction(COP, "x = (not something)\n", "x = (!something)\n");
    }

    /// 比較なら演算子を裏返し、束縛が変わるものは括弧で包む。
    #[test]
    fn a_comparison_flips_and_a_looser_expression_gains_parentheses() {
        expect_correction(COP, "x = (not a == b)\n", "x = (a != b)\n");
        expect_correction(COP, "x = (not a <= b)\n", "x = (a > b)\n");
        expect_correction(COP, "x = (not a && b)\n", "x = (!(a && b))\n");
        expect_correction(COP, "x = (not a + b)\n", "x = (!(a + b))\n");
        expect_no_offenses(COP, "x = !something\n");
    }
}

/// `Style/MinMax`: `[x.min, x.max]` は `x.minmax`。
///
/// 期待値は本家 1.89.0 の `--only Style/MinMax` と `-A` の実測。
mod min_max {
    use super::*;

    const COP: &str = "Style/MinMax";

    #[test]
    fn the_pair_is_reported_and_replaced() {
        expect_offense(
            COP,
            r#"
            bar = [foo.min, foo.max]
                  ^^^^^^^^^^^^^^^^^^ Use `foo.minmax` instead of `[foo.min, foo.max]`.
            "#,
        );
        expect_correction(COP, "bar = [foo.min, foo.max]\n", "bar = foo.minmax\n");
        // `return` は括弧を持たないので、引数の範囲だけが対象。
        expect_correction(
            COP,
            "def m\n  return foo.min, foo.max\nend\n",
            "def m\n  return foo.minmax\nend\n",
        );
    }

    /// 受け手が食い違うもの、順序が逆のもの、受け手がないものは対象外。
    #[test]
    fn the_two_calls_have_to_be_min_then_max_on_the_same_receiver() {
        expect_no_offenses(COP, "bar = [foo.min, baz.max]\n");
        expect_no_offenses(COP, "bar = [foo.max, foo.min]\n");
        expect_no_offenses(COP, "bar = [min, max]\n");
        expect_no_offenses(COP, "bar = [foo.min, foo.max, foo.size]\n");
    }
}

/// `Style/MultilineMemoization`: 複数行の `||=` は `begin`/`end` で包む。
///
/// 期待値は本家 1.89.0 の `--only Style/MultilineMemoization` と `-A` の実測。
mod multiline_memoization {
    use super::*;

    const COP: &str = "Style/MultilineMemoization";

    #[test]
    fn the_parentheses_become_keywords() {
        expect_correction(
            COP,
            "foo ||= (\n  bar\n  baz\n)\n",
            "foo ||= begin\n  bar\n  baz\nend\n",
        );
    }

    #[test]
    fn a_single_line_or_a_begin_block_is_left_alone() {
        expect_no_offenses(COP, "foo ||= (bar)\n");
        expect_no_offenses(COP, "foo ||= begin\n  bar\n  baz\nend\n");
        expect_no_offenses(COP, "foo = (\n  bar\n  baz\n)\n");
    }

    /// `braces` では逆に `begin`/`end` を括弧へ。
    #[test]
    fn the_braces_style_reverses_the_rule() {
        CopCase::annotated(COP, "foo ||= begin\n  bar\n  baz\nend\n")
            .config("Style/MultilineMemoization:\n  EnforcedStyle: braces\n")
            .without_offense_check()
            .corrected("foo ||= (\n  bar\n  baz\n)\n")
            .run();
    }
}

/// `Style/IfUnlessModifierOfIfUnless`: 条件式の後ろに条件修飾子を重ねない。
///
/// 期待値は本家 1.89.0 の `--only Style/IfUnlessModifierOfIfUnless` と `-A` の実測。
mod if_unless_modifier_of_if_unless {
    use super::*;

    const COP: &str = "Style/IfUnlessModifierOfIfUnless";

    #[test]
    fn the_outer_condition_moves_in_front_of_the_body() {
        expect_offense(
            COP,
            r#"
            'stop' if tired? if running?
                             ^^ Avoid modifier `if` after another conditional.
            "#,
        );
        expect_correction(
            COP,
            "'stop' if tired? if running?\n",
            "if running?\n'stop' if tired?\nend\n",
        );
        expect_correction(
            COP,
            "tired? ? 'stop' : 'go' unless running?\n",
            "unless running?\ntired? ? 'stop' : 'go'\nend\n",
        );
    }

    #[test]
    fn a_body_that_is_not_a_conditional_is_left_alone() {
        expect_no_offenses(COP, "'stop' if running?\n");
        expect_no_offenses(COP, "foo(bar) if running?\n");
    }
}

/// `Style/Strip`: `lstrip.rstrip` は `strip`。
///
/// 期待値は本家 1.89.0 の `--only Style/Strip` と `-A` の実測。
mod strip {
    use super::*;

    const COP: &str = "Style/Strip";

    #[test]
    fn either_order_is_reported_from_the_first_selector() {
        expect_correction(COP, "'abc'.lstrip.rstrip\n", "'abc'.strip\n");
        expect_correction(COP, "'abc'.rstrip.lstrip\n", "'abc'.strip\n");
    }

    #[test]
    fn one_of_the_two_alone_is_left_alone() {
        expect_no_offenses(COP, "'abc'.lstrip\n");
        expect_no_offenses(COP, "'abc'.strip\n");
        expect_no_offenses(COP, "'abc'.lstrip.lstrip\n");
    }
}

/// `Style/RedundantSortBy`: `sort_by { |x| x }` は `sort`。
///
/// 期待値は本家 1.89.0 の `--only Style/RedundantSortBy` と `-A` の実測。
mod redundant_sort_by {
    use super::*;

    const COP: &str = "Style/RedundantSortBy";

    #[test]
    fn a_block_returning_its_own_parameter_is_reported() {
        expect_correction(COP, "array.sort_by { |x| x }\n", "array.sort\n");
        expect_correction(COP, "array.sort_by do |var|\n  var\nend\n", "array.sort\n");
    }

    #[test]
    fn a_block_doing_anything_else_is_left_alone() {
        expect_no_offenses(COP, "array.sort_by { |x| x.foo }\n");
        expect_no_offenses(COP, "array.sort_by { |x, y| x }\n");
        expect_no_offenses(COP, "array.sort { |a, b| a }\n");
    }
}

/// `Style/DoubleCopDisableDirective`: 1 行に disable 指示は 1 つ。
///
/// 期待値は本家 1.89.0 の `--only Style/DoubleCopDisableDirective` と `-A` の実測。
mod double_cop_disable_directive {
    use super::*;

    const COP: &str = "Style/DoubleCopDisableDirective";

    #[test]
    fn the_second_directive_becomes_a_comma() {
        expect_correction(
            COP,
            "def f # rubocop:disable Style/For # rubocop:disable Metrics/AbcSize\nend\n",
            "def f # rubocop:disable Style/For, Metrics/AbcSize\nend\n",
        );
    }

    #[test]
    fn a_single_directive_is_left_alone() {
        expect_no_offenses(COP, "def f # rubocop:disable Style/For\nend\n");
        expect_no_offenses(COP, "# rubocop:disable Style/For\n");
    }
}

/// `Style/TrailingMethodEndStatement` と `Style/TrailingBodyOn*`: 本体と `end` は自分の行に。
///
/// 期待値は本家 1.89.0 の `--only <cop>` と `-A` の実測。
mod trailing_body_and_end {
    use super::*;

    #[test]
    fn the_body_moves_below_the_signature() {
        expect_correction(
            "Style/TrailingBodyOnClass",
            "class Foo; def foo; end\nend\n",
            "class Foo \n  def foo; end\nend\n",
        );
        expect_correction(
            "Style/TrailingBodyOnModule",
            "module Bar extend self\nend\n",
            "module Bar \n  extend self\nend\n",
        );
        expect_correction(
            "Style/TrailingBodyOnMethodDefinition",
            "def g(x); b = foo\n  b[c: x]\nend\n",
            "def g(x) \n  b = foo\n  b[c: x]\nend\n",
        );
        expect_correction(
            "Style/TrailingMethodEndStatement",
            "def some_method\ndo_stuff; end\n",
            "def some_method\ndo_stuff; \nend\n",
        );
    }

    #[test]
    fn a_body_already_on_its_own_line_is_left_alone() {
        expect_no_offenses(
            "Style/TrailingBodyOnClass",
            "class Foo\n  def foo; end\nend\n",
        );
        expect_no_offenses(
            "Style/TrailingBodyOnModule",
            "module Bar\n  extend self\nend\n",
        );
        expect_no_offenses(
            "Style/TrailingBodyOnMethodDefinition",
            "def g(x)\n  b = foo\nend\n",
        );
        expect_no_offenses("Style/TrailingMethodEndStatement", "def m\n  x\nend\n");
        expect_no_offenses("Style/TrailingMethodEndStatement", "def m; x; end\n");
    }
}

/// `Style/RedundantConditional`: 真偽値だけを返す条件式は条件そのもの。
///
/// 期待値は本家 1.89.0 の `--only Style/RedundantConditional` と `-A` の実測。
mod redundant_conditional {
    use super::*;

    const COP: &str = "Style/RedundantConditional";

    #[test]
    fn both_orders_are_reported_and_the_inverted_one_gains_a_bang() {
        expect_correction(COP, "z = (x == y ? true : false)\n", "z = (x == y)\n");
        expect_correction(COP, "z = (x == y ? false : true)\n", "z = (!(x == y))\n");
        expect_correction(COP, "if x == y\n  true\nelse\n  false\nend\n", "x == y\n");
    }

    /// 条件が比較でないもの、枝が真偽値でないものは対象外。
    #[test]
    fn anything_but_a_comparison_returning_booleans_is_left_alone() {
        expect_no_offenses(COP, "z = (x ? true : false)\n");
        expect_no_offenses(COP, "z = (x == y ? 1 : false)\n");
        expect_no_offenses(COP, "z = (x == y)\n");
    }
}

/// `Style/NilComparison`: 既定では `== nil` より `nil?`。
///
/// 期待値は本家 1.89.0 の `--only Style/NilComparison` と `-A` の実測。
mod nil_comparison {
    use super::*;

    const COP: &str = "Style/NilComparison";

    #[test]
    fn the_comparison_becomes_the_predicate() {
        expect_offense(
            COP,
            r#"
            if x == nil
                 ^^ Prefer the use of the `nil?` predicate.
            end
            "#,
        );
        expect_correction(COP, "x == nil\n", "x.nil?\n");
        expect_correction(COP, "x === nil\n", "x.nil?\n");
        expect_no_offenses(COP, "x.nil?\n");
        expect_no_offenses(COP, "x != nil\n");
    }

    /// `comparison` では逆向きになる。
    #[test]
    fn the_comparison_style_reverses_the_rule() {
        CopCase::annotated(
            COP,
            r#"
            x.nil?
              ^^^^ Prefer the use of the `==` comparison.
            "#,
        )
        .config("Style/NilComparison:\n  EnforcedStyle: comparison\n")
        .corrected("x == nil\n")
        .run();
    }
}

/// `Style/SingleArgumentDig`: 引数 1 つの `dig` は `[]`。
///
/// 期待値は本家 1.89.0 の `--only Style/SingleArgumentDig` と `-A` の実測。
mod single_argument_dig {
    use super::*;

    const COP: &str = "Style/SingleArgumentDig";

    #[test]
    fn the_call_is_replaced_by_an_index() {
        expect_correction(COP, "[1, 2, 3].dig(0)\n", "[1, 2, 3][0]\n");
        expect_correction(COP, "{ key: 'v' }.dig(:key)\n", "{ key: 'v' }[:key]\n");
    }

    /// 引数が 2 つ以上、splat、安全参照、受け手なしは対象外。
    #[test]
    fn anything_but_one_plain_argument_is_left_alone() {
        expect_no_offenses(COP, "{ a: { b: 'v' } }.dig(:a, :b)\n");
        expect_no_offenses(COP, "h.dig(*keys)\n");
        expect_no_offenses(COP, "hash&.dig(:key)\n");
        expect_no_offenses(COP, "dig(:key)\n");
    }
}

/// `Style/RedundantFileExtensionInRequire`: `require 'foo.rb'` の `.rb` は不要。
///
/// 期待値は本家 1.89.0 の `--only Style/RedundantFileExtensionInRequire` と `-A` の実測。
mod redundant_file_extension_in_require {
    use super::*;

    const COP: &str = "Style/RedundantFileExtensionInRequire";

    #[test]
    fn the_extension_is_reported_and_removed() {
        expect_correction(COP, "require 'foo.rb'\n", "require 'foo'\n");
        expect_correction(
            COP,
            "require_relative '../foo.rb'\n",
            "require_relative '../foo'\n",
        );
    }

    #[test]
    fn another_extension_or_a_receiver_is_left_alone() {
        expect_no_offenses(COP, "require 'foo.so'\n");
        expect_no_offenses(COP, "require 'foo'\n");
        expect_no_offenses(COP, "Kernel.require 'foo.rb'\n");
        expect_no_offenses(COP, "require \"#{x}.rb\"\n");
    }
}

/// `Style/UnpackFirst`: `unpack(...).first` は `unpack1`。
///
/// 期待値は本家 1.89.0 の `--only Style/UnpackFirst` と `-A` の実測。
mod unpack_first {
    use super::*;

    const COP: &str = "Style/UnpackFirst";

    #[test]
    fn every_way_of_taking_the_first_element_is_reported() {
        expect_correction(COP, "'foo'.unpack('h*').first\n", "'foo'.unpack1('h*')\n");
        expect_correction(COP, "'foo'.unpack('h*')[0]\n", "'foo'.unpack1('h*')\n");
        expect_correction(
            COP,
            "'foo'.unpack('h*').slice(0)\n",
            "'foo'.unpack1('h*')\n",
        );
        expect_correction(COP, "'foo'.unpack('h*').at(0)\n", "'foo'.unpack1('h*')\n");
    }

    #[test]
    fn taking_anything_else_is_left_alone() {
        expect_no_offenses(COP, "'foo'.unpack('h*')\n");
        expect_no_offenses(COP, "'foo'.unpack('h*')[1]\n");
        expect_no_offenses(COP, "'foo'.unpack('h*').last\n");
        expect_no_offenses(COP, "'foo'.unpack1('h*')\n");
    }
}

/// `Style/Dir`: `File.expand_path(File.dirname(__FILE__))` は `__dir__`。
///
/// 期待値は本家 1.89.0 の `--only Style/Dir` と `-A` の実測。
mod dir {
    use super::*;

    const COP: &str = "Style/Dir";

    #[test]
    fn both_spellings_collapse_to_the_keyword() {
        expect_correction(
            COP,
            "path = File.expand_path(File.dirname(__FILE__))\n",
            "path = __dir__\n",
        );
        expect_correction(
            COP,
            "path = File.dirname(File.realpath(__FILE__))\n",
            "path = __dir__\n",
        );
    }

    #[test]
    fn another_path_or_another_order_is_left_alone() {
        expect_no_offenses(COP, "path = File.expand_path(File.dirname(other))\n");
        expect_no_offenses(COP, "path = File.dirname(File.expand_path(__FILE__))\n");
        expect_no_offenses(COP, "path = File.expand_path(__FILE__)\n");
    }
}

/// `Style/Attr`: `attr` ではなく `attr_reader` / `attr_accessor`。
///
/// 期待値は本家 1.89.0 の `--only Style/Attr` と `-A` の実測。
mod attr {
    use super::*;

    const COP: &str = "Style/Attr";

    #[test]
    fn the_trailing_boolean_decides_which_macro_is_meant() {
        expect_correction(
            COP,
            "class K\n  attr :something, true\nend\n",
            "class K\n  attr_accessor :something\nend\n",
        );
        expect_correction(
            COP,
            "class K\n  attr :one, :two, :three\nend\n",
            "class K\n  attr_reader :one, :two, :three\nend\n",
        );
        // `module` は `each_ancestor(:class, :block)` に入らないので、そこも対象。
        expect_correction(
            COP,
            "module M\n  attr :a\nend\n",
            "module M\n  attr_reader :a\nend\n",
        );
    }

    /// 引数のないもの、受け手のあるもの、`attr` を自前で定義しているクラスは対象外。
    #[test]
    fn a_receiver_or_a_locally_defined_attr_is_left_alone() {
        expect_no_offenses(COP, "class K\n  attr\nend\n");
        expect_no_offenses(COP, "class K\n  foo.attr :a\nend\n");
        expect_no_offenses(COP, "class K\n  attr :a\n  def attr(x); end\nend\n");
        expect_no_offenses(COP, "foo do\n  attr :a\nend\n");
    }
}

/// `Style/NestedParenthesizedCalls`: 括弧つき呼び出しの中の引数も括弧をつける。
///
/// 期待値は本家 1.89.0 の `--only Style/NestedParenthesizedCalls` と `-A` の実測。
mod nested_parenthesized_calls {
    use super::*;

    const COP: &str = "Style/NestedParenthesizedCalls";

    #[test]
    fn the_nested_call_gains_parentheses() {
        expect_correction(COP, "method1(method2 arg)\n", "method1(method2(arg))\n");
    }

    /// 既に括弧つきのもの、引数のないもの、演算子、既定の許可メソッドは対象外。
    #[test]
    fn what_the_cop_leaves_alone() {
        expect_no_offenses(COP, "method1(method2(arg))\n");
        expect_no_offenses(COP, "method1(method2)\n");
        expect_no_offenses(COP, "expect(x).to eq foo\n");
        expect_no_offenses(COP, "method1 method2 arg\n");
        // 文法上は block 引数だが、本家の字句解析では二項 `&`。
        expect_no_offenses(COP, "assert_equal(0x8, info.attr&0x8)\n");
    }
}

/// `Style/RedundantSelfAssignment`: 破壊的メソッドの結果を自分に代入し直さない。
///
/// 期待値は本家 1.89.0 の `--only Style/RedundantSelfAssignment` と `-A` の実測。
mod redundant_self_assignment {
    use super::*;

    const COP: &str = "Style/RedundantSelfAssignment";

    #[test]
    fn the_assignment_goes_and_the_call_stays() {
        expect_correction(COP, "args = args.concat(ary)\n", "args.concat(ary)\n");
        expect_correction(COP, "@h = @h.merge!(other)\n", "@h.merge!(other)\n");
        expect_correction(
            COP,
            "obj.list = obj.list.concat(more)\n",
            "obj.list.concat(more)\n",
        );
    }

    #[test]
    fn a_different_receiver_or_method_is_left_alone() {
        expect_no_offenses(COP, "args = foo.concat(ary)\n");
        expect_no_offenses(COP, "args = args.map { |x| x }\n");
        expect_no_offenses(COP, "args.concat(ary)\n");
    }
}

/// `Style/ExpandPathArguments`: `File.expand_path('..', __FILE__)` は `__dir__` で書く。
///
/// 期待値は本家 1.89.0 の `--only Style/ExpandPathArguments` と `-A` の実測。
mod expand_path_arguments {
    use super::*;

    const COP: &str = "Style/ExpandPathArguments";

    #[test]
    fn the_depth_of_the_path_decides_the_replacement() {
        expect_correction(
            COP,
            "File.expand_path('..', __FILE__)\n",
            "File.expand_path(__dir__)\n",
        );
        expect_correction(
            COP,
            "File.expand_path('../..', __FILE__)\n",
            "File.expand_path('..', __dir__)\n",
        );
        expect_correction(
            COP,
            "File.expand_path('.', __FILE__)\n",
            "File.expand_path(__FILE__)\n",
        );
        // 末尾の `/` は `String#split` が落とすので、深さは変わらない。
        expect_correction(
            COP,
            "File.expand_path('../../', __FILE__)\n",
            "File.expand_path('..', __dir__)\n",
        );
    }

    #[test]
    fn the_pathname_spellings_lose_their_parent_call() {
        expect_correction(
            COP,
            "Pathname(__FILE__).parent.expand_path\n",
            "Pathname(__dir__).expand_path\n",
        );
        expect_correction(
            COP,
            "Pathname.new(__FILE__).parent.expand_path\n",
            "Pathname.new(__dir__).expand_path\n",
        );
    }

    #[test]
    fn another_base_directory_is_left_alone() {
        expect_no_offenses(COP, "File.expand_path('..', __dir__)\n");
        expect_no_offenses(COP, "File.expand_path(path, base)\n");
        expect_no_offenses(COP, "Pathname(__dir__).expand_path\n");
    }
}

/// `Style/RedundantSort`: 並べ替えてから端を取るなら `min` / `max`。
///
/// 期待値は本家 1.89.0 の `--only Style/RedundantSort` と `-A` の実測。
mod redundant_sort {
    use super::*;

    const COP: &str = "Style/RedundantSort";

    #[test]
    fn every_way_of_taking_an_end_is_reported() {
        expect_correction(COP, "[2, 1, 3].sort.first\n", "[2, 1, 3].min\n");
        expect_correction(COP, "[2, 1, 3].sort[0]\n", "[2, 1, 3].min\n");
        expect_correction(COP, "[2, 1, 3].sort.at(-1)\n", "[2, 1, 3].max\n");
        expect_correction(COP, "arr.sort_by(&:foo).last\n", "arr.max_by(&:foo)\n");
        expect_correction(
            COP,
            "arr.sort_by { |x| x.foo }.first\n",
            "arr.min_by { |x| x.foo }\n",
        );
    }

    /// 論理演算子が続くときは、演算子を並べ替え呼び出しの直後へ移す。
    #[test]
    fn a_logical_operator_after_it_is_moved() {
        expect_correction(COP, "[2, 1, 3].sort.first && x\n", "[2, 1, 3].min &&  x\n");
    }

    #[test]
    fn taking_anything_but_an_end_is_left_alone() {
        expect_no_offenses(COP, "[2, 1, 3].sort\n");
        expect_no_offenses(COP, "[2, 1, 3].sort[1]\n");
        expect_no_offenses(COP, "[2, 1, 3].min\n");
    }
}

/// `Style/OrAssignment`: 既定値の代入は `||=`。
///
/// 期待値は本家 1.89.0 の `--only Style/OrAssignment` と `-A` の実測。
mod or_assignment {
    use super::*;

    const COP: &str = "Style/OrAssignment";

    #[test]
    fn both_the_ternary_and_the_unless_forms_are_reported() {
        expect_correction(COP, "name = name ? name : 'B'\n", "name ||= 'B'\n");
        expect_correction(COP, "name = 'B' unless name\n", "name ||= 'B'\n");
        // 先に代入されていて初めて条件の `name` が `lvar` になる。
        expect_correction(
            COP,
            "name = nil\nunless name\n  name = 'B'\nend\n",
            "name = nil\nname ||= 'B'\n",
        );
        expect_correction(COP, "@name = @name ? @name : 'B'\n", "@name ||= 'B'\n");
    }

    #[test]
    fn a_different_variable_or_an_else_branch_is_left_alone() {
        expect_no_offenses(COP, "name = other ? other : 'B'\n");
        expect_no_offenses(COP, "name = name ? other : 'B'\n");
        expect_no_offenses(COP, "name ||= 'B'\n");
        // 未代入の名前は本家では `send` なので、パターンに当たらない。
        expect_no_offenses(COP, "unless other\n  other = 'C'\nend\n");
    }
}

/// `Style/EvenOdd`: `% 2 == 0` は `even?`。
///
/// 期待値は本家 1.89.0 の `--only Style/EvenOdd` と `-A` の実測。
mod even_odd {
    use super::*;

    const COP: &str = "Style/EvenOdd";

    #[test]
    fn the_four_combinations_map_onto_the_two_predicates() {
        expect_correction(COP, "x % 2 == 0\n", "x.even?\n");
        expect_correction(COP, "x % 2 != 0\n", "x.odd?\n");
        expect_correction(COP, "x % 2 == 1\n", "x.odd?\n");
        expect_correction(COP, "x % 2 != 1\n", "x.even?\n");
        // 演算子の受け手は括弧で包む。
        expect_correction(COP, "(a * b) % 2 == 0\n", "(a * b).even?\n");
    }

    #[test]
    fn another_divisor_or_another_comparison_is_left_alone() {
        expect_no_offenses(COP, "x % 3 == 0\n");
        expect_no_offenses(COP, "x % 2 > 0\n");
        expect_no_offenses(COP, "x.even?\n");
    }
}

/// `Style/ExponentialNotation`: 既定では仮数が 1 以上 10 未満。
///
/// 期待値は本家 1.89.0 の `--only Style/ExponentialNotation` の実測。
mod exponential_notation {
    use super::*;

    const COP: &str = "Style/ExponentialNotation";

    #[test]
    fn the_scientific_style_wants_a_single_leading_digit() {
        expect_offense(
            COP,
            r#"
            10e6
            ^^^^ Use a mantissa >= 1 and < 10.
            "#,
        );
        expect_no_offenses(COP, "1e7\n");
        expect_no_offenses(COP, "1.17e6\n");
        expect_no_offenses(COP, "3.14\n");
    }

    #[test]
    fn the_integral_style_wants_no_decimal_part() {
        CopCase::annotated(
            COP,
            r#"
            3.2e7
            ^^^^^ Use an integer as mantissa, without trailing zero.
            "#,
        )
        .config("Style/ExponentialNotation:\n  EnforcedStyle: integral\n")
        .correctable(false)
        .run();
    }
}

/// `Style/MixinUsage`: `include` はクラス/モジュールの中で。
///
/// 期待値は本家 1.89.0 の `--only Style/MixinUsage` の実測。
mod mixin_usage {
    use super::*;

    const COP: &str = "Style/MixinUsage";

    #[test]
    fn a_top_level_mixin_is_reported() {
        expect_offense(
            COP,
            r#"
            include M
            ^^^^^^^^^ `include` is used at the top level. Use inside `class` or `module`.
            "#,
        );
        expect_offense(
            COP,
            r#"
            extend M
            ^^^^^^^^ `extend` is used at the top level. Use inside `class` or `module`.
            "#,
        );
    }

    #[test]
    fn a_mixin_inside_a_class_or_module_is_fine() {
        expect_no_offenses(COP, "class C\n  include M\nend\n");
        expect_no_offenses(COP, "module M2\n  extend M\nend\n");
        expect_no_offenses(COP, "obj.include M\n");
        expect_no_offenses(COP, "include foo\n");
    }
}

/// `Style/HashLikeCase`: 1 対 1 対応の `case-when` はハッシュ引き。
///
/// 期待値は本家 1.89.0 の `--only Style/HashLikeCase` の実測。
mod hash_like_case {
    use super::*;

    const COP: &str = "Style/HashLikeCase";

    #[test]
    fn three_literal_branches_are_reported() {
        expect_offense(
            COP,
            r#"
            case country
            ^^^^^^^^^^^^ Consider replacing `case-when` with a hash lookup.
            when 'europe'
              'eu'
            when 'america'
              'us'
            when 'australia'
              'au'
            end
            "#,
        );
    }

    /// 分岐が足りないもの、`else` を持つもの、型が揃わないものは対象外。
    #[test]
    fn what_the_cop_leaves_alone() {
        expect_no_offenses(COP, "case c\nwhen 'a'\n  1\nwhen 'b'\n  2\nend\n");
        expect_no_offenses(
            COP,
            "case c\nwhen 'a'\n  1\nwhen 'b'\n  2\nwhen 'c'\n  3\nelse\n  4\nend\n",
        );
        expect_no_offenses(
            COP,
            "case c\nwhen 'a'\n  1\nwhen 'b'\n  'x'\nwhen 'c'\n  3\nend\n",
        );
        expect_no_offenses(
            COP,
            "case c\nwhen 'a'\n  foo\nwhen 'b'\n  bar\nwhen 'c'\n  baz\nend\n",
        );
    }
}

/// `Layout/ExtraSpacing` は本家がトークン列を 2 個ずつ舐めて実装されているので、
/// レキサのトークン境界そのものが仕様になる。ヒアドキュメント本体の位置、
/// パーセント配列の語間、リテラル内部の空白がいずれも「隣接トークンの隙間」に
/// 化けないことを、本家 1.89.0 の実測を期待値に据えて固定する。
mod layout_extra_spacing {
    use super::*;

    const COP: &str = "Layout/ExtraSpacing";

    #[test]
    fn reports_the_run_of_spaces_minus_the_one_that_stays() {
        expect_offense(
            COP,
            r#"
            x  = 1
             ^ Unnecessary spacing detected.
            "#,
        );
        expect_correction(COP, "x  = 1\n", "x = 1\n");
        expect_no_offenses(COP, "x = 1\n");
    }

    /// ヒアドキュメント本体は本家では**開始トークンの直後**に字句化されるので、
    /// `foo(<<~B,  bar)` の `,` の隣は `bar` であって本体ではない。本体を位置順に
    /// 並べると `<<~B` と `,` が隣り合い、その間の空白を誤って報告してしまう。
    /// 本体の先頭も開始行の改行の**次**から始まる。
    #[test]
    fn a_heredoc_body_is_lexed_where_its_opener_stands() {
        CopCase::new(
            COP,
            concat!(
                "x  = <<~A\n",
                "  hi  there\n",
                "A\n",
                "foo(<<~B,  bar)\n",
                "  body\n",
                "B\n",
                "z = %w[a  b]\n",
                "s = \"a  b\"\n",
                "q = /a  b/\n",
                "w  = 1\n",
                "__END__\n",
                "tail  data\n",
            ),
            vec![
                Annotation::new(1, 2, 1, "Unnecessary spacing detected."),
                Annotation::new(4, 10, 1, "Unnecessary spacing detected."),
                Annotation::new(10, 2, 1, "Unnecessary spacing detected."),
            ],
        )
        .run();
    }

    #[test]
    fn corrects_only_outside_literals_and_stops_at_the_data_section() {
        expect_correction(
            COP,
            concat!(
                "x  = <<~A\n",
                "  hi  there\n",
                "A\n",
                "foo(<<~B,  bar)\n",
                "  body\n",
                "B\n",
                "z = %w[a  b]\n",
                "w  = 1\n",
                "__END__\n",
                "tail  data\n",
            ),
            concat!(
                "x = <<~A\n",
                "  hi  there\n",
                "A\n",
                "foo(<<~B, bar)\n",
                "  body\n",
                "B\n",
                "z = %w[a  b]\n",
                "w = 1\n",
                "__END__\n",
                "tail  data\n",
            ),
        );
    }

    /// 複数行ハッシュのキーと値の間は `Layout/HashAlignment` の担当なので除外される。
    /// 単一行のものは除外されない。中括弧付きは括弧の行も `single_line?` に効く。
    #[test]
    fn the_gap_inside_a_multiline_hash_belongs_to_hash_alignment() {
        CopCase::new(
            COP,
            concat!(
                "h = {\n",
                "  a:   1,\n",
                "  bbb: 2,\n",
                "}\n",
                "g = { a:   1, bbb: 2 }\n",
            ),
            vec![Annotation::new(5, 9, 2, "Unnecessary spacing detected.")],
        )
        .run();
        expect_correction(COP, "g = { a:   1, bbb: 2 }\n", "g = { a: 1, bbb: 2 }\n");
    }

    /// `AllowForAlignment` は上下の行と揃っている空白を見逃す。コメントは
    /// 「隣り合うコメントが同じ桁で始まる」ときだけ揃っていると見なされる。
    #[test]
    fn alignment_with_a_neighbouring_line_is_allowed() {
        CopCase::new(
            COP,
            concat!(
                "a = 1  # one\n",
                "bb = 2 # two\n",
                "foo(1)     # x\n",
                "barbaz(2)  # y\n",
                "c = 3  # z\n",
            ),
            vec![Annotation::new(5, 6, 1, "Unnecessary spacing detected.")],
        )
        .run();
        CopCase::new(
            COP,
            concat!(
                "name      = \"RuboCop\"\n",
                "\n",
                "website  += \"/rubocop/rubocop\" unless cond\n",
                "set_app(\"RuboCop\")\n",
                "website  = \"https://github.com/rubocop/rubocop\"\n",
            ),
            vec![Annotation::new(5, 8, 1, "Unnecessary spacing detected.")],
        )
        .run();
    }

    /// `AllowBeforeTrailingComments` は行末コメントの前だけを見逃す。既定では
    /// 見逃さない。
    #[test]
    fn trailing_comments_need_the_option() {
        CopCase::new(
            COP,
            "object.method(arg)  # this is a comment\n",
            vec![Annotation::new(1, 19, 1, "Unnecessary spacing detected.")],
        )
        .run();
        CopCase::new(COP, "object.method(arg)  # this is a comment\n", Vec::new())
            .config("Layout/ExtraSpacing:\n  AllowBeforeTrailingComments: true\n")
            .run();
    }

    /// `ForceEqualSignAlignment` は同じブロックの `=` を最も右の桁へ揃える。
    /// 空行がブロックを切るので、そこから先は別の並びになる。
    #[test]
    fn force_equal_sign_alignment_moves_the_whole_run() {
        let source = concat!("a = 1\n", "bbb = 2\n", "cc = 3\n", "\n", "dd = 4\n");
        CopCase::new(
            COP,
            source,
            vec![
                Annotation::new(2, 5, 1, "`=` is not aligned with the preceding assignment."),
                Annotation::new(3, 4, 1, "`=` is not aligned with the preceding assignment."),
            ],
        )
        .config("Layout/ExtraSpacing:\n  ForceEqualSignAlignment: true\n")
        .corrected(concat!(
            "a   = 1\n",
            "bbb = 2\n",
            "cc  = 3\n",
            "\n",
            "dd = 4\n"
        ))
        .run();
    }

    /// 1 行に収まる `"` / `'` の文字列は本家では `tSTRING` **1 個**で、区切りと中身に
    /// 割れない。トークンの本文をそのまま隣の行と突き合わせる `aligned_words?` は
    /// 長さで結論が変わるので、割ってしまうと `">= 2.2.4"` が隣の行の `"` と
    /// 一致して揃っていることにされてしまう。ラベルの `:` も名前と 1 トークン
    /// (`tLABEL`) になる。`g:` と `j:` の後ろが咎められないのは、値の `1` が
    /// 上下の行で同じ桁に来ているため。
    #[test]
    fn a_single_line_string_and_a_label_are_each_one_token() {
        CopCase::new(
            COP,
            concat!(
                "s.add_dependency \"nokogiri\", \">= 1.8.5\"\n",
                "s.add_dependency \"rack\",      \">= 2.2.4\"\n",
                "s.add_dependency \"rack-session\", \">= 1.0.1\"\n",
                "f = { g:   1, \"h\":   2 }\n",
                "def m(j:   1, k: 2)\n",
                "end\n",
            ),
            vec![
                Annotation::new(2, 25, 5, "Unnecessary spacing detected."),
                Annotation::new(4, 19, 2, "Unnecessary spacing detected."),
            ],
        )
        .run();
    }

    /// 本家は `processed_source.blank?` のファイルを一切見ない。
    #[test]
    fn a_file_holding_only_comments_is_skipped() {
        expect_no_offenses(COP, "# a  b\n#  c\n");
    }
}

/// `Layout/BlockAlignment` の既定 `either` は「式の先頭」と「`do` の行頭」の
/// どちらでも許す。`do` が複数行引数の継続行にあるときだけ、その行の字下げは
/// 括弧が決めたもので作者の意図ではないので、呼び出し側の行が基準になる。
mod layout_block_alignment {
    use super::*;

    const COP: &str = "Layout/BlockAlignment";

    #[test]
    fn either_style_names_both_targets() {
        expect_offense(
            COP,
            r#"
            foo.bar
              .each do
                baz
                    end
                    ^^^ `end` at 4, 8 is not aligned with `foo.bar` at 1, 0 or `.each do` at 2, 2.
            "#,
        );
        expect_no_offenses(COP, "foo.bar\n  .each do\n    baz\nend\n");
        expect_no_offenses(COP, "foo.bar\n  .each do\n    baz\n  end\n");
    }

    /// 代入の右辺にあるブロックの `end` は変数のほうに揃える。`{ }` も同じ扱い。
    #[test]
    fn an_assignment_takes_the_end_over() {
        CopCase::new(
            COP,
            concat!(
                "x = [1].map do |y|\n",
                "  y\n",
                "    end\n",
                "[1].each { |z|\n",
                "  z\n",
                "    }\n",
            ),
            vec![
                Annotation::new(
                    3,
                    5,
                    3,
                    "`end` at 3, 4 is not aligned with `x = [1].map do |y|` at 1, 0.",
                ),
                Annotation::new(
                    6,
                    5,
                    1,
                    "`}` at 6, 4 is not aligned with `[1].each { |z|` at 4, 0.",
                ),
            ],
        )
        .corrected(concat!(
            "x = [1].map do |y|\n",
            "  y\n",
            "end\n",
            "[1].each { |z|\n",
            "  z\n",
            "}\n",
        ))
        .run();
    }

    /// `do` の行が `(` で開いた継続行なら基準は呼び出し行に戻るので、代替の
    /// 候補が消えてメッセージが 1 つになる。括弧のない引数列では継続行の
    /// 字下げが作者の意図なので、そのまま候補に残る。
    #[test]
    fn a_continuation_line_holding_the_do_is_not_an_alignment_target() {
        CopCase::new(
            COP,
            concat!(
                "q = foo(bar,\n",
                "        baz) do |i|\n",
                "  i\n",
                "   end\n",
                "r = foo bar,\n",
                "        baz do |i|\n",
                "  i\n",
                "   end\n",
            ),
            vec![
                Annotation::new(
                    4,
                    4,
                    3,
                    "`end` at 4, 3 is not aligned with `q = foo(bar,` at 1, 0.",
                ),
                Annotation::new(
                    8,
                    4,
                    3,
                    "`end` at 8, 3 is not aligned with `r = foo bar,` at 5, 0 or `baz do |i|` at 6, 8.",
                ),
            ],
        )
        .corrected(concat!(
            "q = foo(bar,\n",
            "        baz) do |i|\n",
            "  i\n",
            "end\n",
            "r = foo bar,\n",
            "        baz do |i|\n",
            "  i\n",
            "end\n",
        ))
        .run();
        // `)` で閉じた継続行に `do` があるとき、その行頭も `end` の置き場所として
        // 認められる。
        expect_no_offenses(
            COP,
            concat!(
                "q = foo(bar,\n",
                "        baz) do |i|\n",
                "  i\n",
                "        end\n",
            ),
        );
    }

    /// メッセージが名指しするのは `find_lhs_node` が畳んだ左辺。畳まれるのは
    /// `op_asgn` と `masgn` だけで、`||=` / `&&=` は `or_asgn` / `and_asgn` なので
    /// 代入式まるごとが出る。
    #[test]
    fn only_op_asgn_and_masgn_are_reduced_to_their_left_hand_side() {
        CopCase::new(
            COP,
            concat!(
                "@dimensions ||= depth.times.map do |index|\n",
                "  index\n",
                "                  end\n",
                "@plus += foo.map do |i|\n",
                "  i\n",
                "           end\n",
                "a, b = foo.map do |i|\n",
                "  i\n",
                "         end\n",
            ),
            vec![
                Annotation::new(
                    3,
                    19,
                    3,
                    "`end` at 3, 18 is not aligned with `@dimensions ||= depth.times.map do |index|` at 1, 0.",
                ),
                Annotation::new(6, 12, 3, "`end` at 6, 11 is not aligned with `@plus` at 4, 0."),
                Annotation::new(9, 10, 3, "`end` at 9, 9 is not aligned with `a, b` at 7, 0."),
            ],
        )
        .corrected(concat!(
            "@dimensions ||= depth.times.map do |index|\n",
            "  index\n",
            "end\n",
            "@plus += foo.map do |i|\n",
            "  i\n",
            "end\n",
            "a, b = foo.map do |i|\n",
            "  i\n",
            "end\n",
        ))
        .run();
    }

    /// `start_of_block` は `do` の行頭だけ、`start_of_line` は式の行頭だけを許す。
    #[test]
    fn the_two_strict_styles_each_accept_one_target() {
        let source = concat!("foo.bar\n", "  .each do\n", "    baz\n", "  end\n");
        CopCase::new(COP, source, Vec::new())
            .config("Layout/BlockAlignment:\n  EnforcedStyleAlignWith: start_of_block\n")
            .run();
        CopCase::new(
            COP,
            source,
            vec![Annotation::new(
                4,
                3,
                3,
                "`end` at 4, 2 is not aligned with `foo.bar` at 1, 0.",
            )],
        )
        .config("Layout/BlockAlignment:\n  EnforcedStyleAlignWith: start_of_line\n")
        .corrected(concat!("foo.bar\n", "  .each do\n", "    baz\n", "end\n"))
        .run();
    }
}

/// `Style/Sample`: `shuffle` に続く取り出しは `sample` 一本にまとめる。
///
/// 期待値は本家 1.89.0 の `--only Style/Sample` の実測。
mod sample {
    use super::*;

    const COP: &str = "Style/Sample";

    #[test]
    fn every_way_of_taking_one_element_becomes_a_bare_sample() {
        expect_correction(COP, "a.shuffle.first\n", "a.sample\n");
        expect_correction(COP, "a.shuffle.last\n", "a.sample\n");
        expect_correction(COP, "a.shuffle[0]\n", "a.sample\n");
        expect_correction(COP, "a.shuffle[-1]\n", "a.sample\n");
        expect_correction(COP, "a.shuffle.at(0)\n", "a.sample\n");
        expect_correction(COP, "a.shuffle.slice(0)\n", "a.sample\n");
        // A receiverless `shuffle` is still a call, and safe navigation is one too.
        expect_correction(COP, "shuffle.first\n", "sample\n");
        expect_correction(COP, "a&.shuffle&.first\n", "a&.sample\n");
    }

    #[test]
    fn a_countable_index_becomes_the_argument() {
        expect_correction(COP, "a.shuffle.first(3)\n", "a.sample(3)\n");
        expect_correction(COP, "a.shuffle[0, 3]\n", "a.sample(3)\n");
        expect_correction(COP, "a.shuffle[0..2]\n", "a.sample(3)\n");
        expect_correction(COP, "a.shuffle[0...3]\n", "a.sample(3)\n");
        expect_correction(COP, "a.shuffle[0..]\n", "a.sample(1)\n");
        // `shuffle`'s own argument comes after the count.
        expect_correction(
            COP,
            "a.shuffle(random: r).first(3)\n",
            "a.sample(3, random: r)\n",
        );
    }

    #[test]
    fn an_index_sample_has_no_argument_for_is_left_alone() {
        expect_no_offenses(COP, "a.shuffle[2]\n");
        expect_no_offenses(COP, "a.shuffle.at(2)\n");
        expect_no_offenses(COP, "a.shuffle[0..-1]\n");
        expect_no_offenses(COP, "a.shuffle[x]\n");
        // A block on `shuffle` makes its receiver a `block` node, which the pattern never matches.
        expect_no_offenses(COP, "a.shuffle { |x| x }.first\n");
        expect_no_offenses(COP, "a.sample\n");
        // A local variable named `shuffle` is an `lvar`, not a call.
        expect_no_offenses(COP, "shuffle = [1]\nshuffle.first\n");
    }
}

/// `Style/RedundantFreeze`: 凍らせても意味の無い値の `freeze`。
///
/// 期待値は本家 1.89.0 の `--only Style/RedundantFreeze` の実測。
mod redundant_freeze {
    use super::*;

    const COP: &str = "Style/RedundantFreeze";

    #[test]
    fn an_immutable_literal_gains_nothing_from_freezing() {
        expect_correction(COP, "1.freeze\n", "1\n");
        expect_correction(COP, "-1.freeze\n", "-1\n");
        expect_correction(COP, "1i.freeze\n", "1i\n");
        expect_correction(COP, "1r.freeze\n", "1r\n");
        expect_correction(COP, ":sym.freeze\n", ":sym\n");
        expect_correction(COP, ":\"a#{b}\".freeze\n", ":\"a#{b}\"\n");
        expect_correction(COP, "nil.freeze\n", "nil\n");
        expect_correction(COP, "true.freeze\n", "true\n");
        // `(1)` is a `begin` around the literal, which upstream unwraps before it looks.
        expect_correction(COP, "(1).freeze\n", "(1)\n");
    }

    #[test]
    fn an_operation_that_can_only_answer_with_an_immutable_object_counts_too() {
        expect_correction(COP, "(1 + 2).freeze\n", "(1 + 2)\n");
        expect_correction(COP, "(1 << 2).freeze\n", "(1 << 2)\n");
        expect_correction(COP, "(x - 1).freeze\n", "(x - 1)\n");
        expect_correction(COP, "(x == y).freeze\n", "(x == y)\n");
        expect_correction(COP, "x.count.freeze\n", "x.count\n");
        expect_correction(COP, "x.count { }.freeze\n", "x.count { }\n");
        expect_correction(COP, "count.freeze\n", "count\n");
    }

    #[test]
    fn a_mutable_receiver_is_left_alone() {
        expect_no_offenses(COP, "\"s\".freeze\n");
        expect_no_offenses(COP, "[1].freeze\n");
        expect_no_offenses(COP, "{ a: 1 }.freeze\n");
        expect_no_offenses(COP, "(\"a\" + \"b\").freeze\n");
        expect_no_offenses(COP, "([1] + 2).freeze\n");
        expect_no_offenses(COP, "(1 <=> 2).freeze\n");
        expect_no_offenses(COP, "x.map { }.freeze\n");
        // `freeze` reached with safe navigation is a `csend`, which `on_send` never sees.
        expect_no_offenses(COP, "x&.freeze\n");
        expect_no_offenses(COP, "def m\n  count = 1\n  count.freeze\nend\n");
    }

    /// Ruby 3.0 で `regexp` と `range` が凍るようになり、文字列は magic comment 次第。
    #[test]
    fn what_is_already_frozen_depends_on_the_target_version() {
        const MESSAGE: &str = "Do not freeze immutable objects, as freezing them has no effect.";
        let source = "# frozen_string_literal: true\n\"s\".freeze\n/re/.freeze\n(1..2).freeze\n";
        CopCase::new(COP, source, vec![Annotation::new(2, 1, 10, MESSAGE)])
            .config("AllCops:\n  TargetRubyVersion: 2.7\n")
            .run();
        CopCase::new(
            COP,
            source,
            vec![
                Annotation::new(2, 1, 10, MESSAGE),
                Annotation::new(3, 1, 11, MESSAGE),
                Annotation::new(4, 1, 13, MESSAGE),
            ],
        )
        .target_ruby("3.0")
        .run();
    }
}

/// `Style/IfWithSemicolon`: `if x; y; end` は三項演算子か改行に。
///
/// 期待値は本家 1.89.0 の `--only Style/IfWithSemicolon` の実測。
mod if_with_semicolon {
    use super::*;

    const COP: &str = "Style/IfWithSemicolon";

    #[test]
    fn a_one_line_conditional_becomes_a_ternary() {
        expect_correction(COP, "if foo; bar; end\n", "foo ? bar : nil\n");
        expect_correction(COP, "if foo; end\n", "foo ? nil : nil\n");
        expect_correction(COP, "unless foo; bar; end\n", "foo ? nil : bar\n");
        expect_correction(COP, "if foo; bar; else baz end\n", "foo ? bar : baz\n");
        // A call written without parentheses gets them, or the ternary would not parse.
        expect_correction(
            COP,
            "if foo; puts 1; else puts 2; end\n",
            "foo ? puts(1) : puts(2)\n",
        );
        // An assignment used as the condition keeps its parentheses.
        expect_correction(COP, "if x = 1; bar; end\n", "(x = 1) ? bar : nil\n");
    }

    #[test]
    fn a_branch_that_cannot_become_a_ternary_arm_gets_a_newline() {
        expect_offense(
            COP,
            r#"
            if foo; bar; baz; end
            ^^^^^^^^^^^^^^^^^^^^^ Do not use `if foo;` - use a newline instead.
            "#,
        );
        expect_correction(COP, "if foo; bar; baz; end\n", "if foo\n bar; baz; end\n");
        expect_correction(COP, "if foo; return 1; end\n", "if foo\n return 1; end\n");
        expect_correction(
            COP,
            "if foo; a, b = 1, 2; end\n",
            "if foo\n a, b = 1, 2; end\n",
        );
    }

    #[test]
    fn an_elsif_chain_is_written_out_over_several_lines() {
        expect_offense(
            COP,
            r#"
            if foo; bar; elsif baz; qux; end
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Do not use `if foo;` - use `if/else` instead.
            "#,
        );
        expect_correction(
            COP,
            "if foo; bar; elsif baz; qux; end\n",
            "if foo\n  bar\nelsif baz\n  qux\nend\n",
        );
    }

    #[test]
    fn a_conditional_written_with_then_or_a_newline_is_left_alone() {
        expect_no_offenses(COP, "if foo then bar end\n");
        expect_no_offenses(COP, "if foo\n  bar\nend\n");
        expect_no_offenses(COP, "bar if foo\n");
        expect_no_offenses(COP, "foo ? bar : baz\n");
    }
}

/// `Style/MethodDefParentheses`: 既定では引数のある `def` に括弧を要求する。
///
/// 期待値は本家 1.89.0 の `--only Style/MethodDefParentheses` の実測。
mod method_def_parentheses {
    use super::*;

    const COP: &str = "Style/MethodDefParentheses";

    #[test]
    fn parameters_written_without_parentheses_gain_them() {
        expect_offense(
            COP,
            r#"
            def foo a, b
                    ^^^^ Use def with parentheses when there are parameters.
            end
            "#,
        );
        expect_correction(COP, "def foo a, b\nend\n", "def foo(a, b)\nend\n");
        expect_correction(COP, "def self.bar a\nend\n", "def self.bar(a)\nend\n");
        // The whole run of spaces before the parameters becomes the opening parenthesis.
        expect_correction(COP, "def spaced   a\nend\n", "def spaced(a)\nend\n");
        // The grammar folds `a = nil, b = nil` into one node; the span is still the whole list.
        expect_correction(
            COP,
            "def r a = nil, b = nil\nend\n",
            "def r(a = nil, b = nil)\nend\n",
        );
    }

    #[test]
    fn a_definition_that_declares_nothing_is_left_alone() {
        expect_no_offenses(COP, "def qux\nend\n");
        expect_no_offenses(COP, "def n()\nend\n");
        expect_no_offenses(COP, "def baz(a)\nend\n");
    }

    /// `require_no_parentheses` は逆向き。無名引数と endless def は括弧を保つ。
    #[test]
    fn the_opposite_style_takes_the_parentheses_off() {
        CopCase::new(
            COP,
            "def baz(a)\nend\n",
            vec![Annotation::new(1, 8, 3, "Use def without parentheses.")],
        )
        .config("Style/MethodDefParentheses:\n  EnforcedStyle: require_no_parentheses\n")
        .corrected("def baz a\nend\n")
        .run();
        CopCase::new(COP, "def o(...)\n  p(...)\nend\n", Vec::new())
            .config("Style/MethodDefParentheses:\n  EnforcedStyle: require_no_parentheses\n")
            .run();
        CopCase::new(COP, "def foo(a) = a\n", Vec::new())
            .config("Style/MethodDefParentheses:\n  EnforcedStyle: require_no_parentheses\n")
            .target_ruby("3.0")
            .run();
    }
}

/// `Style/For`: 既定では `for` ではなく `each`。
///
/// 期待値は本家 1.89.0 の `--only Style/For` の実測。
mod r#for {
    use super::*;

    const COP: &str = "Style/For";

    #[test]
    fn the_head_of_the_loop_becomes_a_block() {
        expect_correction(
            COP,
            "for n in [1, 2, 3] do\n  puts n\nend\n",
            "[1, 2, 3].each do |n|\n  puts n\nend\n",
        );
        // Without `do` the head stops at the collection.
        expect_correction(
            COP,
            "for a, b in x\n  puts a\nend\n",
            "x.each do |a, b|\n  puts a\nend\n",
        );
        expect_correction(
            COP,
            "for n in x; puts n; end\n",
            "x.each do |n|; puts n; end\n",
        );
        // Safe navigation carries over to the `each`.
        expect_correction(
            COP,
            "for n in a&.b\n  puts n\nend\n",
            "a&.b&.each do |n|\n  puts n\nend\n",
        );
    }

    #[test]
    fn a_collection_that_binds_looser_than_a_call_gets_parentheses() {
        expect_correction(
            COP,
            "for n in 1..3\n  puts n\nend\n",
            "(1..3).each do |n|\n  puts n\nend\n",
        );
        expect_correction(
            COP,
            "for n in a + b\n  puts n\nend\n",
            "(a + b).each do |n|\n  puts n\nend\n",
        );
        expect_correction(
            COP,
            "for n in a and b\n  puts n\nend\n",
            "(a and b).each do |n|\n  puts n\nend\n",
        );
        // A collection already written in parentheses keeps exactly those.
        expect_correction(
            COP,
            "for n in (a + b)\n  puts n\nend\n",
            "(a + b).each do |n|\n  puts n\nend\n",
        );
    }

    #[test]
    fn an_each_block_is_the_preferred_form() {
        expect_no_offenses(COP, "x.each do |n|\n  puts n\nend\n");
    }

    /// `EnforcedStyle: for` は逆向き。1 行の `each` は対象外。
    #[test]
    fn the_opposite_style_turns_a_multiline_each_into_a_for() {
        CopCase::new(
            COP,
            "x.each do |n|\n  puts n\nend\n",
            vec![Annotation::new(1, 1, 13, "Prefer `for` over `each`.")],
        )
        .config("Style/For:\n  EnforcedStyle: for\n")
        .corrected("for n in x do\n  puts n\nend\n")
        .run();
        CopCase::new(COP, "x.each { |n| puts n }\n", Vec::new())
            .config("Style/For:\n  EnforcedStyle: for\n")
            .run();
    }
}

/// `Style/FloatDivision`: 既定では `to_f` は片側だけ。
///
/// 期待値は本家 1.89.0 の `--only Style/FloatDivision` の実測。
mod float_division {
    use super::*;

    const COP: &str = "Style/FloatDivision";

    #[test]
    fn coercing_both_sides_is_one_too_many() {
        expect_correction(COP, "a.to_f / b.to_f\n", "a.to_f / b\n");
        expect_correction(COP, "foo.to_f / bar.baz.to_f\n", "foo.to_f / bar.baz\n");
        expect_no_offenses(COP, "a.to_f / b\n");
        expect_no_offenses(COP, "a / b.to_f\n");
        expect_no_offenses(COP, "1 / 2\n");
    }

    /// 正規表現のマッチ結果は文字列なので、両側の `to_f` が要る。
    #[test]
    fn a_match_result_keeps_both_coercions() {
        expect_no_offenses(COP, "Regexp.last_match(1).to_f / b.to_f\n");
        expect_no_offenses(COP, "$1.to_f / b.to_f\n");
    }

    #[test]
    fn the_other_styles_move_or_replace_the_coercion() {
        CopCase::new(
            COP,
            "a / b.to_f\n",
            vec![Annotation::new(
                1,
                1,
                10,
                "Prefer using `.to_f` on the left side.",
            )],
        )
        .config("Style/FloatDivision:\n  EnforcedStyle: left_coerce\n")
        .corrected("a.to_f / b\n")
        .run();
        CopCase::new(
            COP,
            "a.to_f / b\n",
            vec![Annotation::new(
                1,
                1,
                10,
                "Prefer using `.to_f` on the right side.",
            )],
        )
        .config("Style/FloatDivision:\n  EnforcedStyle: right_coerce\n")
        .corrected("a / b.to_f\n")
        .run();
        CopCase::new(
            COP,
            "a.to_f / b.to_f\n",
            vec![Annotation::new(
                1,
                1,
                15,
                "Prefer using `fdiv` for float divisions.",
            )],
        )
        .config("Style/FloatDivision:\n  EnforcedStyle: fdiv\n")
        .corrected("a.fdiv(b)\n")
        .run();
    }
}

/// `Style/NestedModifier`: 入れ子の修飾子は 1 つの条件にまとめる。
///
/// 期待値は本家 1.89.0 の `--only Style/NestedModifier` の実測。
mod nested_modifier {
    use super::*;

    const COP: &str = "Style/NestedModifier";

    #[test]
    fn two_modifiers_become_one_condition() {
        expect_offense(
            COP,
            r#"
            foo if bar if baz
                ^^ Avoid using nested modifiers.
            "#,
        );
        expect_correction(COP, "foo if bar if baz\n", "foo if baz && bar\n");
        // A mismatched pair of keywords negates the inner condition.
        expect_correction(COP, "foo if bar unless baz\n", "foo unless baz || !bar\n");
        expect_correction(COP, "foo unless bar if baz\n", "foo if baz && !bar\n");
        // An `or` on either side keeps its own parentheses.
        expect_correction(COP, "foo if a || b if c\n", "foo if c && (a || b)\n");
        expect_correction(COP, "foo if a if b || c\n", "foo if (b || c) && a\n");
        expect_correction(COP, "foo if a == b if c\n", "foo if c && (a == b)\n");
        // A call written without parentheses gets them.
        expect_correction(COP, "foo if puts 1 if c\n", "foo if c && puts(1)\n");
        expect_correction(COP, "foo if x.y 1, 2 if c\n", "foo if c && x.y(1, 2)\n");
    }

    /// 3 重でも報告は 1 件。内側は無視される。
    #[test]
    fn only_the_outermost_pair_is_reported() {
        expect_offense(
            COP,
            r#"
            foo if a if b if c
                     ^^ Avoid using nested modifiers.
            "#,
        );
    }

    /// ループ同士は結合できないので、報告だけして書き換えない。
    #[test]
    fn a_loop_is_reported_but_not_rewritten() {
        CopCase::annotated(
            COP,
            r#"
            foo while bar if baz
                ^^^^^ Avoid using nested modifiers.
            "#,
        )
        .correctable(false)
        .run();
        expect_no_offenses(COP, "foo if bar\n");
    }
}

/// `Style/ParenthesesAroundCondition`: 条件を括弧で囲まない。
///
/// 期待値は本家 1.89.0 の `--only Style/ParenthesesAroundCondition` の実測。
mod parentheses_around_condition {
    use super::*;

    const COP: &str = "Style/ParenthesesAroundCondition";

    #[test]
    fn each_keyword_names_itself_in_the_message() {
        expect_offense(
            COP,
            r#"
            if (foo)
               ^^^^^ Don't use parentheses around the condition of an `if`.
              bar
            end
            "#,
        );
        expect_correction(COP, "if (foo)\n  bar\nend\n", "if foo\n  bar\nend\n");
        expect_correction(
            COP,
            "unless (foo)\n  bar\nend\n",
            "unless foo\n  bar\nend\n",
        );
        expect_correction(COP, "while (foo)\n  bar\nend\n", "while foo\n  bar\nend\n");
        expect_correction(COP, "until (foo)\n  bar\nend\n", "until foo\n  bar\nend\n");
        expect_correction(COP, "foo if (bar)\n", "foo if bar\n");
        expect_correction(
            COP,
            "a\nif b\n  c\nelsif (d)\n  e\nend\n",
            "a\nif b\n  c\nelsif d\n  e\nend\n",
        );
    }

    #[test]
    fn parentheses_that_carry_meaning_are_left_alone() {
        // A letter written against the parenthesis makes it a call rather than a grouping.
        expect_no_offenses(COP, "if(foo)\n  bar\nend\n");
        // A parenthesized assignment says the assignment was meant.
        expect_no_offenses(COP, "if (x = 1)\n  bar\nend\n");
        expect_no_offenses(COP, "if (x; y)\n  bar\nend\n");
        expect_no_offenses(COP, "if (bar if baz)\n  x\nend\n");
        // A `do ... end` block would attach to the loop without them.
        expect_no_offenses(COP, "while (x.each do |y| end)\n  bar\nend\n");
        expect_no_offenses(COP, "if foo\n  bar\nend\n");
        expect_no_offenses(COP, "x = (foo)\n");
    }

    /// 既定では複数行の条件も対象。`AllowInMultilineConditions` で外れる。
    #[test]
    fn a_multiline_condition_is_reported_unless_it_is_allowed() {
        expect_correction(COP, "if (\n  foo\n)\n  bar\nend\n", "if foo\n  bar\nend\n");
        CopCase::new(COP, "if (\n  foo\n)\n  bar\nend\n", Vec::new())
            .config("Style/ParenthesesAroundCondition:\n  AllowInMultilineConditions: true\n")
            .run();
    }
}

/// `Style/Encoding`: UTF-8 は既定なので encoding コメントは不要。
///
/// 期待値は本家 1.89.0 の `--only Style/Encoding` の実測。
mod encoding {
    use super::*;

    const COP: &str = "Style/Encoding";

    #[test]
    fn a_utf8_encoding_comment_takes_its_line_with_it() {
        expect_offense(
            COP,
            r#"
            # encoding: utf-8
            ^^^^^^^^^^^^^^^^^ Unnecessary utf-8 encoding comment.
            puts 1
            "#,
        );
        expect_correction(COP, "# encoding: utf-8\nputs 1\n", "puts 1\n");
        expect_correction(COP, "# coding: UTF-8\nputs 1\n", "puts 1\n");
        expect_correction(COP, "# -*- coding: utf-8 -*-\nputs 1\n", "puts 1\n");
        // The blank lines the comment was followed by go with it.
        expect_correction(COP, "# encoding: utf-8\n\n\nputs 1\n", "puts 1\n");
    }

    #[test]
    fn a_comment_that_sets_something_else_too_keeps_the_rest() {
        expect_correction(
            COP,
            "# -*- coding: utf-8; frozen_string_literal: true -*-\nputs 1\n",
            "# -*- frozen_string_literal: true -*-\nputs 1\n",
        );
        expect_correction(
            COP,
            "# vim: filetype=ruby, fileencoding=utf-8\nputs 1\n",
            "# vim: filetype=ruby\nputs 1\n",
        );
    }

    #[test]
    fn the_search_stops_at_the_first_line_that_is_not_a_magic_comment() {
        // A shebang is stepped over rather than ending the run.
        expect_correction(
            COP,
            "#!/usr/bin/env ruby\n# encoding: utf-8\nputs 1\n",
            "#!/usr/bin/env ruby\nputs 1\n",
        );
        expect_no_offenses(COP, "puts 1\n# encoding: utf-8\n");
        expect_no_offenses(COP, "  # encoding: utf-8\nputs 1\n");
        expect_no_offenses(COP, "# encoding: ascii-8bit\nputs 1\n");
        // Vim honours `fileencoding` only next to another setting separated by `, `.
        expect_no_offenses(COP, "# vim: filetype=ruby,fileencoding=utf-8\nputs 1\n");
        expect_no_offenses(COP, "");
    }
}

/// `Style/EachWithObject`: 空の入れ物を畳み込む `inject` は `each_with_object`。
///
/// 期待値は本家 1.89.0 の `--only Style/EachWithObject` の実測。
mod each_with_object {
    use super::*;

    const COP: &str = "Style/EachWithObject";

    #[test]
    fn a_fold_that_hands_its_accumulator_back_is_an_each_with_object() {
        expect_offense(
            COP,
            r#"
            [1, 2].inject({}) do |h, i|
                   ^^^^^^ Use `each_with_object` instead of `inject`.
              h[i] = i
              h
            end
            "#,
        );
        expect_correction(
            COP,
            "[1, 2].inject({}) do |h, i|\n  h[i] = i\n  h\nend\n",
            "[1, 2].each_with_object({}) do |i, h|\n  h[i] = i\nend\n",
        );
        // On one line the accumulator alone is removed, not the line it sits on.
        expect_correction(
            COP,
            "[1, 2].reduce({}) { |h, i| h[i] = i; h }\n",
            "[1, 2].each_with_object({}) { |i, h| h[i] = i;  }\n",
        );
    }

    #[test]
    fn a_numbered_block_swaps_its_parameters_instead() {
        expect_correction(
            COP,
            "[1, 2].inject({}) do\n  _1[_2] = _2\n  _1\nend\n",
            "[1, 2].each_with_object({}) do\n  _2[_1] = _1\n  _2\nend\n",
        );
    }

    #[test]
    fn a_fold_that_computes_a_value_is_left_alone() {
        // A basic literal seed means the block folds rather than fills in.
        expect_no_offenses(
            COP,
            "[1, 2].inject(0) do |sum, i|\n  sum += i\n  sum\nend\n",
        );
        expect_no_offenses(COP, "[1, 2].inject(:+)\n");
        // Reassigning the accumulator is not the same as filling one in.
        expect_no_offenses(
            COP,
            "[1, 2].inject({}) do |h, i|\n  h = h.merge(i => i)\n  h\nend\n",
        );
        expect_no_offenses(
            COP,
            "[1, 2].inject({}) do |h, i|\n  h[i] = i\n  h[i]\nend\n",
        );
        expect_no_offenses(COP, "[1, 2].inject { |a, b| a }\n");
        expect_no_offenses(COP, "[1, 2].inject({}) do |h, i, j|\n  h\nend\n");
    }
}

/// `Style/HashTransformKeys` / `Style/HashTransformValues`: ハッシュの片側だけを書き換える畳み込み。
///
/// 期待値は本家 1.89.0 の `--only <cop>` の実測。
mod hash_transform {
    use super::*;

    const KEYS: &str = "Style/HashTransformKeys";
    const VALUES: &str = "Style/HashTransformValues";

    #[test]
    fn the_four_shapes_all_become_one_call() {
        expect_correction(
            KEYS,
            "{a: 1}.each_with_object({}) { |(k, v), h| h[k.to_s] = v }\n",
            "{a: 1}.transform_keys { |k| k.to_s }\n",
        );
        expect_correction(
            KEYS,
            "Hash[{a: 1}.map { |k, v| [k.to_s, v] }]\n",
            "{a: 1}.transform_keys { |k| k.to_s }\n",
        );
        expect_correction(
            KEYS,
            "{a: 1}.map { |k, v| [k.to_s, v] }.to_h\n",
            "{a: 1}.transform_keys { |k| k.to_s }\n",
        );
        expect_correction(
            KEYS,
            "{a: 1}.to_h { |k, v| [k.to_s, v] }\n",
            "{a: 1}.transform_keys { |k| k.to_s }\n",
        );
        expect_correction(
            VALUES,
            "{a: 1}.map { |k, v| [k, v.to_s] }.to_h\n",
            "{a: 1}.transform_values { |v| v.to_s }\n",
        );
    }

    /// 受け手がハッシュだと分かるものに限る。片方だけを書き換えていることも必要。
    #[test]
    fn anything_that_is_not_plainly_a_hash_rewrite_is_left_alone() {
        expect_no_offenses(KEYS, "x.map { |k, v| [k.to_s, v] }.to_h\n");
        expect_no_offenses(
            KEYS,
            "x.each_with_object({}) { |(k, v), h| h[k.to_s] = v }\n",
        );
        expect_no_offenses(KEYS, "{a: 1}.map { |k, v| [k, v] }.to_h\n");
        expect_no_offenses(KEYS, "{a: 1}.map { |k, v| [v, k] }.to_h\n");
        expect_no_offenses(KEYS, "{a: 1}.map { |k, v| [foo(k, v), v] }.to_h\n");
        expect_no_offenses(VALUES, "{a: 1}.map { |k, v| [k.to_s, v] }.to_h\n");
        // A hash-producing call with a block and no arguments counts as a hash receiver.
        expect_offense(
            KEYS,
            r#"
            {a: 1}.group_by { |x| x }.map { |k, v| [k.to_s, v] }.to_h
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Prefer `transform_keys` over `map {...}.to_h`.
            "#,
        );
        expect_no_offenses(
            KEYS,
            "{a: 1}.transform_keys(&:to_s).map { |k, v| [k.to_s, v] }.to_h\n",
        );
    }
}

/// `Style/AndOr`: 既定では条件の中の `and` / `or` だけを見る。
///
/// 期待値は本家 1.89.0 の `--only Style/AndOr` の実測。
mod and_or {
    use super::*;

    const COP: &str = "Style/AndOr";

    #[test]
    fn only_a_condition_is_looked_at_by_default() {
        expect_offense(
            COP,
            r#"
            if foo and bar
                   ^^^ Use `&&` instead of `and`.
              x
            end
            "#,
        );
        expect_correction(
            COP,
            "if foo and bar\n  x\nend\n",
            "if foo && bar\n  x\nend\n",
        );
        expect_correction(
            COP,
            "while foo or bar\n  x\nend\n",
            "while foo || bar\n  x\nend\n",
        );
        expect_correction(COP, "puts 1 if foo and bar\n", "puts 1 if foo && bar\n");
        expect_correction(COP, "(foo and bar) ? 1 : 2\n", "(foo && bar) ? 1 : 2\n");
        expect_no_offenses(COP, "if foo && bar\n  x\nend\n");
        // Outside a condition the semantic operator is left alone.
        expect_no_offenses(COP, "x = foo and bar\n");
        expect_no_offenses(COP, "foo.save and return\n");
    }

    /// 演算子の優先順位が変わる分は括弧で補う。
    #[test]
    fn what_changes_meaning_under_the_tighter_operator_gains_parentheses() {
        expect_correction(
            COP,
            "if foo.include? 1 and bar\n  x\nend\n",
            "if foo.include?(1) && bar\n  x\nend\n",
        );
        expect_correction(
            COP,
            "if a.is_a?String and b\n  x\nend\n",
            "if a.is_a?(String) && b\n  x\nend\n",
        );
        expect_correction(
            COP,
            "if not foo and bar\n  x\nend\n",
            "if (not foo) && bar\n  x\nend\n",
        );
        expect_correction(
            COP,
            "if a == b and c\n  x\nend\n",
            "if (a == b) && c\n  x\nend\n",
        );
        expect_correction(
            COP,
            "if a.b = 1 and c\n  x\nend\n",
            "if (a.b = 1) && c\n  x\nend\n",
        );
        expect_correction(
            COP,
            "if a and b || c\n  x\nend\n",
            "if a && (b || c)\n  x\nend\n",
        );
        // An indexing and a parenthesized call are left as they are.
        expect_correction(COP, "if a[0] and b\n  x\nend\n", "if a[0] && b\n  x\nend\n");
        expect_correction(
            COP,
            "if foo(1) and bar\n  x\nend\n",
            "if foo(1) && bar\n  x\nend\n",
        );
    }

    /// `EnforcedStyle: always` は条件の外も見る。
    #[test]
    fn the_always_style_looks_everywhere() {
        CopCase::new(
            COP,
            "x = foo and bar\n",
            vec![Annotation::new(1, 9, 3, "Use `&&` instead of `and`.")],
        )
        .config("Style/AndOr:\n  EnforcedStyle: always\n")
        .corrected("(x = foo) && bar\n")
        .run();
    }
}

/// `Style/TrivialAccessors`: 単純な読み書きは `attr_*` で。
///
/// 期待値は本家 1.89.0 の `--only Style/TrivialAccessors` の実測。
mod trivial_accessors {
    use super::*;

    const COP: &str = "Style/TrivialAccessors";

    #[test]
    fn a_reader_and_a_writer_become_attr_declarations() {
        expect_correction(
            COP,
            "class C\n  def foo\n    @foo\n  end\nend\n",
            "class C\n  attr_reader :foo\nend\n",
        );
        expect_correction(
            COP,
            "class C\n  def bar=(val)\n    @bar = val\n  end\nend\n",
            "class C\n  attr_writer :bar\nend\n",
        );
        // A class method is rewritten into a singleton class body.
        expect_correction(
            COP,
            "class C\n  def self.cls\n    @cls\n  end\nend\n",
            "class C\n  class << self\n    attr_reader :cls\n  end\nend\n",
        );
    }

    #[test]
    fn what_an_accessor_could_not_replace_is_left_alone() {
        // `ExactNameMatch` wants the names to agree.
        expect_no_offenses(COP, "class C\n  def other\n    @different\n  end\nend\n");
        // `AllowPredicates` and the allowed names.
        expect_no_offenses(COP, "class C\n  def qux?\n    @qux\n  end\nend\n");
        expect_no_offenses(COP, "class C\n  def to_s\n    @to_s\n  end\nend\n");
        expect_no_offenses(
            COP,
            "class C\n  def initialize\n    @initialize\n  end\nend\n",
        );
        // `AllowDSLWriters` allows a writer whose name does not end in `=`.
        expect_no_offenses(COP, "class C\n  def baz(val)\n    @baz = val\n  end\nend\n");
        expect_no_offenses(COP, "class C\n  def m\n    @a\n    @b\n  end\nend\n");
        // A definition inside a module reads as a mixin rather than as an attribute.
        expect_no_offenses(COP, "module M\n  def foo\n    @foo\n  end\nend\n");
        expect_no_offenses(COP, "def top\n  @top\nend\n");
    }
}

/// `Style/ExplicitBlockArgument`: `yield` を渡すだけのブロックは `&block` に。
///
/// 期待値は本家 1.89.0 の `--only Style/ExplicitBlockArgument` の実測。
mod explicit_block_argument {
    use super::*;

    const COP: &str = "Style/ExplicitBlockArgument";

    #[test]
    fn a_block_that_only_yields_becomes_a_block_argument() {
        expect_correction(
            COP,
            "def foo\n  bar { yield }\nend\n",
            "def foo(&block)\n  bar(&block)\nend\n",
        );
        expect_correction(
            COP,
            "def foo\n  bar { |x| yield x }\nend\n",
            "def foo(&block)\n  bar(&block)\nend\n",
        );
        expect_correction(
            COP,
            "def foo(a)\n  bar { |x| yield x }\nend\n",
            "def foo(a, &block)\n  bar(&block)\nend\n",
        );
        // A block argument already declared keeps its own name.
        expect_correction(
            COP,
            "def foo(&blk)\n  bar { |x| yield x }\nend\n",
            "def foo(&blk)\n  bar(&blk)\nend\n",
        );
        // The definition only gains the parameter once however many blocks it holds.
        expect_correction(
            COP,
            "def foo\n  bar { |x| yield x }\n  baz { |y| yield y }\nend\n",
            "def foo(&block)\n  bar(&block)\n  baz(&block)\nend\n",
        );
    }

    #[test]
    fn a_block_that_does_anything_else_is_left_alone() {
        expect_no_offenses(COP, "def foo\n  bar { |x| yield y }\nend\n");
        expect_no_offenses(COP, "def foo\n  bar { |x, y| yield x }\nend\n");
        expect_no_offenses(COP, "def foo\n  bar { |x| yield x, y }\nend\n");
        expect_no_offenses(COP, "def foo\n  bar { |x| puts x; yield x }\nend\n");
        // A `yield` outside any method definition has no signature to move to.
        expect_no_offenses(COP, "bar { |x| yield x }\n");
    }
}
