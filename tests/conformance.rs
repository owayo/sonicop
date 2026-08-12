//! 本家 RuboCop 1.89.0 との一致を測るゲート。
//!
//! 期待値はすべて本家の `--only <cop> --format json` の実測から取っている。
//! sonicop の出力を写すと既存のバグを仕様として焼き付けるため、突合には
//! `scratchpad/ab_cops.py` を使う。
//!
//! 差分は `#[ignore]` で退避せず、[`support::manifest`] のマニフェストへ
//! データとして登録する。ケースは常に実行され、結果とマニフェストを
//! 突き合わせて判定する。**直ったのにエントリが残っている場合も失敗する**ので、
//! 修正がマニフェストの掃除を強制する。
//!
//! ```text
//! cargo test --test conformance                     # ゲート
//! SONICOP_CONFORMANCE_MD=/tmp/CONFORMANCE.md \
//!   cargo test --test conformance -- --ignored generates  # レポート生成
//! ```

mod support;

use std::collections::{BTreeMap, BTreeSet};

use sonicop::diagnostic::Severity;
use sonicop::rules::rule_names;
use support::case::CopCase;
use support::divergence::Kind;
use support::manifest::{Entry, Manifest};

/// 本家の実出力を期待値としたケース一覧。ケース ID はマニフェストの
/// 突き合わせキーなので、一度付けたら変えないこと。
fn catalogue() -> Vec<CopCase> {
    vec![
        // ---- Bundler ----
        // 部門ごと `Include` を持つので、対象になるのは Gemfile 系のファイルだけ。
        CopCase::annotated(
            "Bundler/DuplicatedGem",
            "gem 'a'\ngem \"a\"\n^^^^^^^ Gem `a` requirements already given on line 1 of the Gemfile.\n",
        )
        .id("bundler_duplicated_gem")
        .path("Gemfile")
        .severity(Severity::Warning)
        .correctable(false),
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
            "#,
        )
        .id("bundler_duplicated_group")
        .path("Gemfile")
        .severity(Severity::Warning),
        // `add_global_offense` はファイル先頭の長さ 0 のレンジ。メッセージには本家が
        // 検査前に絶対化したパスがそのまま入る。
        CopCase::annotated(
            "Bundler/GemFilename",
            "gem 'a'\n^{} `gems.rb` file was found but `Gemfile` is required (file path: /tmp/example/gems.rb).\n",
        )
        .id("bundler_gem_filename")
        .path("/tmp/example/gems.rb")
        .locations(&[(1, 1, 1, 1)])
        .lengths(&[0])
        .correctable(false),
        CopCase::annotated(
            "Bundler/InsecureProtocolSource",
            r#"
            source :rubygems
                   ^^^^^^^^^ The source `:rubygems` is deprecated because HTTP requests are insecure. Please change your source to 'https://rubygems.org' if possible, or 'http://rubygems.org' if not.
            "#,
        )
        .id("bundler_insecure_protocol_source")
        .path("Gemfile")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Bundler/OrderedGems",
            r#"
            gem 'rubocop'
            gem 'rspec'
            ^^^^^^^^^^^ Gems should be sorted in an alphabetical order within their section of the Gemfile. Gem `rspec` should appear before `rubocop`.
            "#,
        )
        .id("bundler_ordered_gems")
        .path("Gemfile")
        .correctable(true),
        // ---- Gemspec ----
        CopCase::annotated(
            "Gemspec/DuplicatedAssignment",
            r#"
            Gem::Specification.new do |spec|
              spec.name = 'x'
              spec.name = 'y'
              ^^^^^^^^^^^^^^^ `name=` method calls already given on line 2 of the gemspec.
            end
            "#,
        )
        .id("gemspec_duplicated_assignment")
        .path("example.gemspec")
        .severity(Severity::Warning)
        .correctable(false),
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
        .id("gemspec_ordered_dependencies")
        .path("example.gemspec")
        .correctable(true),
        // `add_global_offense`。宣言が 1 つも無いこと自体が offense なので、指す構文が無い。
        CopCase::annotated(
            "Gemspec/RequiredRubyVersion",
            r#"
            Gem::Specification.new do |spec|
            ^{} `required_ruby_version` should be specified.
              spec.name = 'x'
            end
            "#,
        )
        .id("gemspec_required_ruby_version_missing")
        .path("example.gemspec")
        .severity(Severity::Warning)
        .locations(&[(1, 1, 1, 1)])
        .lengths(&[0]),
        CopCase::annotated(
            "Gemspec/RubyVersionGlobalsUsage",
            r#"
            RUBY_VERSION
            ^^^^^^^^^^^^ Do not use `RUBY_VERSION` in gemspec file.
            "#,
        )
        .id("gemspec_ruby_version_globals_usage")
        .path("example.gemspec")
        .severity(Severity::Warning)
        .correctable(false),
        // ---- Layout ----
        CopCase::annotated(
            "Layout/ArgumentAlignment",
            r#"
            foo :bar,
              :baz
              ^^^^ Align the arguments of a method call if they span more than one line.
            "#,
        )
        .id("layout_argument_alignment")
        .locations(&[(2, 3, 2, 6)])
        .correctable(true),
        CopCase::annotated(
            "Layout/HashAlignment",
            r#"
            x = {
              a: 1,
               b: 2,
               ^^^^ Align the keys of a hash literal if they span more than one line.
            }
            "#,
        )
        .id("layout_hash_alignment")
        .locations(&[(3, 4, 3, 7)])
        .correctable(true),
        CopCase::annotated(
            "Layout/ArrayAlignment",
            r#"
            x = [1,
              2]
              ^ Align the elements of an array literal if they span more than one line.
            "#,
        )
        .id("layout_array_alignment")
        .locations(&[(2, 3, 2, 3)])
        .correctable(true),
        CopCase::annotated(
            "Layout/EmptyLineAfterGuardClause",
            r#"
            def foo
              return if a
              ^^^^^^^^^^^ Add empty line after guard clause.
              bar
            end
            "#,
        )
        .id("layout_empty_line_after_guard_clause")
        .locations(&[(2, 3, 2, 13)])
        .correctable(true),
        CopCase::annotated(
            "Layout/EmptyLinesAroundAccessModifier",
            r#"
            class Foo
              def a; end
              private
              ^^^^^^^ Keep a blank line before and after `private`.
              def b; end
            end
            "#,
        )
        .id("layout_empty_lines_around_access_modifier")
        .locations(&[(3, 3, 3, 9)])
        .correctable(true),
        CopCase::annotated(
            "Layout/FirstArrayElementIndentation",
            r#"
            y = [
                1,
                ^ Use 2 spaces for indentation in an array, relative to the start of the line where the left square bracket is.
              ]
              ^ Indent the right bracket the same as the start of the line where the left bracket is.
            "#,
        )
        .id("layout_first_array_element_indentation")
        .locations(&[(2, 5, 2, 5), (3, 3, 3, 3)])
        .correctable(true),
        CopCase::annotated(
            "Layout/FirstHashElementIndentation",
            r#"
            x = {
                a: 1,
                ^^^^ Use 2 spaces for indentation in a hash, relative to the start of the line where the left curly brace is.
              }
              ^ Indent the right brace the same as the start of the line where the left brace is.
            "#,
        )
        .id("layout_first_hash_element_indentation")
        .locations(&[(2, 5, 2, 8), (3, 3, 3, 3)])
        .correctable(true),
        CopCase::annotated(
            "Layout/EmptyLineAfterMagicComment",
            r#"
            # encoding: utf-8
            puts 1
            ^ Add an empty line after magic comments.
            "#,
        )
        .id("layout_empty_line_after_magic_comment")
        .locations(&[(2, 1, 2, 1)])
        .correctable(true),
        CopCase::annotated(
            "Layout/EndOfLine",
            "x = 1\r\n^^^^^ Carriage return character detected.\n",
        )
        .id("layout_end_of_line_crlf")
        .config("Layout/EndOfLine:\n  EnforcedStyle: lf\n")
        .locations(&[(1, 1, 2, 1)])
        .correctable(false),
        CopCase::annotated(
            "Layout/IndentationConsistency",
            r#"
            class Foo
              def a
              end
                def b
                ^^^^^ Inconsistent indentation detected.
                end
            end
            "#,
        )
        .id("layout_indentation_consistency")
        .locations(&[(4, 5, 5, 7)])
        .lengths(&[13])
        .correctable(true),
        CopCase::annotated(
            "Layout/IndentationWidth",
            r#"
            def foo
                bar
            ^^^^ Use 2 (not 4) spaces for indentation.
            end
            "#,
        )
        .id("layout_indentation_width")
        .locations(&[(2, 1, 2, 4)])
        .correctable(true),
        CopCase::annotated(
            "Layout/LineLength",
            r#"
            x = 1234567890
                      ^^^^ Line is too long. [14/10]
            "#,
        )
        .id("layout_line_length_ascii")
        .config("Layout/LineLength:\n  Max: 10\n")
        .lengths(&[4]),
        // 本家は `String#length` で数えるので全角も 1 文字。表示幅で数えると
        // 全角を含む行だけが早く上限を超え、本家が見逃す行を報告してしまう。
        CopCase::annotated(
            "Layout/LineLength",
            r#"
            # あああ x
               ^^^^ Line is too long. [7/3]
            "#,
        )
        .id("layout_line_length_multibyte")
        .config("Layout/LineLength:\n  Max: 3\n")
        .locations(&[(1, 4, 1, 7)])
        .lengths(&[4]),
        CopCase::annotated(
            "Layout/SpaceAfterComma",
            r#"
            [1,2]
              ^ Space missing after comma.
            "#,
        )
        .id("layout_space_after_comma"),
        CopCase::annotated(
            "Layout/SpaceAroundOperators",
            r#"
            1+2
             ^ Surrounding space missing for operator `+`.
            "#,
        )
        .id("layout_space_around_operators"),
        CopCase::annotated(
            "Layout/SpaceInsideArrayLiteralBrackets",
            r#"
            [ 1]
             ^ Do not use space inside array brackets.
            "#,
        )
        .id("layout_space_inside_array_literal_brackets")
        .locations(&[(1, 2, 1, 2)])
        .correctable(true),
        // 空でない波括弧は開き側・閉じ側を別々に見る。空の波括弧はさらに別扱いで、
        // 既定の `EnforcedStyleForEmptyBraces: no_space` が中の空白を咎める。
        CopCase::annotated(
            "Layout/SpaceInsideBlockBraces",
            r#"
            each {|x| x }
                 ^^ Space between { and | missing.
            each { |x| x}
                        ^ Space missing inside }.
            each {}
            each {  }
                  ^^ Space inside empty braces detected.
            "#,
        )
        .id("layout_space_inside_block_braces")
        .locations(&[(1, 6, 1, 7), (2, 13, 2, 13), (4, 7, 4, 8)])
        .lengths(&[2, 1, 2])
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceInsideParens",
            r#"
            puts( 1)
                 ^ Space inside parentheses detected.
            "#,
        )
        .id("layout_space_inside_parens"),
        CopCase::annotated(
            "Layout/SpaceInsidePercentLiteralDelimiters",
            r#"
            %w( a)
               ^ Do not use spaces inside percent literal delimiters.
            "#,
        )
        .id("layout_space_inside_percent_literal_delimiters")
        .locations(&[(1, 4, 1, 4)])
        .correctable(true),
        CopCase::annotated(
            "Layout/TrailingEmptyLines",
            r#"
            x = 1
                 ^{} Final newline missing.
            "#,
        )
        .id("layout_trailing_empty_lines_missing")
        .chomp()
        .locations(&[(1, 6, 1, 5)]),
        CopCase::annotated(
            "Layout/TrailingEmptyLines",
            "x = 1\n\n^{} 1 trailing blank lines detected.\n",
        )
        .id("layout_trailing_empty_lines_extra")
        .locations(&[(2, 1, 3, 1)]),
        CopCase::annotated(
            "Layout/TrailingWhitespace",
            "x = 1  \n     ^^ Trailing whitespace detected.\n",
        )
        .id("layout_trailing_whitespace")
        .locations(&[(1, 6, 1, 7)])
        .correctable(true),
        CopCase::annotated(
            "Layout/EmptyLines",
            "a = 1\n\n\n^{} Extra blank line detected.\nb = 2\n",
        )
        .id("layout_empty_lines")
        .locations(&[(3, 1, 4, 1)])
        .lengths(&[1])
        .correctable(true),
        CopCase::annotated(
            "Layout/EmptyLineBetweenDefs",
            r#"
            def a
            end
            def b
            ^^^^^ Expected 1 empty line between method definitions; found 0.
            end
            "#,
        )
        .id("layout_empty_line_between_defs")
        .locations(&[(3, 1, 3, 5)])
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceInLambdaLiteral",
            r#"
            f = -> (x) { x }
                  ^ Do not use spaces between `->` and `(` in lambda literals.
            "#,
        )
        .id("layout_space_in_lambda_literal")
        .locations(&[(1, 7, 1, 7)])
        .correctable(true),
        CopCase::annotated(
            "Layout/EmptyLinesAroundAttributeAccessor",
            r#"
            class Foo
              attr_reader :a
              ^^^^^^^^^^^^^^ Add an empty line after attribute accessor.
              def b; end
            end
            "#,
        )
        .id("layout_empty_lines_around_attribute_accessor")
        .locations(&[(2, 3, 2, 16)])
        .correctable(true),
        CopCase::annotated(
            "Layout/DotPosition",
            r#"
            x = foo.
                   ^ Place the . on the next line, together with the method name.
              bar
            "#,
        )
        .id("layout_dot_position")
        .locations(&[(1, 8, 1, 8)])
        .correctable(true),
        CopCase::annotated(
            "Layout/ElseAlignment",
            r#"
            if a
              b
             else
             ^^^^ Align `else` with `if`.
              c
            end
            "#,
        )
        .id("layout_else_alignment")
        .locations(&[(3, 2, 3, 5)])
        .correctable(true),
        CopCase::annotated(
            "Layout/EndAlignment",
            r#"
            if a
              b
              end
              ^^^ `end` at 3, 2 is not aligned with `if` at 1, 0.
            "#,
        )
        .id("layout_end_alignment")
        .locations(&[(3, 3, 3, 5)])
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Layout/BeginEndAlignment",
            r#"
            x = begin
              1
                end
                ^^^ `end` at 3, 4 is not aligned with `x = begin` at 1, 0.
            "#,
        )
        .id("layout_begin_end_alignment")
        .locations(&[(3, 5, 3, 7)])
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Layout/DefEndAlignment",
            r#"
            private def foo
              1
                        end
                        ^^^ `end` at 3, 12 is not aligned with `private def` at 1, 0.
            "#,
        )
        .id("layout_def_end_alignment")
        .locations(&[(3, 13, 3, 15)])
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Layout/AccessModifierIndentation",
            r#"
            class Foo
              def a; end

            private
            ^^^^^^^ Indent access modifiers like `private`.

              def b; end
            end
            "#,
        )
        .id("layout_access_modifier_indentation")
        .locations(&[(4, 1, 4, 7)])
        .correctable(true),
        CopCase::annotated(
            "Layout/CaseIndentation",
            r#"
            case x
              when 1
              ^^^^ Indent `when` as deep as `case`.
              a
            end
            "#,
        )
        .id("layout_case_indentation")
        .locations(&[(2, 3, 2, 6)])
        .correctable(true),
        CopCase::annotated(
            "Layout/MultilineMethodCallBraceLayout",
            r#"
            foo(a,
              b
            )
            ^ Closing method call brace must be on the same line as the last argument when opening brace is on the same line as the first argument.
            "#,
        )
        .id("layout_multiline_method_call_brace_layout")
        .locations(&[(3, 1, 3, 1)])
        .correctable(true),
        CopCase::annotated(
            "Layout/MultilineArrayBraceLayout",
            r#"
            [ :a,
              :b
            ]
            ^ The closing array brace must be on the same line as the last array element when the opening brace is on the same line as the first array element.
            "#,
        )
        .id("layout_multiline_array_brace_layout")
        .locations(&[(3, 1, 3, 1)])
        .correctable(true),
        CopCase::annotated(
            "Layout/MultilineHashBraceLayout",
            r#"
            { a: 1,
              b: 2
            }
            ^ Closing hash brace must be on the same line as the last hash element when opening brace is on the same line as the first hash element.
            "#,
        )
        .id("layout_multiline_hash_brace_layout")
        .locations(&[(3, 1, 3, 1)])
        .correctable(true),
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
        .id("layout_multiline_method_definition_brace_layout")
        .locations(&[(3, 1, 3, 1)])
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceBeforeSemicolon",
            r#"
            foo ; bar
               ^ Space found before semicolon.
            "#,
        )
        .id("layout_space_before_semicolon")
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceInsideArrayPercentLiteral",
            r#"
            x = %w[a  b]
                    ^^ Use only a single space inside array percent literal.
            "#,
        )
        .id("layout_space_inside_array_percent_literal")
        .correctable(true),
        CopCase::new(
            "Layout/SpaceInsideStringInterpolation",
            "q = \"#{ a}\"\n",
            vec![support::annotation::Annotation::new(
                1,
                8,
                1,
                "Do not use space inside string interpolation.",
            )],
        )
        .id("layout_space_inside_string_interpolation")
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceInsideHashLiteralBraces",
            r#"
            h = {a: 1 }
                ^ Space inside { missing.
            "#,
        )
        .id("layout_space_inside_hash_literal_braces")
        .correctable(true),
        CopCase::new(
            "Layout/EmptyLinesAroundExceptionHandlingKeywords",
            "def foo\n  a\n\nrescue\n  b\nend\n",
            vec![support::annotation::Annotation::new(
                3,
                1,
                0,
                "Extra empty line detected before the `rescue`.",
            )],
        )
        .id("layout_empty_lines_around_exception_handling_keywords")
        .locations(&[(3, 1, 4, 1)])
        .lengths(&[1])
        .correctable(true),
        CopCase::new(
            "Layout/HeredocIndentation",
            "def m\n  x = <<~X\n      hi\n    X\nend\n",
            vec![support::annotation::Annotation::new(
                3,
                1,
                8,
                "Use 2 spaces for indentation in a heredoc.",
            )],
        )
        .id("layout_heredoc_indentation")
        .locations(&[(3, 1, 4, 1)])
        .lengths(&[9])
        .correctable(true),
        // ---- Lint ----
        CopCase::annotated(
            "Lint/BinaryOperatorWithIdenticalOperands",
            r#"
            x = a == a
                ^^^^^^ Binary operator `==` has identical operands.
            "#,
        )
        .id("lint_binary_operator_with_identical_operands")
        .severity(Severity::Warning)
        .correctable(false),
        // `add_global_offense`。空であること自体が offense なので、指す構文が無い。
        CopCase::new(
            "Lint/EmptyFile",
            "",
            vec![support::annotation::Annotation::new(
                1,
                1,
                0,
                "Empty file detected.",
            )],
        )
        .id("lint_empty_file")
        .locations(&[(1, 1, 1, 1)])
        .lengths(&[0])
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/EmptyWhen",
            r#"
            case x
            when 1
            ^^^^^^ Avoid `when` branches without a body.
            end
            "#,
        )
        .id("lint_empty_when")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/InheritException",
            r#"
            class C < Exception; end
                      ^^^^^^^^^ Inherit from `StandardError` instead of `Exception`.
            "#,
        )
        .id("lint_inherit_exception")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/RaiseException",
            r#"
            raise Exception, 'boom'
                  ^^^^^^^^^ Use `StandardError` over `Exception`.
            "#,
        )
        .id("lint_raise_exception")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/AmbiguousBlockAssociation",
            r#"
            some_method a { |val| puts val }
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Parenthesize the param `a { |val| puts val }` to make sure that the block will be associated with the `a` method call.
            "#,
        )
        .id("lint_ambiguous_block_association")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/AssignmentInCondition",
            r#"
            if x = 1
                 ^ Use `==` if you meant to do a comparison or wrap the expression in parentheses to indicate you meant to assign in a condition.
              1
            end
            "#,
        )
        .id("lint_assignment_in_condition")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/ConstantDefinitionInBlock",
            r#"
            [1].each do
              FOO = 1
              ^^^^^^^ Do not define constants this way within a block.
            end
            "#,
        )
        .id("lint_constant_definition_in_block")
        .severity(Severity::Warning)
        .correctable(false),
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
        .id("lint_duplicate_methods")
        .locations(&[(3, 1, 3, 7)]),
        CopCase::annotated(
            "Lint/IneffectiveAccessModifier",
            r#"
            class C
              private

              def self.method
              ^^^ `private` (on line 2) does not make singleton methods private. Use `private_class_method` or `private` inside a `class << self` block instead.
              end
            end
            "#,
        )
        .id("lint_ineffective_access_modifier")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::new(
            "Lint/Syntax",
            "x = )\n",
            vec![support::annotation::Annotation::new(
                1,
                5,
                1,
                format!("unexpected token tRPAREN\n{SYNTAX_HINT}"),
            )],
        )
        .id("lint_syntax_unexpected_token")
        .locations(&[(1, 5, 1, 5)])
        .severity(Severity::Fatal),
        // 本家は壊れた構文から回復した先で追加の診断を出す。ここでは endless
        // メソッドが `def` 文脈を開いたままにするので、`class` の還元が
        // `class definition in method body` になる。実測 (rubocop 1.89.0):
        // 2:9 tEQL と 1:1-1:5 class definition in method body の 2 件。
        CopCase::new(
            "Lint/Syntax",
            "class A\n  def x = 1\nend\n",
            vec![
                support::annotation::Annotation::new(
                    1,
                    1,
                    5,
                    format!("class definition in method body\n{SYNTAX_HINT}"),
                ),
                support::annotation::Annotation::new(
                    2,
                    9,
                    1,
                    format!("unexpected token tEQL\n{SYNTAX_HINT}"),
                ),
            ],
        )
        .id("lint_syntax_recovery_cascade")
        .severity(Severity::Fatal),
        // 本家の parser gem はソースを UTF-8 へ再符号化してから、補間の無い正規表現
        // リテラルを `Regexp.new` に通す。`\xdf` は UTF-8 として不完全なので Onigmo の
        // RegexpError がそのまま診断になる。実測 (rubocop 1.89.0): 1:1 6 文字。
        CopCase::new(
            "Lint/Syntax",
            "/\\xdf/\n",
            vec![support::annotation::Annotation::new(
                1,
                1,
                6,
                format!("too short escaped multibyte character: /\\xdf/\n{SYNTAX_HINT}"),
            )],
        )
        .id("lint_syntax_static_regexp_validation"),
        CopCase::annotated(
            "Lint/InterpolationCheck",
            "foo = 'a#{b}'\n      ^^^^^^^ Interpolation in single quoted string detected. Use double quoted strings if you need interpolation.\n",
        )
        .id("lint_interpolation_check")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::new(
            "Lint/MissingSuper",
            "class Foo < Bar\n  def initialize\n  end\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                3,
                14,
                "Call `super` to initialize state of the parent class.",
            )],
        )
        .id("lint_missing_super")
        .locations(&[(2, 3, 3, 5)])
        .lengths(&[20])
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/BooleanSymbol",
            r#"
            a = :true
                ^^^^^ Symbol with a boolean name - you probably meant to use `true`.
            "#,
        )
        .id("lint_boolean_symbol")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/LiteralInInterpolation",
            "x = \"a#{1}b\"\n        ^ Literal interpolation detected.\n",
        )
        .id("lint_literal_in_interpolation")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::new(
            "Lint/RescueException",
            "begin\n  a\nrescue Exception\n  b\nend\n",
            vec![support::annotation::Annotation::new(
                3,
                1,
                16,
                "Avoid rescuing the `Exception` class. Perhaps you meant to rescue `StandardError`?",
            )],
        )
        .id("lint_rescue_exception")
        .locations(&[(3, 1, 4, 3)])
        .lengths(&[20])
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UnderscorePrefixedVariableName",
            r#"
            def m(_foo)
                  ^^^^ Do not use prefix `_` for a variable that is used.
              _foo
            end
            "#,
        )
        .id("lint_underscore_prefixed_variable_name")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/SuppressedException",
            r#"
            begin
              a
            rescue
            ^^^^^^ Do not suppress exceptions.
            end
            "#,
        )
        .id("lint_suppressed_exception")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UnusedBlockArgument",
            r#"
            [1].each { |x| puts 1 }
                        ^ Unused block argument - `x`. You can omit the argument if you don't care about it.
            "#,
        )
        .id("lint_unused_block_argument"),
        CopCase::annotated(
            "Lint/UnusedMethodArgument",
            r#"
            def m(a)
                  ^ Unused method argument - `a`. If it's necessary, use `_` or `_a` as an argument name to indicate that it won't be used. If it's unnecessary, remove it. You can also write as `m(*)` if you want the method to accept any arguments but don't care about them.
              1
            end
            "#,
        )
        .id("lint_unused_method_argument")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/UselessAssignment",
            r#"
            x = 1
            ^ Useless assignment to variable - `x`.
            "#,
        )
        .id("lint_useless_assignment")
        .severity(Severity::Warning),
        CopCase::annotated(
            "Lint/UselessAccessModifier",
            r#"
            class Foo
              public
              ^^^^^^ Useless `public` access modifier.
              def a; end
            end
            "#,
        )
        .id("lint_useless_access_modifier")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/UselessMethodDefinition",
            r#"
            def foo
            ^^^^^^^ Useless method definition detected.
              super
            end
            "#,
        )
        .id("lint_useless_method_definition")
        .locations(&[(1, 1, 3, 3)])
        .lengths(&[19])
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/HashCompareByIdentity",
            r#"
            hash.key?(foo.object_id)
            ^^^^^^^^^^^^^^^^^^^^^^^^ Use `Hash#compare_by_identity` instead of using `object_id` for keys.
            "#,
        )
        .id("lint_hash_compare_by_identity")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/SelfAssignment",
            r#"
            foo = foo
            ^^^^^^^^^ Self-assignment detected.
            "#,
        )
        .id("lint_self_assignment")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/EmptyInterpolation",
            "x = \"a#{}b\"\n      ^^^ Empty interpolation detected.\n",
        )
        .id("lint_empty_interpolation")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/FloatComparison",
            r#"
            x == 0.1
            ^^^^^^^^ Avoid equality comparisons of floats as they are unreliable.
            "#,
        )
        .id("lint_float_comparison")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/Loop",
            r#"
            begin
              x
            end while y
                ^^^^^ Use `Kernel#loop` with `break` rather than `begin/end/until`(or `while`).
            "#,
        )
        .id("lint_loop")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/NonLocalExitFromIterator",
            r#"
            foo.each do |x|
              return if x
              ^^^^^^ Non-local exit from iterator, without return value. `next`, `break`, `Array#find`, `Array#any?`, etc. is preferred.
              x
            end
            "#,
        )
        .id("lint_non_local_exit_from_iterator")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/StructNewOverride",
            r#"
            Bad = Struct.new(:count)
                             ^^^^^^ `:count` member overrides `Struct#count` and it may be unexpected.
            "#,
        )
        .id("lint_struct_new_override")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/DisjunctiveAssignmentInConstructor",
            r#"
            class C
              def initialize
                @a ||= 1
                   ^^^ Unnecessary disjunctive assignment. Use plain assignment.
              end
            end
            "#,
        )
        .id("lint_disjunctive_assignment_in_constructor")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/ParenthesesAsGroupedExpression",
            r#"
            puts (1)
                ^ `(1)` interpreted as grouped expression.
            "#,
        )
        .id("lint_parentheses_as_grouped_expression")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/ReturnInVoidContext",
            r#"
            class C
              def initialize
                return 1
                ^^^^^^ Do not return a value in `initialize`.
              end
            end
            "#,
        )
        .id("lint_return_in_void_context")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/BigDecimalNew",
            r#"
            BigDecimal.new(123.456, 3)
                       ^^^ `BigDecimal.new()` is deprecated. Use `BigDecimal()` instead.
            "#,
        )
        .id("lint_big_decimal_new")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/DeprecatedClassMethods",
            r#"
            File.exists?(path)
            ^^^^^^^^^^^^ `File.exists?` is deprecated in favor of `File.exist?`.
            "#,
        )
        .id("lint_deprecated_class_methods")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/DuplicateCaseCondition",
            r#"
            case x
            when 1
              a
            when 1
                 ^ Duplicate `when` condition detected.
              b
            end
            "#,
        )
        .id("lint_duplicate_case_condition")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/DuplicateElsifCondition",
            r#"
            if a
              x
            elsif a
                  ^ Duplicate `elsif` condition detected.
              y
            end
            "#,
        )
        .id("lint_duplicate_elsif_condition")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/DuplicateHashKey",
            r#"
            { food: 1, food: 2 }
                       ^^^^ Duplicated key in hash literal.
            "#,
        )
        .id("lint_duplicate_hash_key")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/DuplicateRequire",
            r#"
            require 'foo'
            require 'foo'
            ^^^^^^^^^^^^^ Duplicate `require` detected.
            "#,
        )
        .id("lint_duplicate_require")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/DuplicateRescueException",
            r#"
            begin
              x
            rescue A
              y
            rescue A
                   ^ Duplicate `rescue` exception detected.
              z
            end
            "#,
        )
        .id("lint_duplicate_rescue_exception")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/EmptyEnsure",
            r#"
            def m
              x
            ensure
            ^^^^^^ Empty `ensure` block detected.
            end
            "#,
        )
        .id("lint_empty_ensure")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/EnsureReturn",
            r#"
            def n
              x
            ensure
              return 1
              ^^^^^^^^ Do not return from an `ensure` block.
            end
            "#,
        )
        .id("lint_ensure_return")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/IdentityComparison",
            r#"
            foo.object_id == bar.object_id
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `equal?` instead of `==` when comparing `object_id`.
            "#,
        )
        .id("lint_identity_comparison")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/RandOne",
            r#"
            rand 1
            ^^^^^^ `rand 1` always returns `0`. Perhaps you meant `rand(2)` or `rand`?
            "#,
        )
        .id("lint_rand_one")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UnifiedInteger",
            r#"
            1.is_a?(Fixnum)
                    ^^^^^^ Use `Integer` instead of `Fixnum`.
            "#,
        )
        .id("lint_unified_integer")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/UnreachableCode",
            r#"
            def dead
              return
              do_something
              ^^^^^^^^^^^^ Unreachable code detected.
            end
            "#,
        )
        .id("lint_unreachable_code")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UnreachableLoop",
            r#"
            while node
            ^^^^^^^^^^ This loop will have at most one iteration.
              do_something(node)
              break
            end
            "#,
        )
        .id("lint_unreachable_loop")
        .locations(&[(1, 1, 4, 3)])
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UriEscapeUnescape",
            r#"
            URI.escape('http://example.com')
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ `URI.escape` method is obsolete and should not be used. Instead, use `CGI.escape`, `URI.encode_www_form` or `URI.encode_www_form_component` depending on your specific use case.
            "#,
        )
        .id("lint_uri_escape_unescape")
        .severity(Severity::Warning)
        .correctable(false),
        // 置き換え先の parser 定数は `TargetRubyVersion` で変わる。ハーネス既定の 2.7 では
        // `DEFAULT_PARSER`、3.4 以降は `RFC2396_PARSER`。
        CopCase::annotated(
            "Lint/UriRegexp",
            r#"
            URI.regexp('http://example.com')
                ^^^^^^ `URI.regexp('http://example.com')` is obsolete and should not be used. Instead, use `URI::DEFAULT_PARSER.make_regexp('http://example.com')`.
            "#,
        )
        .id("lint_uri_regexp")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/EachWithObjectArgument",
            r#"
            x.each_with_object(1) { |a, b| b }
            ^^^^^^^^^^^^^^^^^^^^^ The argument to each_with_object cannot be immutable.
            "#,
        )
        .id("lint_each_with_object_argument")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/NextWithoutAccumulator",
            r#"
            [1, 2].reduce(0) do |acc, e|
              acc + e
              next
              ^^^^ Use `next` with an accumulator argument in a `reduce`.
            end
            "#,
        )
        .id("lint_next_without_accumulator")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/ToJSON",
            r#"
            def to_json
            ^^^^^^^^^^^ `#to_json` requires an optional argument to be parsable via JSON.generate(obj).
            end
            "#,
        )
        .id("lint_to_json")
        .locations(&[(1, 1, 2, 3)])
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/TopLevelReturnWithArgument",
            r#"
            return 1
            ^^^^^^^^ Top level return with argument detected.
            "#,
        )
        .id("lint_top_level_return_with_argument")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/TrailingCommaInAttributeDeclaration",
            r#"
            attr_reader :foo,
                            ^ Avoid leaving a trailing comma in attribute declarations.
            def bar
            end
            "#,
        )
        .id("lint_trailing_comma_in_attribute_declaration")
        .severity(Severity::Warning)
        .correctable(true),
        // ---- Metrics ----
        CopCase::annotated(
            "Metrics/AbcSize",
            r#"
            def foo
            ^^^^^^^ Assignment Branch Condition size for `foo` is too high. [<0, 2, 0> 2/0]
              bar.baz
            end
            "#,
        )
        .id("metrics_abc_size")
        .config("Metrics/AbcSize:\n  Max: 0\n")
        .locations(&[(1, 1, 3, 3)]),
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
        .id("metrics_block_length")
        .config("Metrics/BlockLength:\n  Max: 1\n"),
        CopCase::annotated(
            "Metrics/BlockNesting",
            r#"
            if a
              if b
                if c
                  if d
                  ^^^^ Avoid more than 3 levels of block nesting.
                    e
                  end
                end
              end
            end
            "#,
        )
        .id("metrics_block_nesting")
        .locations(&[(4, 7, 6, 9)]),
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
        .id("metrics_class_length")
        .config("Metrics/ClassLength:\n  Max: 1\n")
        .locations(&[(1, 1, 4, 3)]),
        CopCase::annotated(
            "Metrics/CyclomaticComplexity",
            r#"
            def foo
            ^^^^^^^ Cyclomatic complexity for `foo` is too high. [2/1]
              bar if baz
            end
            "#,
        )
        .id("metrics_cyclomatic_complexity")
        .config("Metrics/CyclomaticComplexity:\n  Max: 1\n")
        .locations(&[(1, 1, 3, 3)]),
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
        .id("metrics_method_length")
        .config("Metrics/MethodLength:\n  Max: 1\n"),
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
        .id("metrics_module_length")
        .config("Metrics/ModuleLength:\n  Max: 1\n"),
        CopCase::annotated(
            "Metrics/ParameterLists",
            r#"
            def foo(a, b, c, d, e, f)
                   ^^^^^^^^^^^^^^^^^^ Avoid parameter lists longer than 5 parameters. [6/5]
            end
            "#,
        )
        .id("metrics_parameter_lists"),
        CopCase::annotated(
            "Metrics/PerceivedComplexity",
            r#"
            def foo
            ^^^^^^^ Perceived complexity for `foo` is too high. [2/1]
              bar if baz
            end
            "#,
        )
        .id("metrics_perceived_complexity")
        .config("Metrics/PerceivedComplexity:\n  Max: 1\n")
        .locations(&[(1, 1, 3, 3)]),
        // ---- Migration ----
        CopCase::annotated(
            "Migration/DepartmentName",
            r#"
            # rubocop:disable AbcSize
                              ^^^^^^^ Department name is missing.
            "#,
        )
        .id("migration_department_name")
        .locations(&[(1, 19, 1, 25)])
        .correctable(true),
        // ---- Naming ----
        // `get_` は引数無し、`set_` は必須引数ちょうど 1 つのときだけ accessor 扱い。
        CopCase::annotated(
            "Naming/AccessorMethodName",
            r#"
            def get_value
                ^^^^^^^^^ Do not prefix reader method names with `get_`.
            end
            def set_value(value)
                ^^^^^^^^^ Do not prefix writer method names with `set_`.
            end
            "#,
        )
        .id("naming_accessor_method_name")
        .correctable(false),
        // 本家 `should_check?` は `tIDENTIFIER` / `tCONSTANT` だけを通すので ivar は
        // 対象外。レンジは識別子全体ではなく最初の非 ASCII 連続部分のみ。
        CopCase::new(
            "Naming/AsciiIdentifiers",
            "あ = 1\n@い = 2\ndef う; end\nCLASS_え = 3\n",
            vec![
                support::annotation::Annotation::new(
                    1,
                    1,
                    1,
                    "Use only ascii symbols in identifiers.",
                ),
                support::annotation::Annotation::new(
                    3,
                    5,
                    1,
                    "Use only ascii symbols in identifiers.",
                ),
                support::annotation::Annotation::new(
                    4,
                    7,
                    1,
                    "Use only ascii symbols in constants.",
                ),
            ],
        )
        .id("naming_ascii_identifiers")
        .locations(&[(1, 1, 1, 1), (3, 5, 3, 5), (4, 7, 4, 7)])
        .lengths(&[1, 1, 1]),
        // 演算子の唯一の引数は `other` でなければならない。`eql?` は語で綴られていても
        // 演算子扱いで、`<<` や `[]` は `EXCLUDED` なので対象外。
        CopCase::annotated(
            "Naming/BinaryOperatorParameterName",
            r#"
            def +(amount)
                  ^^^^^^ When defining the `+` operator, name its argument `other`.
              amount
            end
            "#,
        )
        .id("naming_binary_operator_parameter_name")
        .correctable(true),
        // `UncommunicativeName` のレンジは引数の先頭から名前の文字数ぶん。`*` は 1 文字
        // ぶん伸び、`&` は伸びないので名前の途中で切れる。
        CopCase::annotated(
            "Naming/BlockParameterName",
            r#"
            bar { |xA, *yB, &zC| xA }
                   ^^ Only use lowercase characters for block parameter.
                       ^^^ Only use lowercase characters for block parameter.
                            ^^ Only use lowercase characters for block parameter.
            "#,
        )
        .id("naming_block_parameter_name")
        .correctable(false),
        // `AllowedNames` は名前から取り除かれてから `_` の有無を見る。レンジは
        // 定数パス全体。
        CopCase::annotated(
            "Naming/ClassAndModuleCamelCase",
            r#"
            class My_Class
                  ^^^^^^^^ Use CamelCase for classes and modules.
            end
            module module_parent::My_Module
                   ^^^^^^^^^^^^^^^^^^^^^^^^ Use CamelCase for classes and modules.
            end
            "#,
        )
        .id("naming_class_and_module_camel_case")
        .correctable(false),
        CopCase::annotated(
            "Naming/ConstantName",
            r#"
            Foo = 1
            ^^^ Use SCREAMING_SNAKE_CASE for constants.
            "#,
        )
        .id("naming_constant_name"),
        // `add_global_offense` はファイル先頭の長さ 0 のレンジ。
        CopCase::annotated(
            "Naming/FileName",
            "x = 1\n^{} The name of this source file (`fooBar.rb`) should use snake_case.\n",
        )
        .id("naming_file_name")
        .path("fooBar.rb")
        .locations(&[(1, 1, 1, 1)])
        .lengths(&[0])
        .correctable(false),
        // offense は `loc.heredoc_end`、つまり終端の行頭から。autocorrect は開始
        // デリミタも直すので、字下げされた終端は行頭に戻る。
        CopCase::annotated(
            "Naming/HeredocDelimiterCase",
            r#"
            a = <<-sql
              x
            sql
            ^^^ Use uppercase heredoc delimiters.
            "#,
        )
        .id("naming_heredoc_delimiter_case")
        .corrected("a = <<-SQL\n  x\nSQL\n")
        .correctable(true),
        // 空の heredoc には終端の位置が無いので、offense は開始デリミタに付く。
        CopCase::annotated(
            "Naming/HeredocDelimiterNaming",
            r#"
            a = <<-END
              x
            END
            ^^^ Use meaningful heredoc delimiters.
            b = <<~EOS
                ^^^^^^ Use meaningful heredoc delimiters.
            EOS
            "#,
        )
        .id("naming_heredoc_delimiter_naming")
        .correctable(false),
        // メモ化は本体の末尾にあるときだけ見られ、`defined?` 形式は 3 か所すべてが
        // 報告される。
        CopCase::annotated(
            "Naming/MemoizedInstanceVariableName",
            r#"
            def foo
              @something ||= calculate
              ^^^^^^^^^^ Memoized variable `@something` does not match method name `foo`. Use `@foo` instead.
            end
            "#,
        )
        .id("naming_memoized_instance_variable_name")
        .corrected("def foo\n  @foo ||= calculate\nend\n")
        .correctable(true),
        CopCase::annotated(
            "Naming/MethodName",
            r#"
            def fooBar
                ^^^^^^ Use snake_case for method names.
            end
            "#,
        )
        .id("naming_method_name"),
        // 既定の `MinNameLength` は 3。`AllowedNames` に載る `id` は免れ、先頭の `_` は
        // 名前から外れるがレンジの長さには残る。
        CopCase::annotated(
            "Naming/MethodParameterName",
            r#"
            def m(_a, ab, abc, aB, id)
                  ^^ Method parameter must be at least 3 characters long.
                      ^^ Method parameter must be at least 3 characters long.
                               ^^ Only use lowercase characters for method parameter.
            end
            "#,
        )
        .id("naming_method_parameter_name")
        .correctable(false),
        // 接頭辞のあとが数字だったり、名前が `=` で終われば免れる。
        CopCase::annotated(
            "Naming/PredicatePrefix",
            r#"
            def is_even(value)
                ^^^^^^^ Rename `is_even` to `even?`.
            end
            def is_1(value)
            end
            "#,
        )
        .id("naming_predicate_prefix")
        .correctable(false),
        // 入れ子の rescue は外側だけが問われ、`_` で始まる名前には `_` 付きが求められる。
        CopCase::annotated(
            "Naming/RescuedExceptionsVariableName",
            r#"
            begin
              foo
            rescue StandardError => err
                                    ^^^ Use `e` instead of `err`.
              puts err
            end
            "#,
        )
        .id("naming_rescued_exceptions_variable_name")
        .corrected("begin\n  foo\nrescue StandardError => e\n  puts e\nend\n")
        .correctable(true),
        CopCase::annotated(
            "Naming/VariableName",
            r#"
            def foo(barBaz)
                    ^^^^^^ Use snake_case for variable names.
            end
            "#,
        )
        .id("naming_variable_name"),
        // `on_arg` が見るのは必須引数だけで、シンボルはエスケープを解いた値で判定される。
        CopCase::annotated(
            "Naming/VariableNumber",
            r#"
            variable_1 = 1
            ^^^^^^^^^^ Use normalcase for variable numbers.
            def some_method_1(arg_1, opt_1 = 1); end
                ^^^^^^^^^^^^^ Use normalcase for method name numbers.
                              ^^^^^ Use normalcase for variable numbers.
            :some_sym_1
            ^^^^^^^^^^^ Use normalcase for symbol numbers.
            "#,
        )
        .id("naming_variable_number")
        .correctable(false),
        // ---- Security ----
        // 本家 `Cop::Base#default_severity` は `lint? ? :warning : :convention`。
        CopCase::annotated(
            "Security/Eval",
            r#"
            eval(code)
            ^^^^ The use of `eval` is a serious security risk.
            "#,
        )
        .id("security_eval")
        .severity(Severity::Convention),
        CopCase::annotated(
            "Security/JSONLoad",
            r#"
            JSON.load('{}')
                 ^^^^ Prefer `JSON.parse` over `JSON.load`.
            "#,
        )
        .id("security_json_load")
        .severity(Severity::Convention)
        .correctable(true),
        CopCase::annotated(
            "Security/MarshalLoad",
            r#"
            Marshal.load(x)
                    ^^^^ Avoid using `Marshal.load`.
            "#,
        )
        .id("security_marshal_load")
        .severity(Severity::Convention)
        .correctable(false),
        CopCase::annotated(
            "Security/Open",
            r#"
            open(something)
            ^^^^ The use of `Kernel#open` is a serious security risk.
            "#,
        )
        .id("security_open")
        .severity(Severity::Convention)
        .correctable(false),
        // `maximum_target_ruby_version 3.0`: Psych 4 を積む Ruby 3.1 以降では退場する。
        CopCase::annotated(
            "Security/YAMLLoad",
            r#"
            YAML.load('x')
                 ^^^^ Prefer using `YAML.safe_load` over `YAML.load`.
            "#,
        )
        .id("security_yaml_load")
        .severity(Severity::Convention)
        .correctable(true),
        // ---- Style ----
        // 既定の `prefer_alias` では、字句スコープで書かれた `alias_method` が対象。
        CopCase::annotated(
            "Style/Alias",
            r#"
            alias_method :foo, :bar
            ^^^^^^^^^^^^ Use `alias` instead of `alias_method` at the top level.
            "#,
        )
        .id("style_alias")
        .correctable(true),
        // 定数が受け手のときは、名前が小文字を含む (=クラス名らしい) ものだけ対象。
        CopCase::annotated(
            "Style/CaseEquality",
            r#"
            Integer === x
                    ^^^ Avoid the use of the case equality operator `===`.
            "#,
        )
        .id("style_case_equality")
        .correctable(true),
        CopCase::annotated(
            "Style/ClassAndModuleChildren",
            r#"
            class Foo::Bar
                  ^^^^^^^^ Use nested module/class definitions instead of compact style.
              X = 1
            end
            "#,
        )
        .id("style_class_and_module_children")
        .correctable(true),
        // 本体を持つクラスと、本体の有無を問わないモジュールが対象。直上の本物の
        // コメント、`:nodoc:`、名前空間だけの本体は免除される。
        CopCase::annotated(
            "Style/Documentation",
            r#"
            class Foo
            ^^^^^^^^^ Missing top-level documentation comment for `class Foo`.
              def bar; end
            end
            "#,
        )
        .id("style_documentation")
        .correctable(false),
        // 書式文字列の中でだけ直せる。`format` の第 1 引数でなければ報告だけになる。
        CopCase::annotated(
            "Style/FormatStringToken",
            r#"
            x = format("%{foo}", foo: 1)
                        ^^^^^^ Prefer annotated tokens (like `%<foo>s`) over template tokens (like `%{foo}`).
            "#,
        )
        .id("style_format_string_token")
        .correctable(true),
        CopCase::annotated(
            "Style/FrozenStringLiteralComment",
            r#"
            x = 1
            ^ Missing frozen string literal comment.
            "#,
        )
        .id("style_frozen_string_literal_comment")
        .locations(&[(1, 1, 1, 1)]),
        CopCase::annotated(
            "Style/HashSyntax",
            r#"
            puts({ :a => 1 })
                   ^^^^^ Use the new Ruby 1.9 hash syntax.
            "#,
        )
        .id("style_hash_syntax"),
        CopCase::annotated(
            "Style/CommentedKeyword",
            r#"
            def foo # comment
                    ^^^^^^^^^ Do not place comments on the same line as the `def` keyword.
              1
            end
            "#,
        )
        .id("style_commented_keyword")
        .correctable(true),
        // 組み込みのグローバル変数は既定で許される。
        CopCase::annotated(
            "Style/GlobalVars",
            r#"
            $global = 1
            ^^^^^^^ Do not introduce global variables.
            "#,
        )
        .id("style_global_vars")
        .correctable(false),
        // 既定の `line_count_based` では、1 行のブロックは波括弧、複数行は `do...end`。
        CopCase::annotated(
            "Style/BlockDelimiters",
            r#"
            each_with_index do |x| x end
                            ^^ Prefer `{...}` over `do...end` for single-line blocks.
            "#,
        )
        .id("style_block_delimiters")
        .correctable(true),
        // 定義の末尾に立つ条件はガード節へ。既定の `MinBodyLength` は 1。
        CopCase::annotated(
            "Style/GuardClause",
            r#"
            def foo
              bar
              if cond
              ^^ Use a guard clause (`return unless cond`) instead of wrapping the code inside a conditional expression.
                body
              end
            end
            "#,
        )
        .id("style_guard_clause")
        .correctable(true),
        // 1 文の本体は修飾形へ。報告位置は `if` / `unless` のキーワード。
        CopCase::annotated(
            "Style/IfUnlessModifier",
            r#"
            if a
            ^^ Favor modifier `if` usage when having a single-line body. Another good alternative is the usage of control flow `&&`/`||`.
              b
            end
            "#,
        )
        .id("style_if_unless_modifier")
        .correctable(true),
        CopCase::annotated(
            "Style/NumericLiterals",
            r#"
            puts 12345
                 ^^^^^ Use underscores(_) as thousands separator and separate every 3 digits with them.
            "#,
        )
        .id("style_numeric_literals"),
        // 既定の `MinBodyLength` は 3 なので、3 行未満の本体は対象外。
        CopCase::annotated(
            "Style/Next",
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
        )
        .id("style_next")
        .correctable(true),
        // 咎めるのは省略可能引数そのもののレンジで、後ろに続く必須引数ではない。
        CopCase::annotated(
            "Style/OptionalArguments",
            r#"
            def foo(a = 1, b)
                    ^^^^^ Optional arguments should appear at the end of the argument list.
              a + b
            end
            "#,
        )
        .id("style_optional_arguments")
        .correctable(false),
        // 左右の要素数が揃い、循環依存が無いものだけが対象。
        CopCase::annotated(
            "Style/ParallelAssignment",
            r#"
            a, b = 1, 2
            ^^^^^^^^^^^ Do not use parallel assignment.
            "#,
        )
        .id("style_parallel_assignment")
        .correctable(true),
        // 既定の希望区切りは `%w` / `%i` が `[]`、`%r` が `{}`、それ以外が `()`。
        CopCase::annotated(
            "Style/PercentLiteralDelimiters",
            r#"
            %w(a b)
            ^^^^^^^ `%w`-literals should be delimited by `[` and `]`.
            "#,
        )
        .id("style_percent_literal_delimiters")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantReturn",
            r#"
            def first;  1;        end
            def second; return 2; end
                        ^^^^^^ Redundant `return` detected.
            "#,
        )
        .id("style_redundant_return")
        .locations(&[(2, 13, 2, 18)])
        .correctable(true),
        // 既定の `slashes` では、スラッシュを含まない `%r` がスラッシュ形へ、
        // スラッシュを含むスラッシュリテラルが `%r` 形へ回る。
        CopCase::annotated(
            "Style/RegexpLiteral",
            r#"
            x = %r{foo}
                ^^^^^^^ Use `//` around regular expression.
            "#,
        )
        .id("style_regexp_literal")
        .correctable(true),
        CopCase::annotated(
            "Style/Semicolon",
            r#"
            puts 1; puts 2
                  ^ Do not use semicolons to terminate expressions.
            "#,
        )
        .id("style_semicolon")
        .correctable(true),
        // 本体を持たない定義は `AllowIfMethodIsEmpty` で免除される。
        CopCase::annotated(
            "Style/SingleLineMethods",
            r#"
            def foo; bar; end
            ^^^^^^^^^^^^^^^^^ Avoid single-line method definitions.
            "#,
        )
        .id("style_single_line_methods")
        .correctable(true),
        CopCase::annotated(
            "Style/StringLiterals",
            r#"
            puts "hi"
                 ^^^^ Prefer single-quoted strings when you don't need string interpolation or special symbols.
            "#,
        )
        .id("style_string_literals_ascii"),
        // 本家の `location.length` は文字数。sonicop はバイト数を出す。カラムは
        // 両者とも文字単位なのでキャレット比較では見えない差分。
        CopCase::annotated(
            "Style/StringLiterals",
            r#"
            puts "あ"
                 ^^^ Prefer single-quoted strings [...]
            "#,
        )
        .id("style_string_literals_multibyte")
        .locations(&[(1, 6, 1, 8)])
        .lengths(&[3]),
        // 補間の中は `Style/StringLiterals` ではなくこちらが見る。既定は単引用符。
        CopCase::annotated(
            "Style/StringLiteralsInInterpolation",
            r##"
            a = "#{"x"}"
                   ^^^ Prefer single-quoted strings inside interpolations.
            "##,
        )
        .id("style_string_literals_in_interpolation")
        .correctable(true),
        // `MinSize` は 2。名前に空白や釣り合わない括弧を持つ配列は対象外。
        CopCase::annotated(
            "Style/SymbolArray",
            r#"
            a = [:foo, :bar]
                ^^^^^^^^^^^^ Use `%i` or `%I` for an array of symbols.
            "#,
        )
        .id("style_symbol_array")
        .correctable(true),
        // 既定の `no_comma` は、括弧付きの呼び出しと添字参照だけを見る。
        CopCase::annotated(
            "Style/TrailingCommaInArguments",
            r#"
            foo(1, 2,)
                    ^ Avoid comma after the last parameter of a method call.
            "#,
        )
        .id("style_trailing_comma_in_arguments")
        .correctable(true),
        CopCase::annotated(
            "Style/TrailingCommaInArrayLiteral",
            r#"
            a = [1, 2,]
                     ^ Avoid comma after the last item of an array.
            "#,
        )
        .id("style_trailing_comma_in_array_literal")
        .correctable(true),
        CopCase::annotated(
            "Style/TrailingCommaInHashLiteral",
            r#"
            b = { c: 1, }
                      ^ Avoid comma after the last item of a hash.
            "#,
        )
        .id("style_trailing_comma_in_hash_literal")
        .correctable(true),
        // 単語に見えない中身 (空白・記号) を持つ配列は角括弧のまま残る。
        CopCase::annotated(
            "Style/WordArray",
            r#"
            b = ['one', 'two']
                ^^^^^^^^^^^^^^ Use `%w` or `%W` for an array of words.
            "#,
        )
        .id("style_word_array")
        .correctable(true),
    ]
}

