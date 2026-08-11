//! CLI の結合テスト。実バイナリを起こすので、cop の振る舞いそのものは
//! `tests/cops.rs` の in-process ハーネスに任せ、ここでは CLI 固有の関心
//! (引数解釈 / 設定の探索 / 出力形式 / ファイル書き換え) だけを見る。
//!
//! すべてのケースを tempdir に閉じ込める。リポジトリルートで走らせると
//! sonicop.gemspec の `required_ruby_version` が対象 Ruby として拾われ、
//! 配布メタデータを変えると無関係なリンタのテストが落ちる。

mod support;

use std::fs;
use std::process::Command;

use support::project::{
    assert_offenses, command, lint_stdin, offenses, project, project_with_ruby,
    project_without_pinned_ruby, report,
};

/// `Lint/Syntax` が付ける版の注記。src 側の書式と一致させて固定する。
fn syntax_message(reason: &str, target_ruby: &str) -> String {
    format!(
        "{reason}\n(Using Ruby {target_ruby} parser; configure using `TargetRubyVersion` parameter, under `AllCops`)"
    )
}

#[test]
fn reports_json_using_rubocop_shape() {
    let directory = project(&[]);
    let output = lint_stdin(
        directory.path(),
        "Layout/TrailingWhitespace",
        "value = 1  \n",
    )
    .code(1)
    .get_output()
    .stdout
    .clone();

    assert_offenses(
        &output,
        &[(
            "Layout/TrailingWhitespace",
            1,
            10,
            "Trailing whitespace detected.",
        )],
    );
    let summary = report(&output).summary;
    assert_eq!(
        (
            summary.offense_count,
            summary.target_file_count,
            summary.inspected_file_count
        ),
        (1, 1, 1)
    );
}

#[test]
fn syntax_uses_gemspec_target_version_and_legacy_recovery_locations() {
    let directory = project_without_pinned_ruby(&[(
        "example.gemspec",
        "Gem::Specification.new do |spec|\n  spec.required_ruby_version = '>= 2.6.0'\nend\n",
    )]);
    let source = "require 'support'\nmodule Example\n  def forward(message, ...)\n    message\n  end\n  def self.enable!\n    $VERBOSE = true\n  end\nend\n";

    let output = lint_stdin(directory.path(), "Lint/Syntax", source)
        .code(1)
        .get_output()
        .stdout
        .clone();

    assert_offenses(
        &output,
        &[
            (
                "Lint/Syntax",
                2,
                1,
                &syntax_message("module definition in method body", "2.6"),
            ),
            (
                "Lint/Syntax",
                3,
                24,
                &syntax_message("unexpected token tDOT3", "2.6"),
            ),
            (
                "Lint/Syntax",
                9,
                1,
                &syntax_message("unexpected token kEND", "2.6"),
            ),
        ],
    );
}

#[test]
fn syntax_gates_beginless_ranges_at_ruby_2_7() {
    let directory = project_without_pinned_ruby(&[(
        "example.gemspec",
        "Gem::Specification.new do |spec|\n  spec.required_ruby_version = '>= 2.6.0'\nend\n",
    )]);

    let gated = lint_stdin(directory.path(), "Lint/Syntax", "source[..position]\n")
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_offenses(
        &gated,
        &[(
            "Lint/Syntax",
            1,
            8,
            &syntax_message("unexpected token tDOT2", "2.6"),
        )],
    );

    fs::write(
        directory.path().join(".rubocop.yml"),
        "AllCops:\n  TargetRubyVersion: 2.7\n",
    )
    .unwrap();
    let allowed = lint_stdin(directory.path(), "Lint/Syntax", "source[..position]\n")
        .success()
        .get_output()
        .stdout
        .clone();
    assert_offenses(&allowed, &[]);
}

#[test]
fn safe_autocorrect_updates_a_file_atomically() {
    let directory = project(&[("example.rb", "value=10000  \n")]);

    command(directory.path())
        .args([
            "-a",
            "--only",
            "Layout/SpaceAroundOperators,Layout/TrailingWhitespace,Style/NumericLiterals",
            "example.rb",
        ])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(directory.path().join("example.rb")).unwrap(),
        "value = 10_000\n"
    );
}

#[test]
fn line_length_marks_breakable_calls_and_blocks_as_correctable() {
    let directory = project(&[]);
    let output = lint_stdin(directory.path(), "Layout/LineLength", &breakable_source())
        .code(1)
        .get_output()
        .stdout
        .clone();

    let offenses = offenses(&output);
    assert_eq!(offenses.len(), 3);
    assert!(
        offenses.iter().all(|offense| offense.correctable),
        "折り返せる呼び出しとブロックは correctable であるべき: {offenses:?}"
    );
}

