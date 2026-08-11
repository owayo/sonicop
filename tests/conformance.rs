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
        // ---- Lint ----
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
            "Lint/UnusedBlockArgument",
            r#"
            [1].each { |x| puts 1 }
                        ^ Unused block argument - `x`. You can omit the argument if you don't care about it.
            "#,
        )
        .id("lint_unused_block_argument"),
        CopCase::annotated(
            "Lint/UselessAssignment",
            r#"
            x = 1
            ^ Useless assignment to variable - `x`.
            "#,
        )
        .id("lint_useless_assignment")
        .severity(Severity::Warning),
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
            "Style/NumericLiterals",
            r#"
            puts 12345
                 ^^^^^ Use underscores(_) as thousands separator and separate every 3 digits with them.
            "#,
        )
        .id("style_numeric_literals"),
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
        CopCase::annotated(
            "Style/Semicolon",
            r#"
            puts 1; puts 2
                  ^ Do not use semicolons to terminate expressions.
            "#,
        )
        .id("style_semicolon")
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