/// `Lint/Syntax` が付ける版の注記。
const SYNTAX_HINT: &str =
    "(Using Ruby 2.7 parser; configure using `TargetRubyVersion` parameter, under `AllCops`)";

/// 本体のゲート。全ケースを実行し、差分をマニフェストと突き合わせる。
#[test]
fn matches_rubocop() {
    let manifest = Manifest::load_default();
    let mut regressions = Vec::new();
    let mut stale = Vec::new();

    for case in catalogue() {
        let (verdict, detail) = manifest.judge(&case);
        if !verdict.unknown.is_empty() {
            regressions.push(format!(
                "■ {} に未登録の差分があります\n{}\n{}",
                case.label(),
                verdict
                    .unknown
                    .iter()
                    .map(|divergence| format!("  {divergence}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                indent(&detail),
            ));
            regressions.push(format!(
                "  既知の差分なら {} へ以下を追記してください:\n{}",
                support::manifest::DEFAULT_PATH,
                indent(&support::manifest::suggest(&case, &verdict.unknown)),
            ));
        }
        for entry in verdict.resolved {
            stale.push(format!(
                "■ {} [{}] は直っています。{} から消してください\n    本家: {}\n    sonicop(登録時): {}",
                entry.case_id, entry.kind, support::manifest::DEFAULT_PATH, entry.upstream, entry.sonicop
            ));
        }
    }

    let mut failures = regressions;
    failures.extend(stale);
    assert!(
        failures.is_empty(),
        "本家との一致状況がマニフェストと合いません\n\n{}",
        failures.join("\n\n")
    );
}

/// マニフェスト自体の健全性。
#[test]
fn manifest_is_well_formed() {
    let manifest = Manifest::load_default();
    let case_ids: Vec<String> = catalogue().iter().map(CopCase::label).collect();

    let duplicated: Vec<&String> = case_ids
        .iter()
        .filter(|id| case_ids.iter().filter(|other| other == id).count() > 1)
        .collect();
    assert!(
        duplicated.is_empty(),
        "ケース ID が重複しています: {duplicated:?}"
    );

    let problems = manifest.problems(&case_ids);
    assert!(
        problems.is_empty(),
        "マニフェストに問題があります:\n  {}",
        problems.join("\n  ")
    );
    assert!(
        !manifest.blind_spots.is_empty(),
        "検証コーパスの盲点を 1 件も書いていません。何を検証していないかを\
         書かないレポートは、次に読む人を同じ罠に落とします"
    );
}

/// 実装済みの cop すべてにケースがあること。
///
/// cop を足したのにケースを書き忘れると、その cop は誰にも検証されないまま
/// 「実装済み」に数えられてしまう。実コーパスでの A/B 検証は、コーパスに
/// 該当する入力が無ければ発火しないので、この穴を塞げない。
#[test]
fn every_implemented_cop_has_a_case() {
    let covered: BTreeSet<String> = catalogue()
        .iter()
        .flat_map(|case| case.only.clone())
        .collect();
    let missing: Vec<&'static str> = rule_names()
        .filter(|name| !covered.contains(*name))
        .collect();
    assert!(
        missing.is_empty(),
        "本家との突合ケースが無い cop があります:\n  {}\n\
         tests/conformance.rs の catalogue() へ、本家の実出力を期待値にしたケースを\
         追加してください (期待値は scratchpad/ab_cops.py で確認できます)",
        missing.join("\n  ")
    );
}

/// CONFORMANCE.md の元になるレポートを組み立てる。
///
/// `SONICOP_CONFORMANCE_MD` が指すパスへ書き出す。既定では書き出さず、
/// レポートが組めることだけを確認する。
#[test]
fn generates_the_conformance_report() {
    let markdown = report();
    match std::env::var("SONICOP_CONFORMANCE_MD") {
        Ok(path) => {
            std::fs::write(&path, &markdown).unwrap_or_else(|error| {
                panic!("{path} へ書き出せませんでした: {error}");
            });
        }
        Err(_) => assert!(
            markdown.contains("## 検証していないもの"),
            "レポートに「検証していないもの」の節がありません"
        ),
    }
}

fn report() -> String {
    let manifest = Manifest::load_default();
    let cases = catalogue();
    let case_ids: BTreeSet<String> = cases.iter().map(CopCase::label).collect();

    let mut by_cop: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for entry in &manifest.divergences {
        by_cop.entry(entry.cop.clone()).or_default().push(entry);
        *by_kind.entry(entry.kind.clone()).or_default() += 1;
    }

    let all_cops: BTreeSet<&'static str> = rule_names().collect();
    let diverging: BTreeSet<String> = by_cop.keys().cloned().collect();
    let matching = all_cops.len() - diverging.len();
    let diverging_cases: BTreeSet<String> = manifest
        .divergences
        .iter()
        .map(|entry| entry.case_id.clone())
        .collect();

    let mut out = String::new();
    out.push_str("# 本家 RuboCop との一致状況\n\n");
    out.push_str(
        "本ファイルは `cargo test --test conformance` のマニフェストから生成しています。\
         手で編集しないでください。\n\n",
    );
    out.push_str("## 集計\n\n");
    out.push_str(&format!(
        "- 実装済み cop: {}\n- 全ケース一致した cop: **{}/{}**\n- 検証ケース: {}\n\
         - 差分のあるケース: {}\n- 既知差分エントリ: {}\n\n",
        all_cops.len(),
        matching,
        all_cops.len(),
        cases.len(),
        diverging_cases.len(),
        manifest.divergences.len(),
    ));

    out.push_str("## 差分の種類\n\n| 種類 | 件数 |\n|---|---|\n");
    for kind in Kind::ALL {
        if let Some(count) = by_kind.get(kind.as_str()) {
            out.push_str(&format!("| `{}` | {count} |\n", kind.as_str()));
        }
    }
    out.push('\n');

    out.push_str("## cop 別の差分\n\n");
    for (cop, entries) in &by_cop {
        out.push_str(&format!("### {cop}\n\n"));
        for entry in entries {
            out.push_str(&format!(
                "- **{}** ({}): {}\n  - 本家: `{}`\n  - sonicop: `{}`\n",
                entry.kind,
                entry.case_id,
                entry.note,
                entry.upstream.replace('\n', "\\n"),
                entry.sonicop.replace('\n', "\\n"),
            ));
        }
        out.push('\n');
    }

    out.push_str("## 検証していないもの\n\n");
    out.push_str(
        "コーパスに該当する入力が無ければ cop は一度も発火しない。「N ファイルで 100% 一致」\
         という数字はコーパス選択の産物であり得るため、何を見ていないかを併記する。\n\n",
    );
    out.push_str("| コーパス | 含まれないもの | 結果 |\n|---|---|---|\n");
    for spot in &manifest.blind_spots {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            spot.corpus, spot.not_covered, spot.consequence
        ));
    }
    out.push('\n');

    let unverified: Vec<&str> = all_cops
        .iter()
        .copied()
        .filter(|cop| {
            !cases
                .iter()
                .any(|case| case.only.iter().any(|name| name == cop))
        })
        .collect();
    out.push_str(&format!(
        "突合ケースが無い cop: {}\n\n",
        match unverified.is_empty() {
            true => "なし".to_owned(),
            false => unverified.join(", "),
        }
    ));

    out.push_str("### 検証軸のカバレッジ\n\n");
    out.push_str(&format!(
        "| 軸 | 検証しているケース数 |\n|---|---|\n\
         | offense の集合 (行/カラム/長さ/文言) | {} |\n\
         | レンジ終端 (`last_line` / `last_column`) | {} |\n\
         | `location.length` の単位 | {} |\n\
         | `severity` | {} |\n\
         | `correctable` | {} |\n\
         | autocorrect の結果 | {} |\n\n",
        cases.iter().filter(|case| case.expected.is_some()).count(),
        cases.iter().filter(|case| case.locations.is_some()).count(),
        cases.iter().filter(|case| case.lengths.is_some()).count(),
        cases.iter().filter(|case| case.severity.is_some()).count(),
        cases
            .iter()
            .filter(|case| case.correctable.is_some())
            .count(),
        cases.iter().filter(|case| case.corrected.is_some()).count(),
    ));

    let _ = case_ids;
    out
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
