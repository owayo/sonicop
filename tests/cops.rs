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
        .locations(&[(1, 6, 1, 6)])
        .run();
    }

    /// 余分な空行があるときは、余った 1 行目を指す。文言も本家の
    /// 「N trailing blank lines detected.」に揃える。
    #[test]
    fn trailing_empty_lines_counts_the_extra_blank_lines() {
        CopCase::new(
            "Layout/TrailingEmptyLines",
            "x = 1\n\n",
            vec![Annotation::new(2, 1, 1, "1 trailing blank lines detected.")],
        )
        .locations(&[(2, 1, 2, 1)])
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
