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
