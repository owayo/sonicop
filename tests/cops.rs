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

    #[test]
    fn syntax_accepts_valid_source() {
        expect_no_offenses("Lint/Syntax", "puts 1\n");
    }

    #[test]
    fn unused_block_argument_accepts_a_referenced_argument() {
        expect_no_offenses("Lint/UnusedBlockArgument", "[1].each { |x| puts x }\n");
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

    #[test]
    fn semicolon_accepts_separate_lines() {
        expect_no_offenses("Style/Semicolon", "puts 1\nputs 2\n");
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