#[test]
fn line_length_only_exempts_a_qualified_name_that_ends_the_line() {
    let directory = project(&[]);
    let exempt = format!(
        "# {} RuboCop::Cop::Layout::LineLength\n",
        "description ".repeat(8)
    );
    let rejected = format!(
        "# {} RuboCop::Cop::Layout::LineLength followed by prose\n",
        "description ".repeat(8)
    );

    let output = lint_stdin(
        directory.path(),
        "Layout/LineLength",
        &format!("{exempt}{rejected}"),
    )
    .code(1)
    .get_output()
    .stdout
    .clone();

    let offenses = offenses(&output);
    assert_eq!(
        offenses
            .iter()
            .map(|offense| offense.location.start_line)
            .collect::<Vec<_>>(),
        vec![2],
        "行末で終わる修飾名だけを免除するべき: {offenses:?}"
    );
}

#[test]
fn line_length_autocorrect_inserts_rubocop_compatible_breaks() {
    let directory = project(&[("example.rb", &breakable_source())]);

    command(directory.path())
        .args(["-a", "--only", "Layout/LineLength", "example.rb"])
        .assert()
        .code(1);

    // 元の 3 行それぞれが折り返された姿を 1 要素ずつ並べる。1 個の巨大な
    // format! に畳むと、どこへ改行が入るのかが読めなくなる。
    let long = "x".repeat(120);
    let padding = "a".repeat(90);
    let expected = [
        format!("register_cop :Example, \n\"{long}\"\n"),
        format!("format.html {{\n redirect_to @book, notice: 'created' }} # {padding}\n"),
        format!("it '{long}', \n:ruby do\n  verify\nend\n"),
    ]
    .concat();

    assert_eq!(
        fs::read_to_string(directory.path().join("example.rb")).unwrap(),
        expected
    );
}

#[test]
fn class_length_matches_rubocop_line_indexing_around_nested_classes() {
    let directory = project(&[(
        ".rubocop.yml",
        "AllCops:\n  DisabledByDefault: true\nMetrics/ClassLength:\n  Enabled: true\n  Max: 1\n",
    )]);
    let source = "class Example\n  FIRST = 1\n  class Nested; end\n\n  attr_reader :value\nend\n";

    let output = lint_stdin(directory.path(), "Metrics/ClassLength", source)
        .code(1)
        .get_output()
        .stdout
        .clone();

    assert_offenses(
        &output,
        &[(
            "Metrics/ClassLength",
            1,
            1,
            "Class has too many lines. [3/1]",
        )],
    );
}

#[test]
fn disable_comment_suppresses_an_offense() {
    let directory = project(&[]);
    let output = lint_stdin(
        directory.path(),
        "Layout/TrailingWhitespace",
        "value = 1  # rubocop:disable Layout/TrailingWhitespace\n",
    )
    .success()
    .get_output()
    .stdout
    .clone();

    assert_offenses(&output, &[]);
}

#[test]
fn directive_text_inside_a_heredoc_does_not_suppress_later_offenses() {
    let directory = project(&[]);
    let output = lint_stdin(
        directory.path(),
        "Layout/TrailingWhitespace",
        "text = <<~RUBY\n  # rubocop:disable all\nRUBY\nvalue = 1  \n",
    )
    .code(1)
    .get_output()
    .stdout
    .clone();

    assert_offenses(
        &output,
        &[(
            "Layout/TrailingWhitespace",
            4,
            10,
            "Trailing whitespace detected.",
        )],
    );
}

