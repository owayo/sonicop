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
        // 既定無効なので `--only` で強制的に有効にして測る。
        CopCase::annotated(
            "Bundler/GemComment",
            "gem 'a'\n^^^^^^^ Missing gem description comment.\n",
        )
        .id("bundler_gem_comment")
        .path("Gemfile")
        .correctable(false),
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
        // 既定無効なので `--only` で強制的に有効にして測る。
        CopCase::annotated(
            "Bundler/GemVersion",
            "gem 'rubocop'\n^^^^^^^^^^^^^ Gem version specification is required.\n",
        )
        .id("bundler_gem_version")
        .path("Gemfile")
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
            "Gemspec/AddRuntimeDependency",
            "spec.add_runtime_dependency 'rake'\n     ^^^^^^^^^^^^^^^^^^^^^^ Use `add_dependency` instead of `add_runtime_dependency`.\n",
        )
        .id("gemspec_add_runtime_dependency")
        .path("example.gemspec")
        .correctable(true),
        CopCase::annotated(
            "Gemspec/AttributeAssignment",
            r#"
            Gem::Specification.new do |spec|
              spec.metadata = {}
              spec.metadata['a'] = 'b'
              ^^^^^^^^^^^^^^^^^^^^^^^^ Use consistent style for Gemspec attributes assignment.
            end
            "#,
        )
        .id("gemspec_attribute_assignment")
        .path("example.gemspec")
        .correctable(false),
        // 既定無効なので `--only` で強制的に有効にして測る。
        CopCase::annotated(
            "Gemspec/DependencyVersion",
            r#"
            Gem::Specification.new do |spec|
              spec.add_dependency 'rubocop'
              ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Dependency version specification is required.
            end
            "#,
        )
        .id("gemspec_dependency_version")
        .path("example.gemspec")
        .correctable(false),
        CopCase::annotated(
            "Gemspec/DeprecatedAttributeAssignment",
            r#"
            Gem::Specification.new do |spec|
              spec.date = Time.now
              ^^^^^^^^^^^^^^^^^^^^ Do not set `date` in gemspec.
            end
            "#,
        )
        .id("gemspec_deprecated_attribute_assignment")
        .path("example.gemspec")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Gemspec/DevelopmentDependencies",
            "spec.add_development_dependency 'rspec'\n^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Specify development dependencies in Gemfile.\n",
        )
        .id("gemspec_development_dependencies")
        .path("example.gemspec")
        .correctable(false),
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
        // ブロック全体が報告され、`end` の直前に設定が書き足される。
        CopCase::annotated(
            "Gemspec/RequireMFA",
            r#"
            Gem::Specification.new do |spec|
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ `metadata['rubygems_mfa_required']` must be set to `'true'`.
              spec.name = 'x'
            end
            "#,
        )
        .id("gemspec_require_mfa")
        .path("example.gemspec")
        .severity(Severity::Warning)
        .locations(&[(1, 1, 3, 3)])
        .lengths(&[54]),
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
            "Layout/EmptyLineAfterMultilineCondition",
            r#"
            if a &&
               ^^^^ Use empty line after multiline condition.
               b
              do_x
            end
            "#,
        )
        .id("layout_empty_line_after_multiline_condition")
        .locations(&[(1, 4, 2, 4)])
        .lengths(&[9])
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
            "Layout/EmptyLinesAfterModuleInclusion",
            r#"
            class A
              include Foo
              ^^^^^^^^^^^ Add an empty line after module inclusion.
              def bar; end
            end
            "#,
        )
        .id("layout_empty_lines_after_module_inclusion")
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
            "Layout/FirstArrayElementLineBreak",
            "a = [1,\n     ^ Add a line break before the first element of a multi-line array.\n  2]\n",
        )
        .id("layout_first_array_element_line_break")
        .correctable(true),
        CopCase::annotated(
            "Layout/FirstHashElementLineBreak",
            "a = { b: 1,\n      ^^^^ Add a line break before the first element of a multi-line hash.\n  c: 2 }\n",
        )
        .id("layout_first_hash_element_line_break")
        .correctable(true),
        CopCase::annotated(
            "Layout/FirstMethodArgumentLineBreak",
            "foo(1,\n    ^ Add a line break before the first argument of a multi-line method argument list.\n  2)\n",
        )
        .id("layout_first_method_argument_line_break")
        .correctable(true),
        CopCase::annotated(
            "Layout/FirstMethodParameterLineBreak",
            "def m(x,\n      ^ Add a line break before the first parameter of a multi-line method parameter list.\n  y); end\n",
        )
        .id("layout_first_method_parameter_line_break")
        .correctable(true),
        CopCase::annotated(
            "Layout/FirstArgumentIndentation",
            r#"
            some_method(
            first_param,
            ^^^^^^^^^^^ Indent the first argument one step more than the start of the previous line.
            second_param)
            "#,
        )
        .id("layout_first_argument_indentation")
        .locations(&[(2, 1, 2, 11)])
        .correctable(true),
        CopCase::annotated(
            "Layout/FirstParameterIndentation",
            r#"
            def some_method(
            first_param,
            ^^^^^^^^^^^ Use 2 spaces for indentation in method args, relative to the start of the line where the left parenthesis is.
            second_param)
              123
            end
            "#,
        )
        .id("layout_first_parameter_indentation")
        .locations(&[(2, 1, 2, 11)])
        .correctable(true),
        CopCase::annotated(
            "Layout/ParameterAlignment",
            r#"
            def foo(bar,
                 baz)
                 ^^^ Align the parameters of a method definition if they span more than one line.
              123
            end
            "#,
        )
        .id("layout_parameter_alignment")
        .locations(&[(2, 6, 2, 8)])
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
            "Layout/LineContinuationLeadingSpace",
            "x = 'foo' \\\n    ' bar'\n     ^ Move leading spaces to the end of the previous line.\n",
        )
        .id("layout_line_continuation_leading_space")
        .correctable(true),
        CopCase::annotated(
            "Layout/LineContinuationSpacing",
            "x = 1 +  \\\n       ^^^ Use one space in front of backslash.\n  2\n",
        )
        .id("layout_line_continuation_spacing")
        .correctable(true),
        CopCase::annotated(
            "Layout/LineEndStringConcatenationIndentation",
            r#"
            text = 'offense' \
              'here'
              ^^^^^^ Align parts of a string concatenated with backslash.
            "#,
        )
        .id("layout_line_end_string_concatenation_indentation")
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
        // 既定の `either` は「式の先頭」と「`do` の行頭」の両方を許すので、
        // メッセージには両方が並ぶ。
        CopCase::annotated(
            "Layout/BlockAlignment",
            r#"
            foo.bar
              .each do
                baz
                    end
                    ^^^ `end` at 4, 8 is not aligned with `foo.bar` at 1, 0 or `.each do` at 2, 2.
            "#,
        )
        .id("layout_block_alignment")
        .locations(&[(4, 9, 4, 11)])
        .lengths(&[3])
        .correctable(true),
        CopCase::annotated(
            "Layout/ExtraSpacing",
            r#"
            x  = 1
             ^ Unnecessary spacing detected.
            "#,
        )
        .id("layout_extra_spacing")
        .locations(&[(1, 2, 1, 2)])
        .lengths(&[1])
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
            "Layout/MultilineArrayLineBreaks",
            "a = [1, 2,\n        ^ Each item in a multi-line array must start on a separate line.\n  3]\n",
        )
        .id("layout_multiline_array_line_breaks")
        .correctable(true),
        CopCase::annotated(
            "Layout/MultilineHashKeyLineBreaks",
            "a = { b: 1, c: 2,\n            ^^^^ Each key in a multi-line hash must start on a separate line.\n  d: 3 }\n",
        )
        .id("layout_multiline_hash_key_line_breaks")
        .correctable(true),
        CopCase::annotated(
            "Layout/MultilineMethodArgumentLineBreaks",
            "foo(1, 2,\n       ^ Each argument in a multi-line method call must start on a separate line.\n  3)\n",
        )
        .id("layout_multiline_method_argument_line_breaks")
        .correctable(true),
        CopCase::annotated(
            "Layout/MultilineMethodParameterLineBreaks",
            "def m(x, y,\n         ^ Each parameter in a multi-line method definition must start on a separate line.\n  z); end\n",
        )
        .id("layout_multiline_method_parameter_line_breaks")
        .correctable(true),
        CopCase::annotated(
            "Layout/MultilineAssignmentLayout",
            r#"
            foo = if bar
            ^^^^^^^^^^^^ Right hand side of multi-line assignment is on the same line as the assignment operator `=`.
                    1
                  end
            "#,
        )
        .id("layout_multiline_assignment_layout")
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
            "Layout/MultilineMethodCallIndentation",
            r#"
            Thing.a
               .b
               ^^ Align `.b` with `.a` on line 1.
            "#,
        )
        .id("layout_multiline_method_call_indentation")
        .locations(&[(2, 4, 2, 5)])
        .correctable(true),
        CopCase::annotated(
            "Layout/MultilineOperationIndentation",
            r#"
            if a +
                b
                ^ Align the operands of a condition in an `if` statement spanning multiple lines.
              c
            end
            "#,
        )
        .id("layout_multiline_operation_indentation")
        .locations(&[(2, 5, 2, 5)])
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
        CopCase::annotated(
            "Layout/EmptyComment",
            r#"
            #
            ^ Source code comment is empty.
            class Foo
            end
            "#,
        )
        .id("layout_empty_comment")
        .correctable(true),
        CopCase::new(
            "Layout/BlockEndNewline",
            "blah do |i|\n  foo(i) end\n",
            vec![support::annotation::Annotation::new(
                2,
                10,
                3,
                "Expression at 2, 10 should be on its own line.",
            )],
        )
        .id("layout_block_end_newline")
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceInsideRangeLiteral",
            r#"
            x = 1 .. 3
                ^^^^^^ Space inside range literal.
            "#,
        )
        .id("layout_space_inside_range_literal")
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceAfterNot",
            r#"
            y = ! foo
                ^^^^^ Do not leave space between `!` and its argument.
            "#,
        )
        .id("layout_space_after_not")
        .correctable(true),
        CopCase::new(
            "Layout/IndentationStyle",
            "def x\n\ty = 1\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                1,
                1,
                "Tab detected in indentation.",
            )],
        )
        .id("layout_indentation_style")
        .correctable(true),
        CopCase::new(
            "Layout/InitialIndentation",
            "  x = 1\n  y = 2\n",
            vec![support::annotation::Annotation::new(
                1,
                3,
                1,
                "Indentation of first line in file detected.",
            )],
        )
        .id("layout_initial_indentation")
        .correctable(true),
        CopCase::new(
            "Layout/EmptyLinesAroundClassBody",
            "class C\n\n  def m; end\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                1,
                0,
                "Extra empty line detected at class body beginning.",
            )],
        )
        .id("layout_empty_lines_around_class_body")
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .correctable(true),
        CopCase::new(
            "Layout/EmptyLinesAroundModuleBody",
            "module M\n\n  X = 1\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                1,
                0,
                "Extra empty line detected at module body beginning.",
            )],
        )
        .id("layout_empty_lines_around_module_body")
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .correctable(true),
        CopCase::new(
            "Layout/EmptyLinesAroundMethodBody",
            "def foo\n\n  1\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                1,
                0,
                "Extra empty line detected at method body beginning.",
            )],
        )
        .id("layout_empty_lines_around_method_body")
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .correctable(true),
        CopCase::new(
            "Layout/EmptyLinesAroundBeginBody",
            "begin\n\n  y\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                1,
                0,
                "Extra empty line detected at `begin` body beginning.",
            )],
        )
        .id("layout_empty_lines_around_begin_body")
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .correctable(true),
        CopCase::new(
            "Layout/EmptyLinesAroundBlockBody",
            "foo do\n\n  z\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                1,
                0,
                "Extra empty line detected at block body beginning.",
            )],
        )
        .id("layout_empty_lines_around_block_body")
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .correctable(true),
        CopCase::annotated(
            "Layout/SingleLineBlockChain",
            "a = b.map { |x| x }.first\n                   ^^^^^^ Put method call on a separate line if chained to a single line block.\n",
        )
        .id("layout_single_line_block_chain")
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceAfterColon",
            r#"
            h = {a:3}
                  ^ Space missing after colon.
            "#,
        )
        .id("layout_space_after_colon")
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceAfterMethodName",
            r#"
            def g (x); end
                 ^ Do not put a space between a method name and the opening parenthesis.
            "#,
        )
        .id("layout_space_after_method_name")
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceAfterSemicolon",
            r#"
            k = 1;l = 2
                 ^ Space missing after semicolon.
            "#,
        )
        .id("layout_space_after_semicolon")
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceBeforeBrackets",
            "collection = [1, 2]\ncollection [0]\n          ^ Remove the space before the opening brackets.\n",
        )
        .id("layout_space_before_brackets")
        .correctable(true),
        CopCase::annotated(
            "Layout/SpaceBeforeComma",
            r#"
            h = [1 , 2]
                  ^ Space found before comma.
            "#,
        )
        .id("layout_space_before_comma")
        .correctable(true),
        CopCase::annotated(
            "Layout/LeadingCommentSpace",
            r#"
            #comment
            ^^^^^^^^ Missing space after `#`.
            "#,
        )
        .id("layout_leading_comment_space")
        .correctable(true),
        CopCase::new(
            "Layout/LeadingEmptyLines",
            "\n\nx = 1\n",
            vec![support::annotation::Annotation::new(
                3,
                1,
                1,
                "Unnecessary blank line at the beginning of the source.",
            )],
        )
        .id("layout_leading_empty_lines")
        .correctable(true),
        CopCase::new(
            "Layout/AssignmentIndentation",
            "value =\nif foo\n  1\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                1,
                6,
                "Indent the first line of the right-hand-side of a multi-line assignment.",
            )],
        )
        .id("layout_assignment_indentation")
        .locations(&[(2, 1, 4, 3)])
        .lengths(&[14])
        .correctable(true),
        CopCase::new(
            "Layout/ConditionPosition",
            "if\n  x\n  puts 1\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                3,
                1,
                "Place the condition on the same line as `if`.",
            )],
        )
        .id("layout_condition_position")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Layout/ClassStructure",
            r#"
            class A
              def pub; end
              include Bar
              ^^^^^^^^^^^ `module_inclusion` is supposed to appear before `public_methods`.
            end
            "#,
        )
        .id("layout_class_structure")
        .correctable(true),
        CopCase::new(
            "Layout/ClosingHeredocIndentation",
            "def foo\n  <<~SQL\n    Hi\n      SQL\nend\n",
            vec![support::annotation::Annotation::new(
                4,
                1,
                9,
                "`SQL` is not aligned with `<<~SQL`.",
            )],
        )
        .id("layout_closing_heredoc_indentation")
        .correctable(true),
        CopCase::new(
            "Layout/ClosingParenthesisIndentation",
            "foo(a,\n  b\n    )\n",
            vec![support::annotation::Annotation::new(
                3,
                5,
                1,
                "Indent `)` to column 0 (not 4)",
            )],
        )
        .id("layout_closing_parenthesis_indentation")
        .correctable(true),
        CopCase::new(
            "Layout/CommentIndentation",
            "def a\n    # comment\n  b\nend\n",
            vec![support::annotation::Annotation::new(
                2,
                5,
                9,
                "Incorrect indentation detected (column 4 instead of 2).",
            )],
        )
        .id("layout_comment_indentation")
        .correctable(true),
        CopCase::new(
            "Layout/EmptyLinesAroundArguments",
            "foo(a,\n\n  b\n)\n",
            vec![support::annotation::Annotation::new(
                2,
                1,
                0,
                "Empty line detected around arguments.",
            )],
        )
        .id("layout_empty_lines_around_arguments")
        .locations(&[(2, 1, 3, 1)])
        .lengths(&[1])
        .correctable(true),
        CopCase::new(
            "Layout/MultilineBlockLayout",
            "bar { |a,\n  b| a }\n",
            vec![support::annotation::Annotation::new(
                1,
                7,
                3,
                "Block argument expression is not on the same line as the block start.",
            )],
        )
        .id("layout_multiline_block_layout")
        .locations(&[(1, 7, 2, 4)])
        .lengths(&[8])
        .correctable(true),
        CopCase::new(
            "Layout/RescueEnsureAlignment",
            "def foo\n  bar\n  rescue StandardError\n  baz\nend\n",
            vec![support::annotation::Annotation::new(
                3,
                3,
                6,
                "`rescue` at 3, 2 is not aligned with `def foo` at 1, 0.",
            )],
        )
        .id("layout_rescue_ensure_alignment")
        .correctable(true),
        CopCase::new(
            "Layout/SpaceAroundBlockParameters",
            "[1].each { | a | a }\n",
            vec![
                support::annotation::Annotation::new(
                    1,
                    13,
                    1,
                    "Space before first block parameter detected.",
                ),
                support::annotation::Annotation::new(
                    1,
                    15,
                    1,
                    "Space after last block parameter detected.",
                ),
            ],
        )
        .id("layout_space_around_block_parameters")
        .correctable(true),
        CopCase::new(
            "Layout/SpaceAroundEqualsInParameterDefault",
            "def m(a=1)\nend\n",
            vec![support::annotation::Annotation::new(
                1,
                8,
                1,
                "Surrounding space missing in default value assignment.",
            )],
        )
        .id("layout_space_around_equals_in_parameter_default")
        .correctable(true),
        CopCase::new(
            "Layout/SpaceAroundKeyword",
            "if(x)\nend\n",
            vec![support::annotation::Annotation::new(
                1,
                1,
                2,
                "Space after keyword `if` is missing.",
            )],
        )
        .id("layout_space_around_keyword")
        .correctable(true),
        CopCase::new(
            "Layout/SpaceAroundMethodCallOperator",
            "foo. bar\n",
            vec![support::annotation::Annotation::new(
                1,
                5,
                1,
                "Avoid using spaces around a method call operator.",
            )],
        )
        .id("layout_space_around_method_call_operator")
        .correctable(true),
        CopCase::new(
            "Layout/SpaceBeforeBlockBraces",
            "7.times{}\n",
            vec![support::annotation::Annotation::new(
                1,
                8,
                1,
                "Space missing to the left of {.",
            )],
        )
        .id("layout_space_before_block_braces")
        .correctable(true),
        CopCase::new(
            "Layout/SpaceBeforeComment",
            "y = 1#comment\n",
            vec![support::annotation::Annotation::new(
                1,
                6,
                8,
                "Put a space before an end-of-line comment.",
            )],
        )
        .id("layout_space_before_comment")
        .correctable(true),
        CopCase::new(
            "Layout/SpaceBeforeFirstArg",
            "foo  1\n",
            vec![support::annotation::Annotation::new(
                1,
                4,
                2,
                "Put one space between the method name and the first argument.",
            )],
        )
        .id("layout_space_before_first_arg")
        .correctable(true),
        CopCase::new(
            "Layout/SpaceInsideReferenceBrackets",
            "a[ :k ]\n",
            vec![
                support::annotation::Annotation::new(
                    1,
                    3,
                    1,
                    "Do not use space inside reference brackets.",
                ),
                support::annotation::Annotation::new(
                    1,
                    6,
                    1,
                    "Do not use space inside reference brackets.",
                ),
            ],
        )
        .id("layout_space_inside_reference_brackets"),
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
        // 本家はこの cop を `--only` と併用できない (`OptionArgumentError`)。ハーネスも
        // 同じ形にそろえるため、選択は `--except` 側で表す。
        CopCase::annotated(
            "Lint/RedundantCopDisableDirective",
            r#"
            # rubocop:disable Layout/LineLength
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Unnecessary disabling of `Layout/LineLength`.
            x = 1
            # rubocop:enable Layout/LineLength
            "#,
        )
        .id("lint_redundant_cop_disable_directive")
        .without_only()
        .corrected("x = 1\n# rubocop:enable Layout/LineLength\n")
        .severity(Severity::Warning)
        .correctable(true),
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
        CopCase::annotated(
            "Lint/RedundantWithIndex",
            r#"
            ary.each_with_index { |x| p x }
                ^^^^^^^^^^^^^^^ Use `each` instead of `each_with_index`.
            ary.each.with_index { |x| p x }
                     ^^^^^^^^^^ Remove redundant `with_index`.
            "#,
        )
        .id("lint_redundant_with_index")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/RedundantWithObject",
            r#"
            ary.each_with_object([]) { |x| p x }
                ^^^^^^^^^^^^^^^^^^^^ Use `each` instead of `each_with_object`.
            ary.each.with_object({}) { |x| p x }
                     ^^^^^^^^^^^^^^^ Remove redundant `with_object`.
            "#,
        )
        .id("lint_redundant_with_object")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/RescueType",
            r#"
            begin
              a
            rescue nil
            ^^^^^^^^^^ Rescuing from `nil` will raise a `TypeError` instead of catching the actual exception.
              b
            end
            "#,
        )
        .id("lint_rescue_type")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/RequireParentheses",
            r#"
            foo a && b ? 1 : 2
            ^^^^^^^^^^ Use parentheses in the method call to avoid confusion about precedence.
            "#,
        )
        .id("lint_require_parentheses")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/RegexpAsCondition",
            r#"
            if /re/
               ^^^^ Do not use regexp literal as a condition. The regexp literal matches `$_` implicitly.
              p 1
            end
            "#,
        )
        .id("lint_regexp_as_condition")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/EmptyExpression",
            r#"
            a = ()
                ^^ Avoid empty expressions.
            "#,
        )
        .id("lint_empty_expression")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/CircularArgumentReference",
            r#"
            def foo(bar = bar)
                          ^^^ Circular argument reference - `bar`.
            end
            "#,
        )
        .id("lint_circular_argument_reference")
        .target_ruby("2.6")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/RedundantStringCoercion",
            r##"
            puts "#{foo.to_s}"
                        ^^^^ Redundant use of `Object#to_s` in interpolation.
            "##,
        )
        .id("lint_redundant_string_coercion")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/SendWithMixinArgument",
            r#"
            send(:include, Foo)
            ^^^^^^^^^^^^^^^^^^^ Use `include Foo` instead of `send(:include, Foo)`.
            "#,
        )
        .id("lint_send_with_mixin_argument")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/MultipleComparison",
            r#"
            p x < y < z
              ^^^^^^^^^ Use the `&&` operator to compare multiple values.
            "#,
        )
        .id("lint_multiple_comparison")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/FloatOutOfRange",
            r#"
            a = 1.0e400
                ^^^^^^^ Float out of range.
            "#,
        )
        .id("lint_float_out_of_range")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/RedundantRequireStatement",
            r#"
            require 'enumerator'
            ^^^^^^^^^^^^^^^^^^^^ Remove unnecessary `require` statement.
            "#,
        )
        .id("lint_redundant_require_statement")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/PercentStringArray",
            r#"
            a = %w[one, "two"]
                ^^^^^^^^^^^^^^ Within `%w`/`%W`, quotes and ',' are unnecessary and may be unwanted in the resulting strings.
            "#,
        )
        .id("lint_percent_string_array")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/PercentSymbolArray",
            r#"
            a = %i[:one, :two]
                ^^^^^^^^^^^^^^ Within `%i`/`%I`, ':' and ',' are unnecessary and may be unwanted in the resulting symbols.
            "#,
        )
        .id("lint_percent_symbol_array")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/NestedPercentLiteral",
            r#"
            a = %w[%w[nested]]
                ^^^^^^^^^^^^^^ Within percent literals, nested percent literals do not function and may be unwanted in the result.
            "#,
        )
        .id("lint_nested_percent_literal")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/OrderedMagicComments",
            r#"
            # frozen_string_literal: true
            # encoding: ascii
            ^^^^^^^^^^^^^^^^^ The encoding magic comment should precede all other magic comments.
            "#,
        )
        .id("lint_ordered_magic_comments")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/ElseLayout",
            r#"
            if something
              foo
            else bar
                 ^^^ Odd `else` layout detected. Did you mean to use `elsif`?
              baz
            end
            "#,
        )
        .id("lint_else_layout")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/ImplicitStringConcatenation",
            r#"
            array = ["foo" "bar"]
                     ^^^^^^^^^^^ Combine "foo" and "bar" into a single string literal, rather than using implicit string concatenation. Or, if they were intended to be separate array elements, separate them with a comma.
            "#,
        )
        .id("lint_implicit_string_concatenation")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/NestedMethodDefinition",
            r#"
            def foo
              def bar
              ^^^^^^^ Method definitions must not be nested. Use `lambda` instead.
              end
            end
            "#,
        )
        .id("lint_nested_method_definition")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UselessElseWithoutRescue",
            r#"
            begin
              do_something
            else
            ^^^^ `else` without `rescue` is useless.
              handle_errors
            end
            "#,
        )
        .id("lint_useless_else_without_rescue")
        .target_ruby("2.5")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/DeprecatedOpenSSLConstant",
            r#"
            OpenSSL::Cipher::AES.new(128, :GCM)
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `OpenSSL::Cipher.new('aes-128-gcm')` instead of `OpenSSL::Cipher::AES.new(128, :GCM)`.
            "#,
        )
        .id("lint_deprecated_open_ssl_constant")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/Debugger",
            r#"
            binding.pry
            ^^^^^^^^^^^ Remove debugger entry point `binding.pry`.
            "#,
        )
        .id("lint_debugger")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/SafeNavigationWithEmpty",
            r#"
            return if foo&.empty?
                      ^^^^^^^^^^^ Avoid calling `empty?` with the safe navigation operator in conditionals.
            "#,
        )
        .id("lint_safe_navigation_with_empty")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/ErbNewArguments",
            r#"
            ERB.new(str, nil, '-')
                         ^^^ Passing safe_level with the 2nd argument of `ERB.new` is deprecated. Do not use it, and specify other arguments as keyword arguments.
                              ^^^ Passing trim_mode with the 3rd argument of `ERB.new` is deprecated. Use keyword argument like `ERB.new(str, trim_mode: '-')` instead.
            "#,
        )
        .id("lint_erb_new_arguments")
        .severity(Severity::Warning)
        .correctable(true)
        .corrected("ERB.new(str, trim_mode: '-')\n"),
        CopCase::annotated(
            "Lint/NonDeterministicRequireOrder",
            r#"
            Dir.glob('./lib/*.rb').each do |file|
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^ Sort files before requiring them.
              require file
            end
            "#,
        )
        .id("lint_non_deterministic_require_order")
        .target_ruby("2.7")
        .severity(Severity::Warning)
        .correctable(true)
        .corrected("Dir.glob('./lib/*.rb').sort.each do |file|\n  require file\nend\n"),
        CopCase::annotated(
            "Lint/SafeNavigationChain",
            r#"
            foo&.bar.baz
                    ^^^^ Do not chain ordinary method call after safe navigation operator.
            "#,
        )
        .id("lint_safe_navigation_chain")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/SafeNavigationConsistency",
            r#"
            foo&.bar && foo&.baz
                           ^^ Use `.` instead of unnecessary `&.`.
            "#,
        )
        .id("lint_safe_navigation_consistency")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/RedundantSafeNavigation",
            r#"
            do_something.to_s&.strip
                             ^^ Redundant safe navigation detected, use `.` instead.
            "#,
        )
        .id("lint_redundant_safe_navigation")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/RedundantSplatExpansion",
            r#"
            foo(*[1, 2])
                ^^^^^^^ Pass array contents as separate arguments.
            "#,
        )
        .id("lint_redundant_splat_expansion")
        .severity(Severity::Warning)
        .correctable(true)
        .corrected("foo(1, 2)\n"),
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
        .id("lint_shadowed_exception")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UselessSetterCall",
            r#"
            def foo
              x = Object.new
              x.attr = 1
              ^ Useless setter call to local variable `x`.
            end
            "#,
        )
        .id("lint_useless_setter_call")
        .severity(Severity::Warning)
        .correctable(true),
        // 本家は `File.exist?` が偽のソースを実行可能とみなして見送る。ハーネスは
        // ファイルを書き出さないので、ここで検証できるのはその陰性側だけ。実ファイルの
        // 権限を見る陽性ケースは `tests/cops.rs` にある。
        CopCase::new(
            "Lint/ScriptPermission",
            "#!/usr/bin/env ruby\nputs 1\n".to_owned(),
            Vec::new(),
        )
        .id("lint_script_permission")
        .path("script.rb"),
        CopCase::annotated(
            "Lint/ShadowedArgument",
            r#"
            def do_something(foo)
              foo = 42
              ^^^^^^^^ Argument `foo` was shadowed by a local variable before it was used.
              puts foo
            end
            "#,
        )
        .id("lint_shadowed_argument")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/LiteralAsCondition",
            r#"
            if 20
               ^^ Literal `20` appeared as a condition.
              do_something
            end
            "#,
        )
        .id("lint_literal_as_condition")
        .severity(Severity::Warning)
        .correctable(true)
        .corrected("do_something\n"),
        CopCase::annotated(
            "Lint/Void",
            r#"
            def some_method
              some_num * 10
                       ^ Operator `*` used in void context.
              do_something
            end
            "#,
        )
        .id("lint_void")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/FormatParameterMismatch",
            r#"
            format("%s %s", 1)
            ^^^^^^ Number of arguments (1) to `format` doesn't match the number of fields (2).
            "#,
        )
        .id("lint_format_parameter_mismatch")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/AmbiguousOperator",
            r#"
            foo *[]
                ^ Ambiguous splat operator. Parenthesize the method arguments if it's surely a splat operator, or add a whitespace to the right of the `*` if it should be a multiplication.
            "#,
        )
        .id("lint_ambiguous_operator")
        .severity(Severity::Warning)
        .correctable(true)
        .corrected("foo(*[])\n"),
        CopCase::annotated(
            "Lint/AmbiguousRegexpLiteral",
            r#"
            foo /re/, 1
                ^ Ambiguous regexp literal. Parenthesize the method arguments if it's surely a regexp literal, or add a whitespace to the right of the `/` if it should be a division.
            "#,
        )
        .id("lint_ambiguous_regexp_literal")
        .severity(Severity::Warning)
        .correctable(true)
        .corrected("foo(/re/, 1)\n"),
        CopCase::annotated(
            "Lint/MissingCopEnableDirective",
            "# rubocop:disable Layout/LineLength\n^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Re-enable Layout/LineLength cop with `# rubocop:enable` after disabling it.\nfoo = 1\n",
        )
        .id("lint_missing_cop_enable_directive")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/RedundantCopEnableDirective",
            "# rubocop:enable Layout/LineLength\n                 ^^^^^^^^^^^^^^^^^ Unnecessary enabling of Layout/LineLength.\nfoo = 1\n",
        )
        .id("lint_redundant_cop_enable_directive")
        .severity(Severity::Warning)
        .correctable(true)
        .corrected("foo = 1\n"),
        CopCase::annotated(
            "Lint/EmptyConditionalBody",
            r#"
            if condition
            ^^^^^^^^^^^^ Avoid `if` branches without a body.
            end
            "#,
        )
        .id("lint_empty_conditional_body")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UselessTimes",
            r#"
            1.times { |i| do_something(i) }
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Useless call to `1.times` detected.
            "#,
        )
        .id("lint_useless_times")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/MixedRegexpCaptureTypes",
            r#"
            /(?<foo>bar)(baz)/
            ^^^^^^^^^^^^^^^^^^ Do not mix named captures and numbered captures in a Regexp literal.
            "#,
        )
        .id("lint_mixed_regexp_capture_types")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/OutOfRangeRegexpRef",
            r#"
            "foo" =~ /(f)oo/
            puts $2
                 ^^ $2 is out of range (1 regexp capture group detected).
            "#,
        )
        .id("lint_out_of_range_regexp_ref")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/FlipFlop",
            r#"
            if (1..2)
                ^^^^ Avoid the use of flip-flop operators.
              do_something
            end
            "#,
        )
        .id("lint_flip_flop")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UselessDefined",
            r#"
            defined?("foo")
            ^^^^^^^^^^^^^^^ Calling `defined?` with a string argument will always return a truthy value.
            "#,
        )
        .id("lint_useless_defined")
        .severity(Severity::Warning)
        .correctable(false),
        // `_1 = 1` は 3.0 以降では構文エラーなので、ハーネス既定の 2.7 でしか届かない。
        CopCase::annotated(
            "Lint/NumberedParameterAssignment",
            r#"
            _1 = 1
            ^^^^^^ `_1` is reserved for numbered parameter; consider another name.
            "#,
        )
        .id("lint_numbered_parameter_assignment")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/OrAssignmentToConstant",
            r#"
            CONST ||= 1
                  ^^^ Avoid using or-assignment with constants.
            "#,
        )
        .id("lint_or_assignment_to_constant")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/LambdaWithoutLiteralBlock",
            r#"
            lambda(&block)
            ^^^^^^^^^^^^^^ lambda without a literal block is deprecated; use the proc without lambda instead.
            "#,
        )
        .id("lint_lambda_without_literal_block")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/EmptyClass",
            r#"
            class Foo
            ^^^^^^^^^ Empty class detected.
            end
            "#,
        )
        .id("lint_empty_class")
        .severity(Severity::Warning)
        .locations(&[(1, 1, 2, 3)])
        .lengths(&[13])
        .correctable(false),
        CopCase::annotated(
            "Lint/AmbiguousAssignment",
            r#"
            x =- 1
              ^^ Suspicious assignment detected. Did you mean `-=`?
            "#,
        )
        .id("lint_ambiguous_assignment")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/ConstantOverwrittenInRescue",
            r#"
            begin
              x
            rescue => CONST
                   ^^ `CONST` is overwritten by `rescue =>`.
            end
            "#,
        )
        .id("lint_constant_overwritten_in_rescue")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/HashNewWithKeywordArgumentsAsDefault",
            r#"
            Hash.new(a: 1)
                     ^^^^ Use a hash literal instead of keyword arguments.
            "#,
        )
        .id("lint_hash_new_with_keyword_arguments_as_default")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/SharedMutableDefault",
            r#"
            Hash.new([])
            ^^^^^^^^^^^^ Do not create a Hash with a mutable default value as the default value can accidentally be changed.
            "#,
        )
        .id("lint_shared_mutable_default")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/TripleQuotes",
            r#"
            x = """foo"""
                ^^^^^^^^^ Delimiting a string with multiple quotes has no effect, use a single quote instead.
            "#,
        )
        .id("lint_triple_quotes")
        .severity(Severity::Warning)
        .correctable(true),
        // `minimum_target_ruby_version 3.1`。ハーネス既定の 2.7 では登録すらされない。
        CopCase::annotated(
            "Lint/RefinementImportMethods",
            r#"
            module M
              refine Foo do
                include Bar
                ^^^^^^^ Use `import_methods` instead of `include` because it was removed in Ruby 3.2.
              end
            end
            "#,
        )
        .id("lint_refinement_import_methods")
        .target_ruby("3.2")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/DeprecatedConstants",
            r#"
            NIL
            ^^^ Use `nil` instead of `NIL`, deprecated since Ruby 2.4.
            "#,
        )
        .id("lint_deprecated_constants")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/DataDefineOverride",
            r#"
            Data.define(:hash)
                        ^^^^^ `:hash` member overrides `Data#hash` and it may be unexpected.
            "#,
        )
        .id("lint_data_define_override")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UselessOr",
            r#"
            x.to_s || 'default'
                   ^^^^^^^^^^^^ `'default'` will never evaluate because `x.to_s` always returns a truthy value.
            "#,
        )
        .id("lint_useless_or")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/RequireRelativeSelfPath",
            r#"
            require_relative 'b12'
            ^^^^^^^^^^^^^^^^^^^^^^ Remove the `require_relative` that requires itself.
            "#,
        )
        .id("lint_require_relative_self_path")
        .path("b12.rb")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/EmptyBlock",
            r#"
            foo {}
            ^^^^^^ Empty block detected.
            "#,
        )
        .id("lint_empty_block")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/EmptyInPattern",
            r#"
            case x
            in 1
            ^^^^ Avoid `in` branches without a body.
            end
            "#,
        )
        .id("lint_empty_in_pattern")
        .target_ruby("3.0")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/DuplicateMatchPattern",
            r#"
            case x
            in 1
              a
            in 1
               ^ Duplicate `in` pattern detected.
              b
            end
            "#,
        )
        .id("lint_duplicate_match_pattern")
        .target_ruby("3.0")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/NoReturnInBeginEndBlocks",
            r#"
            x = begin
              return 1 if a
              ^^^^^^^^ Do not `return` in `begin..end` blocks in assignment contexts.
              0
            end
            "#,
        )
        .id("lint_no_return_in_begin_end_blocks")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/RedundantDirGlobSort",
            r#"
            Dir.glob('*').sort
                          ^^^^ Remove redundant `sort`.
            "#,
        )
        .id("lint_redundant_dir_glob_sort")
        .target_ruby("3.0")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/DuplicateSetElement",
            r#"
            Set[1, 2, 1]
                      ^ Remove the duplicate element in Set.
            "#,
        )
        .id("lint_duplicate_set_element")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/UselessNumericOperation",
            r#"
            foo + 0
            ^^^^^^^ Do not apply inconsequential numeric operations to variables.
            "#,
        )
        .id("lint_useless_numeric_operation")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/NumericOperationWithConstantResult",
            r#"
            foo * 0
            ^^^^^^^ Numeric operation with a constant result detected.
            "#,
        )
        .id("lint_numeric_operation_with_constant_result")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/AmbiguousRange",
            r#"
            v = 1..a + b
                   ^^^^^ Wrap complex range boundaries with parentheses to avoid ambiguity.
            "#,
        )
        .id("lint_ambiguous_range")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/DuplicateMagicComment",
            r#"
            # encoding: utf-8
            # encoding: ascii
            ^^^^^^^^^^^^^^^^^ Duplicate magic comment detected.
            x = 1
            "#,
        )
        .id("lint_duplicate_magic_comment")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/IncompatibleIoSelectWithFiberScheduler",
            r#"
            IO.select([io], nil)
            ^^^^^^^^^^^^^^^^^^^^ Use `io.wait_readable` instead of `IO.select([io], nil)`.
            "#,
        )
        .id("lint_incompatible_io_select_with_fiber_scheduler")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/AmbiguousOperatorPrecedence",
            r#"
            a = 1 + 2 * 3
                    ^^^^^ Wrap expressions with varying precedence with parentheses to avoid ambiguity.
            "#,
        )
        .id("lint_ambiguous_operator_precedence")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/UnreachablePatternBranch",
            r#"
            case x
            in y
            in 1
            ^^^^ Unreachable `in` pattern branch detected.
            end
            "#,
        )
        .id("lint_unreachable_pattern_branch")
        .target_ruby("3.0")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UselessDefaultValueArgument",
            r#"
            h.fetch(:k, 0) { 1 }
                        ^ Block supersedes default value argument.
            "#,
        )
        .id("lint_useless_default_value_argument")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/ItWithoutArgumentsInBlock",
            r#"
            [1].each { it }
                       ^^ `it` calls without arguments will refer to the first block param in Ruby 3.4; use `it()` or `self.it`.
            "#,
        )
        .id("lint_it_without_arguments_in_block")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UnexpectedBlockArity",
            r#"
            x.reduce { |a| a }
            ^^^^^^^^^^^^^^^^^^ `reduce` expects at least 2 positional arguments, got 1.
            "#,
        )
        .id("lint_unexpected_block_arity")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/LiteralAssignmentInCondition",
            r#"
            if x = 1
                 ^^^ Don't use literal assignment `= 1` in conditional, should be `==` or non-literal operand.
            end
            "#,
        )
        .id("lint_literal_assignment_in_condition")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UselessRescue",
            r#"
            begin
              x
            rescue
            ^^^^^^ Useless `rescue` detected.
              raise
            end
            "#,
        )
        .id("lint_useless_rescue")
        .severity(Severity::Warning)
        .locations(&[(3, 1, 4, 7)])
        .lengths(&[14])
        .correctable(false),
        // 既定では無効な cop。設定で入れたうえで本家の出力と突き合わせる。
        CopCase::annotated(
            "Lint/ConstantResolution",
            r#"
            Foo
            ^^^ Fully qualify this constant to avoid possibly ambiguous resolution.
            "#,
        )
        .id("lint_constant_resolution")
        .config("Lint/ConstantResolution:\n  Enabled: true\n")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/ArrayLiteralInRegexp",
            r#"
            /#{[1, 2]}/
             ^^^^^^^^^ Use a character class instead of interpolating an array in a regexp.
            "#,
        )
        .id("lint_array_literal_in_regexp")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/SuppressedExceptionInNumberConversion",
            r#"
            Integer('1') rescue nil
            ^^^^^^^^^^^^^^^^^^^^^^^ Use `Integer('1', exception: false)` instead.
            "#,
        )
        .id("lint_suppressed_exception_in_number_conversion")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/UselessRuby2Keywords",
            r#"
            ruby2_keywords def qux(a); end
            ^^^^^^^^^^^^^^ `ruby2_keywords` is unnecessary for method `qux`.
            "#,
        )
        .id("lint_useless_ruby2_keywords")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UselessConstantScoping",
            r#"
            class C
              private
              FOO = 1
              ^^^^^^^ Useless `private` access modifier for constant scope.
            end
            "#,
        )
        .id("lint_useless_constant_scoping")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/CopDirectiveSyntax",
            r#"
            # rubocop:disable
            ^^^^^^^^^^^^^^^^^ Malformed directive comment detected. The cop name is missing.
            "#,
        )
        .id("lint_cop_directive_syntax")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/DuplicateBranch",
            r#"
            if a
              foo
            else
            ^^^^ Duplicate branch body detected.
              foo
            end
            "#,
        )
        .id("lint_duplicate_branch")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/NonAtomicFileOperation",
            r#"
            unless File.exist?(path)
            ^^^^^^^^^^^^^^^^^^^^^^^^ Remove unnecessary existence check `File.exist?`.
              FileUtils.mkdir_p(path)
            end
            "#,
        )
        .id("lint_non_atomic_file_operation")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/ConstantReassignment",
            r#"
            FOO = 1
            FOO = 2
            ^^^^^^^ Constant `FOO` is already assigned in this namespace.
            "#,
        )
        .id("lint_constant_reassignment")
        .severity(Severity::Warning)
        .correctable(false),
        // 本家は `AllCops: UseProjectIndex` を立てて `rubydex` を入れたときしか索引を
        // 持たず、既定では何も報告しない。
        CopCase::new(
            "Lint/DeprecatedReference",
            "# @deprecated Use bar instead.\ndef foo; end\nfoo\n",
            Vec::new(),
        )
        .id("lint_deprecated_reference"),
        // 既定では無効な cop。設定で入れたうえで本家の出力と突き合わせる。
        CopCase::annotated(
            "Lint/NumberConversion",
            r#"
            "10".to_i
            ^^^^^^^^^ Replace unsafe number conversion with number class parsing, instead of using `"10".to_i`, use stricter `Integer("10", 10)`.
            "#,
        )
        .id("lint_number_conversion")
        .config("Lint/NumberConversion:\n  Enabled: true\n")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/RedundantTypeConversion",
            r#"
            "foo".to_s
                  ^^^^ Redundant `to_s` detected.
            "#,
        )
        .id("lint_redundant_type_conversion")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/SymbolConversion",
            r#"
            "foo".to_sym
            ^^^^^^^^^^^^ Unnecessary symbol conversion; use `:foo` instead.
            "#,
        )
        .id("lint_symbol_conversion")
        .severity(Severity::Warning)
        .correctable(true),
        CopCase::annotated(
            "Lint/ToEnumArguments",
            r#"
            def bar(x)
              return to_enum(:bar) unless block_given?
                     ^^^^^^^^^^^^^ Ensure you correctly provided all the arguments.
            end
            "#,
        )
        .id("lint_to_enum_arguments")
        .severity(Severity::Warning)
        .correctable(false),
        CopCase::annotated(
            "Lint/UnmodifiedReduceAccumulator",
            r#"
            values.reduce({}) do |acc, el|
              el
              ^^ Ensure the accumulator `acc` will be modified by `reduce`.
            end
            "#,
        )
        .id("lint_unmodified_reduce_accumulator")
        .severity(Severity::Warning)
        .correctable(false),
        // どちらも `AllCops: UseProjectIndex` + `rubydex` のときしか索引を持たず、既定では
        // 何も報告しない。
        CopCase::new(
            "Lint/UnusedPrivateMethod",
            "class C\n  private\n  def unused_one; end\nend\n",
            Vec::new(),
        )
        .id("lint_unused_private_method"),
        CopCase::new("Lint/NameTypo", "Foo::Barr\nFoo.barr\n", Vec::new()).id("lint_name_typo"),
        // 既定では無効な cop。設定で入れたうえで本家の出力と突き合わせる。
        CopCase::annotated(
            "Lint/HeredocMethodCallPosition",
            r#"
            x = <<~SQL
              bar
            SQL
              .strip
            ^ Put a method call with a HEREDOC receiver on the same line as the HEREDOC opening.
            "#,
        )
        .id("lint_heredoc_method_call_position")
        .config("Lint/HeredocMethodCallPosition:\n  Enabled: true\n")
        .severity(Severity::Warning)
        .correctable(true),
        // 既定では無効な cop。設定で入れたうえで本家の出力と突き合わせる。
        CopCase::annotated(
            "Lint/ShadowingOuterLocalVariable",
            r#"
            def m
              x = 1
              [1].each { |x| }
                          ^ Shadowing outer local variable - `x`.
            end
            "#,
        )
        .id("lint_shadowing_outer_local_variable")
        .config("Lint/ShadowingOuterLocalVariable:\n  Enabled: true\n")
        .severity(Severity::Warning)
        .correctable(false),
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
            "Metrics/CollectionLiteralLength",
            "[1, 2, 3]\n^^^^^^^^^ Avoid hard coding large quantities of data in code. Prefer reading the data from an external source.\n",
        )
        .id("metrics_collection_literal_length")
        .config("Metrics/CollectionLiteralLength:\n  Max: 3\n")
        .correctable(false),
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
            "Naming/BlockForwarding",
            r#"
            def foo(&block)
                    ^^^^^^ Use anonymous block forwarding.
              bar(&block)
                  ^^^^^^ Use anonymous block forwarding.
            end
            "#,
        )
        .id("naming_block_forwarding")
        .target_ruby("3.1")
        .correctable(true),
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
        // 既定無効。`CheckFilepaths` はテスト用の一時ディレクトリを読むので切っておく。
        CopCase::annotated(
            "Naming/InclusiveLanguage",
            "whitelist = 1\n^^^^^^^^^ Consider replacing 'whitelist' with 'allowlist' or 'permit'.\n",
        )
        .id("naming_inclusive_language")
        .config("Naming/InclusiveLanguage:\n  CheckFilepaths: false\n")
        .correctable(false),
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
            "Naming/PredicateMethod",
            r#"
            def foo
                ^^^ Predicate method names should end with `?`.
              x == y
            end
            "#,
        )
        .id("naming_predicate_method")
        .correctable(false),
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
            "Security/CompoundHash",
            r#"
            def hash
              a.hash ^ b.hash
              ^^^^^^^^^^^^^^^ Use `[...].hash` instead of combining hash values manually.
            end
            "#,
        )
        .id("security_compound_hash")
        .correctable(false),
        CopCase::annotated(
            "Security/IoMethods",
            "IO.read('x')\n^^^^^^^^^^^^ `File.read` is safer than `IO.read`.\n",
        )
        .id("security_io_methods")
        .correctable(true),
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
        // Perl 由来の特殊変数。`English` を要する名前かどうかで文面が変わる。
        CopCase::annotated(
            "Style/SpecialGlobalVars",
            r#"
            puts $!
                 ^^ Prefer `$ERROR_INFO` from the stdlib 'English' module (don't forget to require it) over `$!`.
            "#,
        )
        .id("style_special_global_vars")
        .correctable(true),
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
        // 空の定義は 1 行に。行で見るコメント判定に引っかからないもののみ。
        CopCase::annotated(
            "Style/EmptyMethod",
            r#"
            def foo(bar)
            ^^^^^^^^^^^^ Put empty method definitions on a single line.
            end
            "#,
        )
        .id("style_empty_method")
        .correctable(true),
        // 単線の lambda は `->` で書く。報告は send のレンジだけ。
        CopCase::annotated(
            "Style/Lambda",
            r#"
            a = lambda { |x| x }
                ^^^^^^ Use the `-> { ... }` lambda literal syntax for single line lambdas.
            "#,
        )
        .id("style_lambda")
        .correctable(true),
        // 大文字の基数接頭辞は小文字へ。
        CopCase::annotated(
            "Style/NumericLiteralPrefix",
            r#"
            a = 0O1234
                ^^^^^^ Use 0o for octal literals.
            "#,
        )
        .id("style_numeric_literal_prefix")
        .correctable(true),
        // `== 0` は述語で書ける。既定は unsafe なので `-A` でだけ直る。
        CopCase::annotated(
            "Style/NumericPredicate",
            r#"
            a = foo == 0
                ^^^^^^^^ Use `foo.zero?` instead of `foo == 0`.
            "#,
        )
        .id("style_numeric_predicate")
        .correctable(true),
        // Perl 由来の後方参照は `Regexp.last_match` へ。
        CopCase::annotated(
            "Style/PerlBackrefs",
            r#"
            puts $1
                 ^^ Prefer `Regexp.last_match(1)` over `$1`.
            "#,
        )
        .id("style_perl_backrefs")
        .correctable(true),
        // 連鎖の頂点で 1 度だけ報告する。
        CopCase::annotated(
            "Style/StringConcatenation",
            r#"
            a = 'x' + y + 'z'
                ^^^^^^^^^^^^^ Prefer string interpolation to string concatenation.
            "#,
        )
        .id("style_string_concatenation")
        .correctable(true),
        // 配列末尾の裸のハッシュは波括弧で包む。
        CopCase::annotated(
            "Style/HashAsLastArrayItem",
            r#"
            a = [1, 2, one: 1, two: 2]
                       ^^^^^^^^^^^^^^ Wrap hash in `{` and `}`.
            "#,
        )
        .id("style_hash_as_last_array_item")
        .correctable(true),
        // 修飾形の `rescue` は式全体を報告する。代入の中では代入の右辺だけが対象。
        CopCase::annotated(
            "Style/RescueModifier",
            r#"
            x.foo rescue nil
            ^^^^^^^^^^^^^^^^ Avoid using `rescue` in its modifier form.
            "#,
        )
        .id("style_rescue_modifier")
        .correctable(true),
        // 値を受け渡す位置では 1 行に畳み、そうでなければ `if` に開く。
        CopCase::annotated(
            "Style/MultilineTernaryOperator",
            r#"
            x = cond ?
                ^^^^^^ Avoid multi-line ternary operators, use `if` or `unless` instead.
              a :
              b
            "#,
        )
        .id("style_multiline_ternary_operator")
        .correctable(true),
        // `else` の中の `if` は `elsif`。報告は内側の `if` キーワード。
        CopCase::annotated(
            "Style/IfInsideElse",
            r#"
            if a
              x
            else
              if b
              ^^ Convert `if` nested inside `else` to `elsif`.
                y
              end
            end
            "#,
        )
        .id("style_if_inside_else")
        .correctable(true),
        // どの分岐でも同じことをするなら外へ。分岐ごとに 1 件報告する。
        CopCase::annotated(
            "Style/IdenticalConditionalBranches",
            r#"
            if a
              do_x
              foo
              ^^^ Move `foo` out of the conditional.
            else
              do_y
              foo
              ^^^ Move `foo` out of the conditional.
            end
            "#,
        )
        .id("style_identical_conditional_branches")
        .correctable(true),
        // 既定の assign_to_condition では条件そのものを報告する。
        CopCase::annotated(
            "Style/ConditionalAssignment",
            r#"
            if foo
            ^^^^^^ Use the return of the conditional for variable assignment and comparison.
              bar = 1
            else
              bar = 2
            end
            "#,
        )
        .id("style_conditional_assignment")
        .correctable(true),
        // 同じコレクションを 2 度回すループ。報告は 2 つ目のループ全体。
        CopCase::annotated(
            "Style/CombinableLoops",
            r#"
            def m
              items.each { |x| foo(x) }
              items.each { |x| bar(x) }
              ^^^^^^^^^^^^^^^^^^^^^^^^^ Combine this loop with the previous loop.
            end
            "#,
        )
        .id("style_combinable_loops")
        .correctable(true),
        // 多重代入の末尾の `_`。報告は消える範囲そのもの。
        CopCase::annotated(
            "Style/TrailingUnderscoreVariable",
            r#"
            a, b, _ = foo()
                  ^^ Do not use trailing `_`s in parallel assignment. Prefer `a, b, = foo()`.
            "#,
        )
        .id("style_trailing_underscore_variable")
        .correctable(true),
        // 補間 1 つだけの文字列。報告は文字列リテラル全体。
        CopCase::annotated(
            "Style/RedundantInterpolation",
            r##"
            c = "#{foo}"
                ^^^^^^^^ Prefer `to_s` over string interpolation.
            "##,
        )
        .id("style_redundant_interpolation")
        .correctable(true),
        // 既定の explicit では、クラスを名指ししない `rescue` を報告する。
        CopCase::annotated(
            "Style/RescueStandardError",
            r#"
            begin
              foo
            rescue
            ^^^^^^ Avoid rescuing without specifying an error class.
              bar
            end
            "#,
        )
        .id("style_rescue_standard_error")
        .correctable(true),
        // 標準ストリームは定数ではなくグローバル変数で綴る。
        CopCase::annotated(
            "Style/GlobalStdStream",
            r#"
            STDOUT.puts 'a'
            ^^^^^^ Use `$stdout` instead of `STDOUT`.
            "#,
        )
        .id("style_global_std_stream")
        .correctable(true),
        // 既定の short では `has_key?` の側を報告する。
        CopCase::annotated(
            "Style/PreferredHashMethods",
            r#"
            h.has_key?(:a)
              ^^^^^^^^ Use `Hash#key?` instead of `Hash#has_key?`.
            "#,
        )
        .id("style_preferred_hash_methods")
        .correctable(true),
        CopCase::annotated(
            "Style/Proc",
            r#"
            p = Proc.new { |n| n }
                ^^^^^^^^ Use `proc` instead of `Proc.new`.
            "#,
        )
        .id("style_proc")
        .correctable(true),
        CopCase::annotated(
            "Style/ClassVars",
            r#"
            @@test = 10
            ^^^^^^ Replace class var @@test with a class instance var.
            "#,
        )
        .id("style_class_vars")
        .correctable(false),
        CopCase::annotated(
            "Style/OptionalBooleanParameter",
            r#"
            def some_method(bar = false)
                            ^^^^^^^^^^^ Prefer keyword arguments for arguments with a boolean default value; use `bar: false` instead of `bar = false`.
              bar
            end
            "#,
        )
        .id("style_optional_boolean_parameter")
        .correctable(false),
        CopCase::annotated(
            "Style/StabbyLambdaParentheses",
            r#"
            f = ->a, b { a + b }
                  ^^^^ Wrap stabby lambda arguments with parentheses.
            "#,
        )
        .id("style_stabby_lambda_parentheses")
        .correctable(true),
        CopCase::annotated(
            "Style/LambdaCall",
            r#"
            h = f.(1, 2)
                ^^^^^^^^ Prefer the use of `f.call(1, 2)` over `f.(1, 2)`.
            "#,
        )
        .id("style_lambda_call")
        .correctable(true),
        // 修飾形も対象。ブロック形は `if` から `end` までを報告する。
        CopCase::annotated(
            "Style/NegatedIf",
            r#"
            z if !w
            ^^^^^^^ Favor `unless` over `if` for negative conditions.
            "#,
        )
        .id("style_negated_if")
        .correctable(true),
        CopCase::annotated(
            "Style/SymbolLiteral",
            r##"
            :"foo"
            ^^^^^^ Do not use strings for word-like symbol literals.
            "##,
        )
        .id("style_symbol_literal")
        .correctable(true),
        // ブロックの `{` から `}` までを報告する。`&:sym` は本文が呼ぶメソッド名。
        CopCase::annotated(
            "Style/SymbolProc",
            r#"
            something.map { |s| s.upcase }
                          ^^^^^^^^^^^^^^^^ Pass `&:upcase` as an argument to `map` instead of a block.
            "#,
        )
        .id("style_symbol_proc")
        .correctable(true),
        CopCase::annotated(
            "Style/WhenThen",
            r#"
            case n
            when 1; puts 1
                  ^ Do not use `when 1;`. Use `when 1 then` instead.
            end
            "#,
        )
        .id("style_when_then")
        .correctable(true),
        CopCase::annotated(
            "Style/ClassCheck",
            r#"
            n.kind_of?(Integer)
              ^^^^^^^^ Prefer `Object#is_a?` over `Object#kind_of?`.
            "#,
        )
        .id("style_class_check")
        .correctable(true),
        CopCase::annotated(
            "Style/StderrPuts",
            r#"
            $stderr.puts 'oops'
            ^^^^^^^^^^^^ Use `warn` instead of `$stderr.puts` to allow such output to be disabled.
            "#,
        )
        .id("style_stderr_puts")
        .correctable(true),
        CopCase::annotated(
            "Style/FormatString",
            r#"
            puts sprintf('%10s', 'foo')
                 ^^^^^^^ Favor `format` over `sprintf`.
            "#,
        )
        .id("style_format_string")
        .correctable(true),
        CopCase::annotated(
            "Style/BarePercentLiterals",
            r#"
            a = %Q(hi)
                ^^^ Use `%` instead of `%Q`.
            "#,
        )
        .id("style_bare_percent_literals")
        .correctable(true),
        CopCase::annotated(
            "Style/PercentQLiterals",
            r#"
            b = %Q(hi)
                ^^^ Do not use `%Q` unless interpolation is needed. Use `%q`.
            "#,
        )
        .id("style_percent_q_literals")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantPercentQ",
            r#"
            c = %q(hi)
                ^^^^^^ Use `%q` only for strings that contain both single quotes and double quotes.
            "#,
        )
        .id("style_redundant_percent_q")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantCapitalW",
            r#"
            d = %W[a b]
                ^^^^^^^ Do not use `%W` unless interpolation is needed. If not, use `%w`.
            "#,
        )
        .id("style_redundant_capital_w")
        .correctable(true),
        CopCase::annotated(
            "Style/RaiseArgs",
            r#"
            raise RuntimeError.new('msg')
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Provide an exception class and message as arguments to `raise`.
            "#,
        )
        .id("style_raise_args")
        .correctable(true),
        CopCase::annotated(
            "Style/ZeroLengthPredicate",
            r#"
            x = a.size.zero?
                  ^^^^^^^^^^ Use `empty?` instead of `size.zero?`.
            "#,
        )
        .id("style_zero_length_predicate")
        .correctable(true),
        // 同じスコープに `respond_to_missing?` があれば咎めない。
        CopCase::annotated(
            "Style/MissingRespondToMissing",
            r#"
            class Q
              def method_missing(name)
              ^^^^^^^^^^^^^^^^^^^^^^^^ When using `method_missing`, define `respond_to_missing?`.
                nil
              end
            end
            "#,
        )
        .id("style_missing_respond_to_missing")
        .correctable(false),
        CopCase::annotated(
            "Style/ArrayJoin",
            r#"
            a = [1, 2] * ', '
                       ^ Favor `Array#join` over `Array#*`.
            "#,
        )
        .id("style_array_join")
        .correctable(true),
        CopCase::annotated(
            "Style/CharacterLiteral",
            r#"
            c = ?a
                ^^ Do not use the character literal - use string literal instead.
            "#,
        )
        .id("style_character_literal")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantException",
            r#"
            raise RuntimeError, 'msg'
            ^^^^^^^^^^^^^^^^^^^^^^^^^ Redundant `RuntimeError` argument can be removed.
            "#,
        )
        .id("style_redundant_exception")
        .correctable(true),
        CopCase::annotated(
            "Style/VariableInterpolation",
            r##"
            e = "#@foo"
                  ^^^^ Replace interpolated variable `@foo` with expression `#{@foo}`.
            "##,
        )
        .id("style_variable_interpolation")
        .correctable(true),
        CopCase::annotated(
            "Style/BeginBlock",
            r#"
            BEGIN { test }
            ^^^^^ Avoid the use of `BEGIN` blocks.
            "#,
        )
        .id("style_begin_block")
        .correctable(false),
        CopCase::annotated(
            "Style/EndBlock",
            r#"
            END { puts 'x' }
            ^^^ Avoid the use of `END` blocks. Use `Kernel#at_exit` instead.
            "#,
        )
        .id("style_end_block")
        .correctable(true),
        CopCase::annotated(
            "Style/EnvHome",
            r#"
            ENV['HOME']
            ^^^^^^^^^^^ Use `Dir.home` instead.
            "#,
        )
        .id("style_env_home")
        .correctable(true),
        // 位置は selector から呼び出しの末尾まで。レシーバは置き換えの外に残る。
        CopCase::annotated(
            "Style/NestedFileDirname",
            "File.dirname(File.dirname(path))\n",
        )
        .id("style_nested_file_dirname")
        .target_ruby("3.1")
        .without_offense_check()
        .locations(&[(1, 6, 1, 32)])
        .lengths(&[27])
        .correctable(true),
        // 位置は selector から呼び出しの末尾まで。`[element]` は括弧ごと消える。
        CopCase::annotated(
            "Style/ArrayIntersectWithSingleElement",
            "array.intersect?([1])\n      ^^^^^^^^^^^^^^^ Use `include?(element)` instead of `intersect?([element])`.\n",
        )
        .id("style_array_intersect_with_single_element")
        .correctable(true),
        CopCase::annotated(
            "Style/DirEmpty",
            "Dir.children(path).empty?\n^^^^^^^^^^^^^^^^^^^^^^^^^ Use `Dir.empty?(path)` instead.\n",
        )
        .id("style_dir_empty")
        .correctable(true),
        CopCase::annotated(
            "Style/FileEmpty",
            "File.zero?(path)\n^^^^^^^^^^^^^^^^ Use `File.empty?(path)` instead.\n",
        )
        .id("style_file_empty")
        .correctable(true),
        // 報告されるのは `block` ノード、つまり呼び出しとブロックを合わせた範囲。
        CopCase::annotated(
            "Style/FileTouch",
            "File.open(filename, 'a') {}\n^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `FileUtils.touch(filename)` instead of `File.open` in append mode with empty block.\n",
        )
        .id("style_file_touch")
        .correctable(true),
        // 位置はヒアドキュメントの開始記号だけ。本体と終端行は補正で消える。
        CopCase::annotated(
            "Style/EmptyHeredoc",
            "x = <<~MSG\n    ^^^^^^ Use an empty string literal instead of heredoc.\nMSG\n",
        )
        .id("style_empty_heredoc")
        .correctable(true),
        CopCase::annotated(
            "Style/ModuleMemberExistenceCheck",
            "Foo.instance_methods.include?(:bar)\n    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `method_defined?(:bar)` instead.\n",
        )
        .id("style_module_member_existence_check")
        .correctable(true),
        // 位置は `if` 全体。複数行なので注記では表せない。
        CopCase::annotated("Style/MinMaxComparison", "if a > b\n  a\nelse\n  b\nend\n")
            .id("style_min_max_comparison")
            .without_offense_check()
            .locations(&[(1, 1, 5, 3)])
            .lengths(&[25])
            .correctable(true),
        CopCase::annotated(
            "Style/ComparableBetween",
            "x >= 1 && x <= 10\n^^^^^^^^^^^^^^^^^ Prefer `x.between?(1, 10)` over logical comparison.\n",
        )
        .id("style_comparable_between")
        .correctable(true),
        CopCase::annotated(
            "Style/ExactRegexpMatch",
            "foo.match?(/\\Abar\\z/)\n^^^^^^^^^^^^^^^^^^^^^ Use `foo == 'bar'`.\n",
        )
        .id("style_exact_regexp_match")
        .correctable(true),
        // 位置は一番内側の `dig` の selector から連鎖の末尾まで。
        CopCase::annotated(
            "Style/DigChain",
            "x.dig(:a).dig(:b)\n  ^^^^^^^^^^^^^^^ Use `dig(:a, :b)` instead of chaining.\n",
        )
        .id("style_dig_chain")
        .correctable(true),
        CopCase::annotated(
            "Style/HashFetchChain",
            "x.fetch(:a, nil).fetch(:b, nil)\n  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `dig(:a, :b)` instead.\n",
        )
        .id("style_hash_fetch_chain")
        .correctable(true),
        CopCase::annotated(
            "Style/ConcatArrayLiterals",
            "a.concat([1, 2])\n  ^^^^^^^^^^^^^^ Use `push(1, 2)` instead of `concat([1, 2])`.\n",
        )
        .id("style_concat_array_literals")
        .correctable(true),
        CopCase::annotated(
            "Style/FileNull",
            "x = '/dev/null'\n    ^^^^^^^^^^^ Use `File::NULL` instead of `/dev/null`.\n",
        )
        .id("style_file_null")
        .correctable(true),
        // 補正を持たない cop なので `[Correctable]` は付かない。
        CopCase::annotated(
            "Style/FileOpen",
            "File.open('f')\n^^^^^^^^^^^^^^ `File.open` without a block may leak a file descriptor; use the block form.\n",
        )
        .id("style_file_open")
        .correctable(false),
        // 報告されるのは読み出しの側。置き換えは `open` の selector から始まる。
        CopCase::annotated(
            "Style/FileRead",
            "File.open(filename).read\n^^^^^^^^^^^^^^^^^^^^^^^^ Use `File.read`.\n",
        )
        .id("style_file_read")
        .correctable(true),
        CopCase::annotated(
            "Style/FileWrite",
            "File.open(filename, 'w') { |f| f.write(content) }\n^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `File.write`.\n",
        )
        .id("style_file_write")
        .correctable(true),
        // 位置は `;` 1 文字だけ。
        CopCase::annotated("Style/InPatternThen", "case x\nin 1; foo\n    ^ Do not use `in 1;`. Use `in 1 then` instead.\nend\n")
            .id("style_in_pattern_then")
            .correctable(true),
        CopCase::annotated(
            "Style/MultilineInPatternThen",
            "case x\nin 1 then\n     ^^^^ Do not use `then` for multiline `in` statement.\n  foo\nend\n",
        )
        .id("style_multiline_in_pattern_then")
        .correctable(true),
        CopCase::annotated(
            "Style/ItAssignment",
            "it = 5\n^^ `it` is the default block parameter; consider another name.\n",
        )
        .id("style_it_assignment")
        .correctable(false),
        CopCase::annotated(
            "Style/BitwisePredicate",
            "(x & 0b0100).positive?\n^^^^^^^^^^^^^^^^^^^^^^ Replace with `x.anybits?(0b0100)` for comparison with bit flags.\n",
        )
        .id("style_bitwise_predicate")
        .correctable(true),
        // 位置は selector からブロックの末尾まで。レシーバは置き換えの外に残る。
        CopCase::annotated(
            "Style/CollectionCompact",
            "array.reject { |e| e.nil? }\n      ^^^^^^^^^^^^^^^^^^^^^ Use `compact` instead of `reject { |e| e.nil? }`.\n",
        )
        .id("style_collection_compact")
        .correctable(true),
        // 位置は `if` 全体。複数行なので注記では表せない。
        CopCase::annotated(
            "Style/ComparableClamp",
            "if x < min\n  min\nelsif max < x\n  max\nelse\n  x\nend\n",
        )
        .id("style_comparable_clamp")
        .without_offense_check()
        .locations(&[(1, 1, 7, 3)])
        .lengths(&[49])
        .correctable(true),
        // 位置は `map` の selector だけ。
        CopCase::annotated(
            "Style/MapJoin",
            "x.map(&:to_s).join\n  ^^^ Remove redundant `map(&:to_s)` before `join`.\n",
        )
        .id("style_map_join")
        .correctable(true),
        CopCase::annotated(
            "Style/MapToHash",
            "x.map { |a| [a, a] }.to_h\n  ^^^ Pass a block to `to_h` instead of calling `map.to_h`.\n",
        )
        .id("style_map_to_hash")
        .correctable(true),
        CopCase::annotated(
            "Style/MapToSet",
            "x.map { |a| a * 2 }.to_set\n  ^^^ Pass a block to `to_set` instead of calling `map.to_set`.\n",
        )
        .id("style_map_to_set")
        .correctable(true),
        // 位置は親クラスとして書かれた `Data.define(...)` だけ。
        CopCase::annotated(
            "Style/DataInheritance",
            "class Point < Data.define(:x, :y)\n              ^^^^^^^^^^^^^^^^^^^ Don't extend an instance initialized by `Data.define`. Use a block to customize the class.\nend\n",
        )
        .id("style_data_inheritance")
        .target_ruby("3.2")
        .correctable(true),
        CopCase::annotated(
            "Style/EmptyClassDefinition",
            "Foo = Class.new(Bar)\n^^^^^^^^^^^^^^^^^^^^ Use the `class` keyword instead of `Class.new` to define an empty class.\n",
        )
        .id("style_empty_class_definition")
        .correctable(true),
        CopCase::annotated(
            "Style/CombinableDefined",
            "defined?(Foo) && defined?(Foo::Bar)\n^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Combine nested `defined?` calls.\n",
        )
        .id("style_combinable_defined")
        .correctable(true),
        // 位置は selector からブロックの末尾まで。レシーバは置き換えの外に残る。
        CopCase::annotated(
            "Style/HashExcept",
            "hash.reject { |k, v| k == :foo }\n     ^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `except(:foo)` instead.\n",
        )
        .id("style_hash_except")
        .target_ruby("3.0")
        .correctable(true),
        CopCase::annotated(
            "Style/HashSlice",
            "hash.select { |k, v| keys.include?(k) }\n     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `slice(*keys)` instead.\n",
        )
        .id("style_hash_slice")
        .correctable(true),
        // 位置は `count` の selector から比較の末尾まで。
        CopCase::annotated(
            "Style/CollectionQuerying",
            "x.count { |e| e > 2 }.positive?\n  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `any?` instead.\n",
        )
        .id("style_collection_querying")
        .correctable(true),
        CopCase::annotated(
            "Style/ArrayIntersect",
            "(a & b).any?\n^^^^^^^^^^^^ Use `a.intersect?(b)` instead of `(a & b).any?`.\n",
        )
        .id("style_array_intersect")
        .target_ruby("3.1")
        .correctable(true),
        // 位置は条件式全体。補間の `#{` と `}` は置き換えの外に残る。
        CopCase::annotated(
            "Style/EmptyStringInsideInterpolation",
            "\"#{x ? 'foo' : ''}\"\n   ^^^^^^^^^^^^^^ Do not return empty strings in string interpolation.\n",
        )
        .id("style_empty_string_inside_interpolation")
        .correctable(true),
        // 位置は `merge` の呼び出し。置き換えるのは `**` から始まる引数の方。
        CopCase::annotated(
            "Style/KeywordArgumentsMerging",
            "foo(**opts.merge(a: 1))\n      ^^^^^^^^^^^^^^^^ Provide additional arguments directly rather than using `merge`.\n",
        )
        .id("style_keyword_arguments_merging")
        .correctable(true),
        // 位置はキーワードだけ。三項では条件の末尾から式の末尾まで。
        CopCase::annotated(
            "Style/IfWithBooleanLiteralBranches",
            "if foo.do_something?\n^^ Remove redundant `if` with boolean literal branches.\n  true\nelse\n  false\nend\n",
        )
        .id("style_if_with_boolean_literal_branches")
        .correctable(true),
        CopCase::annotated(
            "Style/SuperWithArgsParentheses",
            r"
            super bar, baz
            ^^^^^^^^^^^^^^ Use parentheses for `super` with arguments.
            ",
        )
        .id("style_super_with_args_parentheses")
        .correctable(true),
        // 位置は selector から呼び出しの末尾まで。レシーバは置き換えの外に残る。
        CopCase::annotated(
            "Style/StringChars",
            r#"
            "x".split('')
                ^^^^^^^^^ Use `chars` instead of `split('')`.
            "#,
        )
        .id("style_string_chars")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantRegexpConstructor",
            r"
            Regexp.new(/foo/im)
            ^^^^^^^^^^^^^^^^^^^ Remove the redundant `Regexp.new`.
            ",
        )
        .id("style_redundant_regexp_constructor")
        .correctable(true),
        // 位置はドットから `flatten` 呼び出しの末尾まで。
        CopCase::annotated(
            "Style/RedundantArrayFlatten",
            r"
            x.flatten(1).join
             ^^^^^^^^^^^ Remove the redundant `flatten`.
            ",
        )
        .id("style_redundant_array_flatten")
        .correctable(true),
        CopCase::annotated(
            "Style/SafeNavigationChainLength",
            r"
            x&.a&.b&.c
            ^^^^^^^^^^ Avoid safe navigation chains longer than 2 calls.
            ",
        )
        .id("style_safe_navigation_chain_length")
        .correctable(false),
        // 位置は先頭の `./` だけ。
        CopCase::annotated(
            "Style/RedundantCurrentDirectoryInPath",
            r"
            require_relative './foo'
                              ^^ Remove the redundant current directory path.
            ",
        )
        .id("style_redundant_current_directory_in_path")
        .correctable(true),
        // 位置は selector から呼び出しの末尾まで。`YAML.` は外に残る。
        CopCase::annotated(
            "Style/YAMLFileRead",
            r"
            YAML.load(File.read(path))
                 ^^^^^^^^^^^^^^^^^^^^^ Use `load_file(path)` instead.
            ",
        )
        .id("style_yaml_file_read")
        .correctable(true),
        CopCase::annotated(
            "Style/OpenStructUse",
            r"
            OpenStruct.new
            ^^^^^^^^^^ Avoid using `OpenStruct`; use `Struct`, `Hash`, a class or test doubles instead.
            ",
        )
        .id("style_open_struct_use")
        .correctable(false),
        CopCase::annotated(
            "Style/ObjectThen",
            r"
            foo.yield_self { |y| y }
                ^^^^^^^^^^ Prefer `then` over `yield_self`.
            ",
        )
        .id("style_object_then")
        .correctable(true),
        // 位置は開き側の `<<~'FOO'` 全体。
        CopCase::annotated(
            "Style/RedundantHeredocDelimiterQuotes",
            r"
            h = <<~'FOO'
                ^^^^^^^^ Remove the redundant heredoc delimiter quotes, use `<<~FOO` instead.
              plain
            FOO
            ",
        )
        .id("style_redundant_heredoc_delimiter_quotes")
        .correctable(true),
        // 位置は `String.new` まで。引数のリテラルは外に残る。
        CopCase::annotated(
            "Style/RedundantInterpolationUnfreeze",
            r##"
            u = String.new("#{a}")
                ^^^^^^^^^^ Don't unfreeze interpolated strings as they are already unfrozen.
            "##,
        )
        .id("style_redundant_interpolation_unfreeze")
        .target_ruby("3.0")
        .correctable(true),
        // 位置はブロック全体 (上流の `block` ノード)。
        CopCase::annotated(
            "Style/NilLambda",
            r"
            a = -> { nil }
                ^^^^^^^^^^ Use an empty lambda instead of always returning nil.
            ",
        )
        .id("style_nil_lambda")
        .correctable(true),
        // 位置はレシーバから selector まで。
        CopCase::annotated(
            "Style/RedundantArrayConstructor",
            r"
            Array.new([1, 2])
            ^^^^^^^^^ Remove the redundant `Array` constructor.
            ",
        )
        .id("style_redundant_array_constructor")
        .correctable(true),
        // `rfind` は Ruby 4.0 で入る。
        CopCase::annotated(
            "Style/ReverseFind",
            r"
            x.reverse.find { |y| y }
              ^^^^^^^^^^^^ Use `rfind` instead.
            ",
        )
        .id("style_reverse_find")
        .target_ruby("4.0")
        .correctable(true),
        // 位置は 1 つ目の代入だけ。
        CopCase::annotated(
            "Style/SwapValues",
            r"
            tmp = x
            ^^^^^^^ Replace this and assignments at lines 2 and 3 with `x, y = y, x`.
            x = y
            y = tmp
            ",
        )
        .id("style_swap_values")
        .correctable(true),
        // 位置は selector だけ。
        CopCase::annotated(
            "Style/Send",
            r"
            foo.send(:bar)
                ^^^^ Prefer `Object#__send__` or `Object#public_send` to `send`.
            ",
        )
        .id("style_send")
        .correctable(false),
        CopCase::annotated(
            "Style/ImplicitRuntimeError",
            r"
            raise 'boom'
            ^^^^^^^^^^^^ Use `raise` with an explicit exception class and message, rather than just a message.
            ",
        )
        .id("style_implicit_runtime_error")
        .correctable(false),
        CopCase::annotated(
            "Style/StringMethods",
            r"
            'x'.intern
                ^^^^^^ Prefer `to_sym` over `intern`.
            ",
        )
        .id("style_string_methods")
        .correctable(true),
        CopCase::annotated(
            "Style/InlineComment",
            r"
            x = 1 # trailing
                  ^^^^^^^^^^ Avoid trailing inline comments.
            ",
        )
        .id("style_inline_comment")
        .correctable(false),
        // 位置は最初の非 ASCII の連続だけ。長さは文字数。
        CopCase::annotated(
            "Style/AsciiComments",
            "# mixed \u{a9}\u{3068}\u{65e5}\u{672c}\u{8a9e}\n",
        )
        .id("style_ascii_comments")
        .without_offense_check()
        .locations(&[(1, 9, 1, 13)])
        .lengths(&[5])
        .correctable(false),
        // 位置は `end` から呼び出しの末尾まで。
        CopCase::annotated(
            "Style/MethodCalledOnDoEndBlock",
            r"
            foo do
              bar
            end.baz
            ^^^^^^^ Avoid chaining a method call on a do...end block.
            ",
        )
        .id("style_method_called_on_do_end_block")
        .correctable(false),
        CopCase::annotated(
            "Style/TopLevelMethodDefinition",
            r"
            def self.top2; end
            ^^^^^^^^^^^^^^^^^^ Do not define methods at the top-level.
            ",
        )
        .id("style_top_level_method_definition")
        .correctable(false),
        CopCase::annotated(
            "Style/DateTime",
            r"
            DateTime.now
            ^^^^^^^^^^^^ Prefer `Time` over `DateTime`.
            ",
        )
        .id("style_date_time")
        .correctable(true),
        CopCase::annotated(
            "Style/CollectionMethods",
            r"
            arr.collect { |x| x }
                ^^^^^^^ Prefer `map` over `collect`.
            ",
        )
        .id("style_collection_methods")
        .correctable(true),
        CopCase::annotated(
            "Style/YodaExpression",
            r"
            p1 = 1 + a
                 ^^^^^ Non-literal operand (`a`) should be first.
            ",
        )
        .id("style_yoda_expression")
        .correctable(true),
        CopCase::annotated(
            "Style/ArrayCoercion",
            r"
            q1 = [*foo]
                 ^^^^^^ Use `Array(foo)` instead of `[*foo]`.
            ",
        )
        .id("style_array_coercion")
        .correctable(true),
        // 位置は `::` の 2 文字だけ。
        CopCase::annotated(
            "Style/RedundantConstantBase",
            r"
            ::Foo
            ^^ Remove redundant `::`.
            ",
        )
        .id("style_redundant_constant_base")
        .correctable(true),
        // 位置は `each` とその後ろのドット。
        CopCase::annotated(
            "Style/RedundantEach",
            r"
            xs.each.each_with_index { |x, i| x }
               ^^^^^ Remove redundant `each`.
            ",
        )
        .id("style_redundant_each")
        .correctable(true),
        // 位置は `select` から述語 selector の末尾まで。
        CopCase::annotated(
            "Style/RedundantFilterChain",
            r"
            ys.select { |y| y }.any?
               ^^^^^^^^^^^^^^^^^^^^^ Use `any?` instead of `select.any?`.
            ",
        )
        .id("style_redundant_filter_chain")
        .correctable(true),
        CopCase::annotated(
            "Style/UnlessLogicalOperators",
            r"
            y unless a && b || c
            ^^^^^^^^^^^^^^^^^^^^ Do not use mixed logical operators in an `unless`.
            ",
        )
        .id("style_unless_logical_operators")
        .correctable(false),
        CopCase::annotated(
            "Style/ConstantVisibility",
            r"
            class Foo
              BAZ = 2
              ^^^^^^^ Explicitly make `BAZ` public or private using either `#public_constant` or `#private_constant`.
            end
            ",
        )
        .id("style_constant_visibility")
        .correctable(false),
        // 位置は最後のカンマ 1 文字。
        CopCase::annotated(
            "Style/TrailingCommaInBlockArgs",
            r"
            foo { |a, b,| a }
                       ^ Useless trailing comma present in block arguments.
            ",
        )
        .id("style_trailing_comma_in_block_args")
        .correctable(true),
        // 位置は縦棒を含む引数リスト全体。
        CopCase::annotated(
            "Style/SingleLineBlockParams",
            r"
            xs.reduce { |a, e| a + e }
                        ^^^^^^ Name `reduce` block params `|acc, elem|`.
            ",
        )
        .id("style_single_line_block_params")
        .correctable(true),
        // 既定 (`allow_single_line`) では複数行のブロックだけが対象。
        CopCase::annotated("Style/NumberedParameters", "xs.map do\n  _1\nend\n")
            .id("style_numbered_parameters")
            .without_offense_check()
            .locations(&[(1, 1, 3, 3)])
            .lengths(&[18])
            .correctable(false),
        CopCase::annotated(
            "Style/NumberedParametersLimit",
            r"
            xs.map { _1 + _2 }
            ^^^^^^^^^^^^^^^^^^ Avoid using more than 1 numbered parameter; 2 detected.
            ",
        )
        .id("style_numbered_parameters_limit")
        .correctable(false),
        // 位置は selector からブロックの閉じまで。
        CopCase::annotated(
            "Style/RedundantMinMaxBy",
            r"
            xs.max_by { |x| x }
               ^^^^^^^^^^^^^^^^ Use `max` instead of `max_by { |x| x }`.
            ",
        )
        .id("style_redundant_min_max_by")
        .correctable(true),
        CopCase::annotated(
            "Style/PredicateWithKind",
            r"
            xs.any? { |x| x.is_a?(Foo) }
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Prefer `any?(Foo)` to `any? { ... }` with a kind check.
            ",
        )
        .id("style_predicate_with_kind")
        .correctable(true),
        CopCase::annotated(
            "Style/ReturnNil",
            r"
            def m
              return nil
              ^^^^^^^^^^ Use `return` instead of `return nil`.
            end
            ",
        )
        .id("style_return_nil")
        .correctable(true),
        // 位置は selector だけ。置き換えはドットから末尾まで。
        CopCase::annotated(
            "Style/HashLookupMethod",
            r"
            a = h.fetch(:k)
                  ^^^^^ Use `Hash#[]` instead of `Hash#fetch`.
            ",
        )
        .id("style_hash_lookup_method")
        .correctable(true),
        CopCase::annotated(
            "Style/AutoResourceCleanup",
            r"
            f = File.open('x')
                ^^^^^^^^^^^^^^ Use the block version of `File.open`.
            ",
        )
        .id("style_auto_resource_cleanup")
        .correctable(false),
        // 位置は末尾の引数だけ。
        CopCase::annotated(
            "Style/OptionHash",
            r"
            def m(options = {}); end
                  ^^^^^^^^^^^^ Prefer keyword arguments to options hashes.
            ",
        )
        .id("style_option_hash")
        .correctable(false),
        // 位置はキーワードから名前の末尾まで。2 つ目以降が対象。
        CopCase::annotated(
            "Style/OneClassPerFile",
            r"
            class A; end
            module B; end
            ^^^^^^^^ Do not define multiple classes/modules at the top level in a single file.
            ",
        )
        .id("style_one_class_per_file")
        .correctable(false),
        // 位置は selector から末尾まで。
        CopCase::annotated(
            "Style/SendWithLiteralMethodName",
            r"
            foo.public_send(:bar)
                ^^^^^^^^^^^^^^^^^ Use `bar` method call directly instead.
            ",
        )
        .id("style_send_with_literal_method_name")
        .correctable(true),
        // 位置は自己代入している枝だけ。
        CopCase::annotated(
            "Style/RedundantSelfAssignmentBranch",
            r"
            x = if c
              x
              ^ Remove the self-assignment branch.
            else
              y
            end
            ",
        )
        .id("style_redundant_self_assignment_branch")
        .correctable(true),
        // 位置は `keyword_init:` のペア。
        CopCase::annotated(
            "Style/RedundantStructKeywordInit",
            r"
            S1 = Struct.new(:a, keyword_init: true)
                                ^^^^^^^^^^^^^^^^^^ Remove the redundant `keyword_init: true`.
            ",
        )
        .id("style_redundant_struct_keyword_init")
        .target_ruby("3.2")
        .correctable(true),
        // 位置はファイル先頭の 1 文字。既定の `AutocorrectNotice` は空なので補正は付かない。
        CopCase::annotated("Style/Copyright", "x = 1\n")
            .id("style_copyright")
            .without_offense_check()
            .locations(&[(1, 1, 1, 1)])
            .lengths(&[1])
            .correctable(false),
        CopCase::annotated(
            "Style/IpAddresses",
            r"
            a = '1.2.3.4'
                ^^^^^^^^^ Do not hardcode IP addresses.
            ",
        )
        .id("style_ip_addresses")
        .correctable(false),
        CopCase::annotated(
            "Style/ReturnNilInPredicateMethodDefinition",
            r"
            def foo?
              return if bar
              ^^^^^^ Return `false` instead of `nil` in predicate methods.
              baz
            end
            ",
        )
        .id("style_return_nil_in_predicate_method_definition")
        .correctable(true),
        // 位置は定義全体。複数行なので注記では表せない。
        CopCase::annotated("Style/EndlessMethod", "def m =\n  42\n")
            .id("style_endless_method")
            .target_ruby("3.0")
            .without_offense_check()
            .locations(&[(1, 1, 2, 4)])
            .lengths(&[12])
            .correctable(true),
        // 位置は修飾子を含む式全体。置き換わるのは定義の側。
        CopCase::annotated(
            "Style/AmbiguousEndlessMethodDefinition",
            "def m = 42 if cond\n^^^^^^^^^^^^^^^^^^ Avoid using `if` statements with endless methods.\n",
        )
        .id("style_ambiguous_endless_method_definition")
        .target_ruby("3.0")
        .correctable(true),
        CopCase::annotated(
            "Style/HashConversion",
            "Hash[ary]\n^^^^^^^^^ Prefer `ary.to_h` to `Hash[ary]`.\n",
        )
        .id("style_hash_conversion")
        .correctable(true),
        // 位置は `_1` の読みそのもの。
        CopCase::annotated(
            "Style/ItBlockParameter",
            "foo { _1 * 2 }\n      ^^ Use `it` block parameter.\n",
        )
        .id("style_it_block_parameter")
        .target_ruby("3.4")
        .correctable(true),
        CopCase::annotated(
            "Style/FetchEnvVar",
            "ENV['X']\n^^^^^^^^ Use `ENV.fetch('X', nil)` instead of `ENV['X']`.\n",
        )
        .id("style_fetch_env_var")
        .correctable(true),
        // 位置は `map` の selector から `.compact` の末尾まで。
        CopCase::annotated(
            "Style/MapCompactWithConditionalBlock",
            "ary.map { |x| x if cond(x) }.compact\n    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Replace `map { ... }.compact` with `select`.\n",
        )
        .id("style_map_compact_with_conditional_block")
        .correctable(true),
        // 位置は selector だけ。補正は無い。
        CopCase::annotated(
            "Style/DocumentDynamicEvalDefinition",
            "class_eval <<~RUBY\n^^^^^^^^^^ Add a comment block showing its appearance if interpolated.\n  def #{name}\n  end\nRUBY\n",
        )
        .id("style_document_dynamic_eval_definition")
        .correctable(false),
        // 位置はディレクティブの綴りだけ。値と `#` は範囲の外に残る。
        CopCase::annotated(
            "Style/MagicCommentFormat",
            "# frozen-string-literal: true\n  ^^^^^^^^^^^^^^^^^^^^^ Prefer lower snake case for magic comments.\nx = 1\n",
        )
        .id("style_magic_comment_format")
        .correctable(true),
        // 位置は `each` のブロック全体。
        CopCase::annotated(
            "Style/MapIntoArray",
            "dest = []\nsrc.each { |e| dest << e * 2 }\n^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `map` instead of `each` to map elements into an array.\ndest\n",
        )
        .id("style_map_into_array")
        .correctable(true),
        // 位置は鍵の文字列だけ。
        CopCase::annotated(
            "Style/StringHashKeys",
            r"
            { 'a' => 1 }
              ^^^ Prefer symbols instead of strings as hash keys.
            ",
        )
        .id("style_string_hash_keys")
        .correctable(true),
        // 位置は `[...]` の部分。
        CopCase::annotated(
            "Style/ArrayFirstLast",
            r"
            a[0]
             ^^^ Use `first`.
            ",
        )
        .id("style_array_first_last")
        .correctable(true),
        // 位置は定義全体。
        CopCase::annotated("Style/RedundantInitialize", "class A\n  def initialize\n  end\nend\n")
            .id("style_redundant_initialize")
            .without_offense_check()
            .locations(&[(2, 3, 3, 5)])
            .lengths(&[20])
            .correctable(true),
        // 位置は添字の式だけ。
        CopCase::annotated(
            "Style/NegativeArrayIndex",
            r"
            arr[arr.length - 1]
                ^^^^^^^^^^^^^^ Use `arr[-1]` instead of `arr[arr.length - 1]`.
            ",
        )
        .id("style_negative_array_index")
        .correctable(true),
        // 位置はクラス全体。
        CopCase::annotated("Style/StaticClass", "class A\n  def self.foo; end\nend\n")
            .id("style_static_class")
            .without_offense_check()
            .locations(&[(1, 1, 3, 3)])
            .lengths(&[31])
            .correctable(true),
        // 位置は括弧ごと。
        CopCase::annotated(
            "Style/RedundantArgument",
            r"
            a.join('')
                  ^^^^ Argument '' is redundant because it is implied by default.
            ",
        )
        .id("style_redundant_argument")
        .correctable(true),
        CopCase::annotated(
            "Style/QuotedSymbols",
            r#"
            :"a"
            ^^^^ Prefer single-quoted symbols when you don't need string interpolation or special symbols.
            "#,
        )
        .id("style_quoted_symbols")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantRegexpArgument",
            r"
            'a'.split(/,/)
                      ^^^ Use string `','` as argument instead of regexp `/,/`.
            ",
        )
        .id("style_redundant_regexp_argument")
        .correctable(true),
        // 位置はドット 1 文字。
        CopCase::annotated(
            "Style/OperatorMethodCall",
            r"
            foo.+(bar)
               ^ Redundant dot detected.
            ",
        )
        .id("style_operator_method_call")
        .correctable(true),
        // 位置は selector。
        CopCase::annotated(
            "Style/ReduceToHash",
            r"
            xs.each_with_object({}) { |e, h| h[e] = e * 2 }
               ^^^^^^^^^^^^^^^^ Use `to_h { ... }` instead of `each_with_object`.
            ",
        )
        .id("style_reduce_to_hash")
        .correctable(true),
        CopCase::annotated(
            "Style/DocumentationMethod",
            r"
            def foo; end
            ^^^^^^^^^^^^ Missing method documentation comment.
            ",
        )
        .id("style_documentation_method")
        .correctable(false),
        CopCase::annotated(
            "Style/NegatedIfElseCondition",
            r"
            !x ? a : b
            ^^^^^^^^^^ Invert the negated condition and swap the ternary branches.
            ",
        )
        .id("style_negated_if_else_condition")
        .correctable(true),
        CopCase::annotated(
            "Style/InvertibleUnlessCondition",
            r"
            a unless x != y
            ^^^^^^^^^^^^^^^ Prefer `if x == y` over `unless x != y`.
            ",
        )
        .id("style_invertible_unless_condition")
        .correctable(true),
        // 位置は定義全体。
        CopCase::annotated("Style/MultilineMethodSignature", "def foo(a,\n        b)\nend\n")
            .id("style_multiline_method_signature")
            .without_offense_check()
            .locations(&[(1, 1, 3, 3)])
            .lengths(&[25])
            .correctable(true),
        // 位置はコメント全体。`=end` の行末までで、行末の改行も含む。
        CopCase::annotated(
            "Style/BlockComments",
            "=begin\nMultiple lines\n=end\nx = 1\n",
        )
        .id("style_block_comments")
        .without_offense_check()
        .locations(&[(1, 1, 4, 1)])
        .lengths(&[27])
        .correctable(true),
        CopCase::annotated(
            "Style/ClassMethods",
            r#"
            class SomeClass
              def SomeClass.class_method
                  ^^^^^^^^^ Use `self.class_method` instead of `SomeClass.class_method`.
              end
            end
            "#,
        )
        .id("style_class_methods")
        .correctable(true),
        CopCase::annotated(
            "Style/ColonMethodCall",
            r#"
            Timeout::timeout(500) { do_something }
                   ^^ Do not use `::` for method calls.
            "#,
        )
        .id("style_colon_method_call")
        .correctable(true),
        CopCase::annotated(
            "Style/ColonMethodDefinition",
            r#"
            def self::bar
                    ^^ Do not use `::` for defining class methods.
            end
            "#,
        )
        .id("style_colon_method_definition")
        .correctable(true),
        CopCase::annotated(
            "Style/DefWithParentheses",
            r#"
            def foo()
                   ^^ Omit the parentheses in defs when the method doesn't accept any arguments.
              do_something
            end
            "#,
        )
        .id("style_def_with_parentheses")
        .correctable(true),
        CopCase::annotated(
            "Style/EachForSimpleLoop",
            r#"
            (1..5).each { }
            ^^^^^^^^^^^ Use `Integer#times` for a simple loop which iterates a fixed number of times.
            "#,
        )
        .id("style_each_for_simple_loop")
        .correctable(true),
        CopCase::annotated(
            "Style/CaseLikeIf",
            r#"
            if x == 1
            ^^^^^^^^^ Convert `if-elsif` to `case-when`.
              a
            elsif x == 2
              b
            elsif x == 3
              c
            end
            "#,
        )
        .id("style_case_like_if")
        .locations(&[(1, 1, 7, 3)])
        .correctable(true),
        CopCase::annotated(
            "Style/RandomWithOffset",
            r#"
            a = 1 + rand(6)
                ^^^^^^^^^^^ Prefer ranges when generating random numbers instead of integers with offsets.
            "#,
        )
        .id("style_random_with_offset")
        .correctable(true),
        CopCase::annotated(
            "Style/NonNilCheck",
            r#"
            y = x != nil
                ^^^^^^^^ Prefer `!x.nil?` over `x != nil`.
            "#,
        )
        .id("style_non_nil_check")
        .correctable(true),
        CopCase::annotated(
            "Style/NestedTernaryOperator",
            r#"
            x = a ? b : c ? d : e
                        ^^^^^^^^^ Ternary operators must not be nested. Prefer `if` or `else` constructs instead.
            "#,
        )
        .id("style_nested_ternary_operator")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantAssignment",
            r#"
            def foo
              x = compute
              ^^^^^^^^^^^ Redundant assignment before returning detected.
              x
            end
            "#,
        )
        .id("style_redundant_assignment")
        .correctable(true),
        CopCase::annotated(
            "Style/EmptyCaseCondition",
            r#"
            case
            ^^^^ Do not use empty `case` condition, instead use an `if` expression.
            when a
              b
            end
            puts 1
            "#,
        )
        .id("style_empty_case_condition")
        .correctable(true),
        CopCase::annotated(
            "Style/EmptyElse",
            r#"
            if a
              b
            else
            ^^^^ Redundant `else`-clause.
            end
            "#,
        )
        .id("style_empty_else")
        .correctable(true),
        CopCase::annotated(
            "Style/StructInheritance",
            r#"
            class Person < Struct.new(:a, :b)
                           ^^^^^^^^^^^^^^^^^^ Don't extend an instance initialized by `Struct.new`. Use a block to customize the struct.
              def age
                42
              end
            end
            "#,
        )
        .id("style_struct_inheritance")
        .correctable(true),
        CopCase::annotated(
            "Style/WhileUntilModifier",
            r#"
            while a
            ^^^^^ Favor modifier `while` usage when having a single-line body.
              b
            end
            "#,
        )
        .id("style_while_until_modifier")
        .correctable(true),
        CopCase::annotated(
            "Style/CommandLiteral",
            r#"
            a = %x(ls)
                ^^^^^^ Use backticks around command string.
            "#,
        )
        .id("style_command_literal")
        .correctable(true),
        CopCase::annotated(
            "Style/DoubleNegation",
            r#"
            x = !!y
                ^ Avoid the use of double negation (`!!`).
            "#,
        )
        .id("style_double_negation")
        .correctable(true),
        CopCase::annotated(
            "Style/ClassEqualityComparison",
            r#"
            x.class == Foo
              ^^^^^^^^^^^^ Use `instance_of?(Foo)` instead of comparing classes.
            "#,
        )
        .id("style_class_equality_comparison")
        .correctable(true),
        CopCase::annotated(
            "Style/EmptyBlockParameter",
            r#"
            a do ||
                 ^^ Omit pipes for the empty block parameters.
              do_something
            end
            "#,
        )
        .id("style_empty_block_parameter")
        .correctable(true),
        CopCase::annotated(
            "Style/EmptyLambdaParameter",
            r#"
            -> () { do_something }
               ^^ Omit parentheses for the empty lambda parameters.
            "#,
        )
        .id("style_empty_lambda_parameter")
        .correctable(true),
        CopCase::annotated(
            "Style/UnlessElse",
            r#"
            unless foo
            ^^^^^^^^^^ Do not use `unless` with `else`. Rewrite these with the positive case first.
              a
            else
              b
            end
            "#,
        )
        .id("style_unless_else")
        .locations(&[(1, 1, 5, 3)])
        .correctable(true),
        CopCase::annotated(
            "Style/WhileUntilDo",
            r#"
            while x.any? do
                         ^^ Do not use `do` with multi-line `while`.
              do_something(x.pop)
            end
            "#,
        )
        .id("style_while_until_do")
        .correctable(true),
        CopCase::annotated(
            "Style/MultilineIfThen",
            r#"
            if cond then
                    ^^^^ Do not use `then` for multi-line `if`.
              a
            end
            "#,
        )
        .id("style_multiline_if_then")
        .correctable(true),
        CopCase::annotated(
            "Style/MultilineWhenThen",
            r#"
            case foo
            when bar then
                     ^^^^ Do not use `then` for multiline `when` statement.
            end
            "#,
        )
        .id("style_multiline_when_then")
        .correctable(true),
        CopCase::annotated(
            "Style/NegatedWhile",
            r#"
            while !foo
            ^^^^^^^^^^ Favor `until` over `while` for negative conditions.
              bar
            end
            "#,
        )
        .id("style_negated_while")
        .locations(&[(1, 1, 3, 3)])
        .correctable(true),
        CopCase::annotated(
            "Style/NegatedUnless",
            r#"
            unless !foo
            ^^^^^^^^^^^ Favor `if` over `unless` for negative conditions.
              bar
            end
            "#,
        )
        .id("style_negated_unless")
        .locations(&[(1, 1, 3, 3)])
        .correctable(true),
        CopCase::annotated(
            "Style/Not",
            r#"
            x = (not something)
                 ^^^ Use `!` instead of `not`.
            "#,
        )
        .id("style_not")
        .correctable(true),
        // 反転メソッドがあるなら否定しない。報告は否定式全体。
        CopCase::annotated(
            "Style/InverseMethods",
            r#"
            a = !foo.any?
                ^^^^^^^^^ Use `none?` instead of inverting `any?`.
            "#,
        )
        .id("style_inverse_methods")
        .correctable(true),
        CopCase::annotated(
            "Style/MinMax",
            r#"
            bar = [foo.min, foo.max]
                  ^^^^^^^^^^^^^^^^^^ Use `foo.minmax` instead of `[foo.min, foo.max]`.
            "#,
        )
        .id("style_min_max")
        .correctable(true),
        CopCase::annotated(
            "Style/MultilineMemoization",
            r#"
            foo ||= (
            ^^^^^^^^^ Wrap multiline memoization blocks in `begin` and `end`.
              bar
              baz
            )
            "#,
        )
        .id("style_multiline_memoization")
        .locations(&[(1, 1, 4, 1)])
        .correctable(true),
        CopCase::annotated(
            "Style/IfUnlessModifierOfIfUnless",
            r#"
            'stop' if tired? if running?
                             ^^ Avoid modifier `if` after another conditional.
            "#,
        )
        .id("style_if_unless_modifier_of_if_unless")
        .correctable(true),
        CopCase::annotated(
            "Style/Strip",
            r#"
            'abc'.lstrip.rstrip
                  ^^^^^^^^^^^^^ Use `strip` instead of `lstrip.rstrip`.
            "#,
        )
        .id("style_strip")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantSortBy",
            r#"
            array.sort_by { |x| x }
                  ^^^^^^^^^^^^^^^^^ Use `sort` instead of `sort_by { |x| x }`.
            "#,
        )
        .id("style_redundant_sort_by")
        .correctable(true),
        CopCase::annotated(
            "Style/DoubleCopDisableDirective",
            r#"
            def f # rubocop:disable Style/For # rubocop:disable Metrics/AbcSize
                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ More than one disable comment on one line.
            end
            "#,
        )
        .id("style_double_cop_disable_directive")
        .correctable(true),
        CopCase::annotated(
            "Style/TrailingMethodEndStatement",
            r#"
            def some_method
            do_stuff; end
                      ^^^ Place the end statement of a multi-line method on its own line.
            "#,
        )
        .id("style_trailing_method_end_statement")
        .correctable(true),
        CopCase::annotated(
            "Style/TrailingBodyOnClass",
            r#"
            class Foo; def foo; end
                       ^^^^^^^^^^^^ Place the first line of class body on its own line.
            end
            "#,
        )
        .id("style_trailing_body_on_class")
        .correctable(true),
        CopCase::annotated(
            "Style/TrailingBodyOnModule",
            r#"
            module Bar extend self
                       ^^^^^^^^^^^ Place the first line of module body on its own line.
            end
            "#,
        )
        .id("style_trailing_body_on_module")
        .correctable(true),
        CopCase::annotated(
            "Style/TrailingBodyOnMethodDefinition",
            r#"
            def g(x); b = foo
                      ^^^^^^^ Place the first line of a multi-line method definition's body on its own line.
              b[c: x]
            end
            "#,
        )
        .id("style_trailing_body_on_method_definition")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantConditional",
            r#"
            z = (x == y ? true : false)
                 ^^^^^^^^^^^^^^^^^^^^^ This conditional expression can just be replaced by `x == y`.
            "#,
        )
        .id("style_redundant_conditional")
        .correctable(true),
        CopCase::annotated(
            "Style/NilComparison",
            r#"
            if x == nil
                 ^^ Prefer the use of the `nil?` predicate.
            end
            "#,
        )
        .id("style_nil_comparison")
        .correctable(true),
        CopCase::annotated(
            "Style/SingleArgumentDig",
            r#"
            [1, 2, 3].dig(0)
            ^^^^^^^^^^^^^^^^ Use `[1, 2, 3][0]` instead of `[1, 2, 3].dig(0)`.
            "#,
        )
        .id("style_single_argument_dig")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantFileExtensionInRequire",
            r#"
            require 'foo.rb'
                        ^^^ Redundant `.rb` file extension detected.
            "#,
        )
        .id("style_redundant_file_extension_in_require")
        .correctable(true),
        CopCase::annotated(
            "Style/UnpackFirst",
            r#"
            'foo'.unpack('h*').first
                  ^^^^^^^^^^^^^^^^^^ Use `unpack1('h*')` instead of `unpack('h*').first`.
            "#,
        )
        .id("style_unpack_first")
        .correctable(true),
        CopCase::annotated(
            "Style/Dir",
            r#"
            path = File.expand_path(File.dirname(__FILE__))
                   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Use `__dir__` to get an absolute path to the current file's directory.
            "#,
        )
        .id("style_dir")
        .correctable(true),
        CopCase::annotated(
            "Style/Attr",
            r#"
            class K
              attr :something, true
              ^^^^ Do not use `attr`. Use `attr_accessor` instead.
            end
            "#,
        )
        .id("style_attr")
        .correctable(true),
        // 既定の grouped では、まとめられる宣言のそれぞれを報告する。
        CopCase::annotated(
            "Style/AccessorGrouping",
            r#"
            class K
              attr_reader :bar
              ^^^^^^^^^^^^^^^^ Group together all `attr_reader` attributes.
              attr_reader :baz
              ^^^^^^^^^^^^^^^^ Group together all `attr_reader` attributes.
            end
            "#,
        )
        .id("style_accessor_grouping")
        .correctable(true),
        CopCase::annotated(
            "Style/NestedParenthesizedCalls",
            r#"
            method1(method2 arg)
                    ^^^^^^^^^^^ Add parentheses to nested method call `method2 arg`.
            "#,
        )
        .id("style_nested_parenthesized_calls")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantSelfAssignment",
            r#"
            args = args.concat(ary)
                 ^ Redundant self assignment detected. Method `concat` modifies its receiver in place.
            "#,
        )
        .id("style_redundant_self_assignment")
        .correctable(true),
        CopCase::annotated(
            "Style/ExpandPathArguments",
            r#"
            File.expand_path('..', __FILE__)
                 ^^^^^^^^^^^ Use `expand_path(__dir__)` instead of `expand_path('..', __FILE__)`.
            "#,
        )
        .id("style_expand_path_arguments")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantSort",
            r#"
            [2, 1, 3].sort.first
                      ^^^^^^^^^^ Use `min` instead of `sort...first`.
            "#,
        )
        .id("style_redundant_sort")
        .correctable(true),
        CopCase::annotated(
            "Style/OrAssignment",
            r#"
            name = name ? name : 'B'
            ^^^^^^^^^^^^^^^^^^^^^^^^ Use the double pipe equals operator `||=` instead.
            "#,
        )
        .id("style_or_assignment")
        .correctable(true),
        CopCase::annotated(
            "Style/EvenOdd",
            r#"
            if a % 2 == 0
               ^^^^^^^^^^ Replace with `Integer#even?`.
            end
            "#,
        )
        .id("style_even_odd")
        .correctable(true),
        CopCase::annotated(
            "Style/ExponentialNotation",
            r#"
            10e6
            ^^^^ Use a mantissa >= 1 and < 10.
            "#,
        )
        .id("style_exponential_notation")
        .correctable(false),
        CopCase::annotated(
            "Style/MixinUsage",
            r#"
            include M
            ^^^^^^^^^ `include` is used at the top level. Use inside `class` or `module`.
            "#,
        )
        .id("style_mixin_usage")
        .correctable(false),
        // 既定の separated では 1 文 1 モジュール。
        CopCase::annotated(
            "Style/MixinGrouping",
            r#"
            class Foo
              include Bar, Baz
              ^^^^^^^^^^^^^^^^ Put `include` mixins in separate statements.
            end
            "#,
        )
        .id("style_mixin_grouping")
        .correctable(true),
        CopCase::annotated(
            "Style/HashLikeCase",
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
        )
        .id("style_hash_like_case")
        .locations(&[(1, 1, 8, 3)])
        .correctable(false),
        CopCase::annotated(
            "Style/SelfAssignment",
            r#"
            x = x + 1
            ^^^^^^^^^ Use self-assignment shorthand `+=`.
            "#,
        )
        .id("style_self_assignment")
        .correctable(true),
        CopCase::annotated(
            "Style/SlicingWithRange",
            r#"
            items[1..-1]
                 ^^^^^^^ Prefer `[1..]` over `[1..-1]`.
            "#,
        )
        .id("style_slicing_with_range")
        .correctable(true),
        CopCase::annotated(
            "Style/MethodCallWithoutArgsParentheses",
            r#"
            foo.bar()
                   ^^ Do not use parentheses for method calls with no arguments.
            "#,
        )
        .id("style_method_call_without_args_parentheses")
        .correctable(true),
        CopCase::annotated(
            "Style/KeywordParametersOrder",
            r#"
            def m(a: 1, b:)
                  ^^^^ Place optional keyword parameters at the end of the parameters list.
              1
            end
            "#,
        )
        .id("style_keyword_parameters_order")
        .correctable(true),
        CopCase::annotated(
            "Style/MultilineBlockChain",
            r#"
            foo.each do |x|
              x
            end.map do |y|
            ^^^^^^^ Avoid multi-line chains of blocks.
              y
            end
            "#,
        )
        .id("style_multiline_block_chain")
        .correctable(false),
        CopCase::annotated(
            "Style/MultilineIfModifier",
            r#"
            do_something(1,
            ^^^^^^^^^^^^^^^ Favor a normal if-statement over a modifier clause in a multiline statement.
                         2) if condition
            "#,
        )
        .id("style_multiline_if_modifier")
        .locations(&[(1, 1, 2, 28)])
        .lengths(&[44])
        .correctable(true),
        CopCase::annotated(
            "Style/EmptyLiteral",
            r#"
            a = Array.new
                ^^^^^^^^^ Use array literal `[]` instead of `Array.new`.
            "#,
        )
        .id("style_empty_literal")
        .correctable(true),
        CopCase::annotated(
            "Style/CommentAnnotation",
            r#"
            # TODO make better
              ^^^^^ Annotation keywords like `TODO` should be all upper case, followed by a colon, and a space, then a note describing the problem.
            "#,
        )
        .id("style_comment_annotation")
        .correctable(true),
        CopCase::annotated(
            "Style/ModuleFunction",
            r#"
            module M
              extend self
              ^^^^^^^^^^^ Use `module_function` instead of `extend self`.
            end
            "#,
        )
        .id("style_module_function")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantFetchBlock",
            r#"
            h.fetch(:key) { 5 }
              ^^^^^^^^^^^^^^^^^ Use `fetch(:key, 5)` instead of `fetch(:key) { 5 }`.
            "#,
        )
        .id("style_redundant_fetch_block")
        .correctable(true),
        CopCase::annotated(
            "Style/MultipleComparison",
            r#"
            def m(x)
              x == 1 || x == 2
              ^^^^^^^^^^^^^^^^ Avoid comparing a variable with multiple items in a conditional, use `Array#include?` instead.
            end
            "#,
        )
        .id("style_multiple_comparison")
        .correctable(true),
        CopCase::annotated(
            "Style/SignalException",
            r#"
            fail 'a'
            ^^^^ Always use `raise` to signal exceptions.
            "#,
        )
        .id("style_signal_exception")
        .correctable(true),
        CopCase::annotated(
            "Style/Sample",
            r#"
            a.shuffle.first
              ^^^^^^^^^^^^^ Use `sample` instead of `shuffle.first`.
            "#,
        )
        .id("style_sample")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantFreeze",
            r#"
            :sym.freeze
            ^^^^^^^^^^^ Do not freeze immutable objects, as freezing them has no effect.
            "#,
        )
        .id("style_redundant_freeze")
        .correctable(true),
        CopCase::annotated(
            "Style/AccessModifierDeclarations",
            r#"
            class Foo
              private def bar; end
              ^^^^^^^ `private` should not be inlined in method definitions.
            end
            "#,
        )
        .id("style_access_modifier_declarations")
        .correctable(true),
        // 補正は `after_class` でまとめて 1 回積まれるので、offense 自身は
        // corrector を持たない (`correctable: false`)。
        CopCase::annotated(
            "Style/BisectedAttrAccessor",
            r#"
            class Foo
              attr_reader :bar
                          ^^^^ Combine both accessors into `attr_accessor :bar`.
              attr_writer :bar
                          ^^^^ Combine both accessors into `attr_accessor :bar`.
            end
            "#,
        )
        .id("style_bisected_attr_accessor")
        .correctable(false),
        CopCase::annotated(
            "Style/MutableConstant",
            r#"
            CONST = [1, 2, 3]
                    ^^^^^^^^^ Freeze mutable objects assigned to constants.
            "#,
        )
        .id("style_mutable_constant")
        .correctable(true),
        CopCase::annotated(
            "Style/InfiniteLoop",
            r#"
            while true
            ^^^^^ Use `Kernel#loop` for infinite loops.
              work
            end
            "#,
        )
        .id("style_infinite_loop")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantRegexpCharacterClass",
            r#"
            r = /[x]/
                 ^^^ Redundant single-element character class, `[x]` can be replaced with `x`.
            "#,
        )
        .id("style_redundant_regexp_character_class")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantSelf",
            r#"
            def foo(bar)
              self.baz
              ^^^^ Redundant `self` detected.
            end
            "#,
        )
        .id("style_redundant_self")
        .correctable(true),
        CopCase::annotated(
            "Style/LineEndConcatenation",
            r#"
            some_str = 'ala' +
                             ^ Use `\` instead of `+` to concatenate multiline strings.
                       'bala'
            "#,
        )
        .id("style_line_end_concatenation")
        .correctable(true),
        CopCase::annotated(
            "Style/IfWithSemicolon",
            r#"
            if foo; bar; end
            ^^^^^^^^^^^^^^^^ Do not use `if foo;` - use a ternary operator instead.
            "#,
        )
        .id("style_if_with_semicolon")
        .correctable(true),
        CopCase::annotated(
            "Style/MethodDefParentheses",
            r#"
            def foo a, b
                    ^^^^ Use def with parentheses when there are parameters.
            end
            "#,
        )
        .id("style_method_def_parentheses")
        .correctable(true),
        CopCase::annotated(
            "Style/For",
            r#"
            for n in [1, 2, 3] do
            ^^^^^^^^^^^^^^^^^^^^^ Prefer `each` over `for`.
              puts n
            end
            "#,
        )
        .id("style_for")
        .locations(&[(1, 1, 3, 3)])
        .correctable(true),
        CopCase::annotated(
            "Style/FloatDivision",
            r#"
            a.to_f / b.to_f
            ^^^^^^^^^^^^^^^ Prefer using `.to_f` on one side only.
            "#,
        )
        .id("style_float_division")
        .correctable(true),
        CopCase::annotated(
            "Style/NestedModifier",
            r#"
            foo if bar if baz
                ^^ Avoid using nested modifiers.
            "#,
        )
        .id("style_nested_modifier")
        .correctable(true),
        CopCase::annotated(
            "Style/ParenthesesAroundCondition",
            r#"
            if (foo)
               ^^^^^ Don't use parentheses around the condition of an `if`.
              bar
            end
            "#,
        )
        .id("style_parentheses_around_condition")
        .correctable(true),
        CopCase::annotated(
            "Style/Encoding",
            r#"
            # encoding: utf-8
            ^^^^^^^^^^^^^^^^^ Unnecessary utf-8 encoding comment.
            puts 1
            "#,
        )
        .id("style_encoding")
        .correctable(true),
        CopCase::annotated(
            "Style/EachWithObject",
            r#"
            [1, 2].inject({}) do |h, i|
                   ^^^^^^ Use `each_with_object` instead of `inject`.
              h[i] = i
              h
            end
            "#,
        )
        .id("style_each_with_object")
        .correctable(true),
        CopCase::annotated(
            "Style/HashTransformKeys",
            r#"
            {a: 1}.map { |k, v| [k.to_s, v] }.to_h
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Prefer `transform_keys` over `map {...}.to_h`.
            "#,
        )
        .id("style_hash_transform_keys")
        .correctable(true),
        CopCase::annotated(
            "Style/HashTransformValues",
            r#"
            {a: 1}.map { |k, v| [k, v.to_s] }.to_h
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Prefer `transform_values` over `map {...}.to_h`.
            "#,
        )
        .id("style_hash_transform_values")
        .correctable(true),
        CopCase::annotated(
            "Style/AndOr",
            r#"
            if foo and bar
                   ^^^ Use `&&` instead of `and`.
              x
            end
            "#,
        )
        .id("style_and_or")
        .correctable(true),
        CopCase::annotated(
            "Style/TrivialAccessors",
            r#"
            class C
              def foo
              ^^^ Use `attr_reader` to define trivial reader methods.
                @foo
              end
            end
            "#,
        )
        .id("style_trivial_accessors")
        .correctable(true),
        CopCase::annotated(
            "Style/ExplicitBlockArgument",
            r#"
            def foo
              bar { |x| yield x }
              ^^^^^^^^^^^^^^^^^^^ Consider using explicit block argument in the surrounding method's signature over `yield`.
            end
            "#,
        )
        .id("style_explicit_block_argument")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantRegexpEscape",
            r#"
            /\-/
             ^^ Redundant escape inside regexp literal
            "#,
        )
        .id("style_redundant_regexp_escape")
        .correctable(true),
        CopCase::annotated(
            "Style/OneLineConditional",
            r#"
            if foo then bar else baz end
            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ Favor the ternary operator (`?:`) over single-line `if/then/else/end` constructs.
            "#,
        )
        .id("style_one_line_conditional")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantCondition",
            r#"
            a ? a : b
              ^^^^^ Use double pipes `||` instead.
            "#,
        )
        .id("style_redundant_condition")
        .correctable(true),
        CopCase::annotated(
            "Style/YodaCondition",
            r#"
            99 == foo
            ^^^^^^^^^ Reverse the order of the operands `99 == foo`.
            "#,
        )
        .id("style_yoda_condition")
        .corrected("foo == 99\n")
        .correctable(true),
        CopCase::annotated(
            "Style/TernaryParentheses",
            r#"
            foo = (bar?) ? a : b
                  ^^^^^^^^^^^^^^ Omit parentheses for ternary conditions.
            "#,
        )
        .id("style_ternary_parentheses")
        .corrected("foo = bar? ? a : b\n")
        .correctable(true),
        CopCase::annotated(
            "Style/SoleNestedConditional",
            r#"
            if condition_a
              if condition_b
              ^^ Consider merging nested conditions into outer `if` conditions.
                do_something
              end
            end
            "#,
        )
        .id("style_sole_nested_conditional")
        .correctable(true),
        CopCase::annotated(
            "Style/EvalWithLocation",
            r#"
            C.class_eval "x = 1"
            ^^^^^^^^^^^^^^^^^^^^ Pass `__FILE__` and `__LINE__` to `class_eval`.
            "#,
        )
        .id("style_eval_with_location")
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantBegin",
            r#"
            def m
              begin
              ^^^^^ Redundant `begin` block detected.
                work
              rescue StandardError => e
                handle(e)
              end
            end
            "#,
        )
        .id("style_redundant_begin")
        .correctable(true),
        CopCase::annotated(
            "Style/HashEachMethods",
            r#"
            hash.keys.each { |k| p k }
                 ^^^^^^^^^ Use `each_key` instead of `keys.each`.
            "#,
        )
        .id("style_hash_each_methods")
        .correctable(true),
        CopCase::annotated(
            "Style/SafeNavigation",
            r#"
            foo.bar if foo
            ^^^^^^^^^^^^^^ Use safe navigation (`&.`) instead of checking if an object exists before calling the method.
            "#,
        )
        .id("style_safe_navigation")
        .correctable(true),
        CopCase::annotated(
            "Layout/HeredocArgumentClosingParenthesis",
            "foo(<<~SQL\n  text\nSQL\n)\n^ Put the closing parenthesis for a method call with a HEREDOC parameter on the same line as the HEREDOC opening.\n",
        )
        .id("layout_heredoc_argument_closing_parenthesis")
        .config("Layout/HeredocArgumentClosingParenthesis:\n  Enabled: true\n")
        .locations(&[(4, 1, 4, 1)])
        .lengths(&[1])
        .correctable(true),
        CopCase::annotated(
            "Style/RedundantParentheses",
            r#"
            x = (1)
                ^^^ Don't use parentheses around a literal.
            "#,
        )
        .id("style_redundant_parentheses")
        .corrected("x = 1\n")
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
        // `Bundler/GemFilename` の期待値は本家が絶対化したパスを丸ごと含むため、Windows では
        // ドライブレターの付いた別の文字列になる (`/tmp/example/gems.rb` →
        // `D:\tmp\example\gems.rb`、しかも走るマシンのカレントドライブ次第)。その環境の
        // 本家から期待値を取り直せないので検証から外す。cop の判定自体は OS に依存しない。
        if cfg!(windows) && case.label() == "bundler_gem_filename" {
            continue;
        }
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