#[test]
fn gem_wrapper_finds_the_development_binary() {
    let binary = assert_cmd::cargo::cargo_bin!("sonicop");
    let status = Command::new("ruby")
        .env("SONICOP_BINARY", binary)
        .args(["-Ilib", "exe/sonicop", "--version"])
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn list_target_files_preserves_relative_paths_and_rubocop_order() {
    let directory = project(&[
        ("Gemfile", ""),
        ("lib/a.rb", "puts 1\n"),
        ("lib.rb", "puts 1\n"),
    ]);

    command(directory.path())
        .args(["--list-target-files", "lib", "lib.rb"])
        .assert()
        .success()
        .stdout("lib.rb\nlib/a.rb\n");
}

#[test]
fn list_target_files_applies_relative_excludes_and_skips_hidden_files() {
    let directory = project(&[
        (
            ".rubocop.yml",
            "AllCops:\n  Exclude:\n    - Gemfile\n    - excluded/**/*\n",
        ),
        ("Gemfile", ""),
        ("good.rb", "puts 1\n"),
        ("excluded/skip.rb", "puts 1\n"),
        (".hidden.rb", "#!/usr/bin/env ruby\nputs 1\n"),
    ]);

    command(directory.path())
        .args(["--list-target-files", "."])
        .assert()
        .success()
        .stdout("good.rb\n");
}

/// trailing whitespace を a.rb に 3 件 / b.rb に 1 件持つ、offense 4 件 ・
/// ファイル 2 件のプロジェクト。両者が食い違う値なので、count と exclude limit
/// のどちらがどちらを見ているかを 1 回の生成で切り分けられる。
const UNEVEN_OFFENSES: &[(&str, &str)] = &[
    ("a.rb", "x = 1  \ny = 2  \nz = 3  \n"),
    ("b.rb", "w = 4  \n"),
];

/// 一時プロジェクトへ `--auto-gen-config` をかけ、生成された
/// `.rubocop_todo.yml` を返す。
///
/// 生成後の `.rubocop.yml` は `.rubocop_todo.yml` を継承するため、同じ
/// ディレクトリで 2 度走らせると対象が除外されて offense が消える。呼び出し
/// ごとにプロジェクトを作り直す。
fn generate_todo(files: &[(&str, &str)], extra: &[&str]) -> String {
    let directory = project(files);
    let mut arguments = vec!["--auto-gen-config", "--only", "Layout/TrailingWhitespace"];
    arguments.extend_from_slice(extra);
    command(directory.path()).args(arguments).assert().success();
    fs::read_to_string(directory.path().join(".rubocop_todo.yml"))
        .expect(".rubocop_todo.yml が生成されていない")
}

/// `# Offense count:` は対象ファイル数ではなく offense の総数。本家
/// `disabled_config_formatter.rb` は offense ごとに `@cops_with_offenses` を
/// 加算する (:59) ため、同一ファイルの 3 件は 3 と数える。
#[test]
fn auto_gen_config_counts_offenses_not_offending_files() {
    let todo = generate_todo(UNEVEN_OFFENSES, &[]);

    assert!(
        todo.contains("# Offense count: 4\n"),
        "offense 4 件 / ファイル 2 件を offense 数で数えていない:\n{todo}"
    );
    assert!(
        todo.contains("  Exclude:\n    - 'a.rb'\n    - 'b.rb'\n"),
        "Exclude はファイル単位で 1 度ずつ:\n{todo}"
    );
}

/// exclude limit が見るのは offense 総数ではなく対象ファイル数。本家が
/// `offending_files.count` と比べる (:237) ため、offense 4 件 / ファイル 2 件は
/// limit 2 では `Enabled: false` に落ちない。
#[test]
fn auto_gen_config_weighs_the_exclude_limit_against_files() {
    let within = generate_todo(UNEVEN_OFFENSES, &["--exclude-limit", "2"]);
    assert!(
        within.contains("  Exclude:\n    - 'a.rb'\n    - 'b.rb'\n"),
        "ファイル数が limit 以内なら Exclude を並べる:\n{within}"
    );

    let exceeded = generate_todo(UNEVEN_OFFENSES, &["--exclude-limit", "1"]);
    assert!(
        exceeded.contains("  Enabled: false\n"),
        "ファイル数が limit 超過なら Enabled: false:\n{exceeded}"
    );
}

/// パスに `'` が入ると単一引用符スカラがそこで閉じ、生成した設定が読めなく
/// なる。YAML の唯一のエスケープである `''` へ二重化する。
#[test]
fn auto_gen_config_escapes_single_quotes_in_paths() {
    let todo = generate_todo(&[("it's.rb", "x = 1  \n")], &[]);

    assert!(
        todo.contains("    - 'it''s.rb'\n"),
        "`'` を含むパスがエスケープされていない:\n{todo}"
    );
}

#[test]
fn rails_compatibility_exceptions_do_not_create_false_positives() {
    // ソースが `call(payload:)` を含むので、値の省略を受け付ける 3.1 で見る。
    // 2.7 のままだと本家もここを構文エラーにし (実測: 21:18 tRPAREN と 42:4 $end)、
    // ファイル全体が Lint/Syntax 以外の cop の対象から外れてしまう。
    let directory = project_with_ruby(
        &[(
            ".rubocop.yml",
            "Style/Semicolon:\n  AllowAsExpressionSeparator: true\nStyle/StringLiterals:\n  EnforcedStyle: double_quotes\n",
        )],
        "3.1",
    );
    let source = r##"class Example
  if RUBY_ENGINE == "ruby"
    def dump; 1; end
  else
    def dump; 2; end
  end

  VALUES = %w( one two )
  CONFIG = { 'model_class': nil }
  MIXED_KEYS = { :symbol => 1, "string" => 2 }

  def render(items)
    puts "#{items.join(', ')}"
    call( # the argument starts on the next line
      1
    )
  end

  def emit
    payload = {}
    call(payload:)
  end

  def template
    captured = 1
    binding
  end

  begin
    def platform_clock
      1
    end
  rescue
    def platform_clock
      0
    end
  end

  def mutable_string
    return +""
  end
end
"##;

    let output = lint_stdin(
        directory.path(),
        "Style/Semicolon,Style/StringLiterals,Style/HashSyntax,Layout/SpaceInsideParens,Layout/SpaceAroundOperators,Lint/DuplicateMethods,Lint/UselessAssignment",
        source,
    )
    .success()
    .get_output()
    .stdout
    .clone();

    assert_offenses(&output, &[]);
}

/// 折り返せる呼び出し / ブロック / do ブロックを 1 行ずつ含む長い行のソース。
fn breakable_source() -> String {
    let long = "x".repeat(120);
    let padding = "a".repeat(90);
    [
        format!("register_cop :Example, \"{long}\"\n"),
        format!("format.html {{ redirect_to @book, notice: 'created' }} # {padding}\n"),
        format!("it '{long}', :ruby do\n  verify\nend\n"),
    ]
    .concat()
}
