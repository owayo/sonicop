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
    assert_offenses, command, lint_stdin, offense_tuples, offenses, project, project_with_ruby,
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

/// 散文の後ろに書かれたディレクティブもディレクティブ。コメントを開いていない
/// ので、効くのはその行だけ。
#[test]
fn a_directive_written_behind_prose_covers_its_own_line() {
    let directory = project(&[]);
    let output = lint_stdin(
        directory.path(),
        "Layout/TrailingWhitespace",
        "# see: # rubocop:disable Layout/TrailingWhitespace  \ny = 1  \n",
    )
    .code(1)
    .get_output()
    .stdout
    .clone();

    assert_offenses(
        &output,
        &[(
            "Layout/TrailingWhitespace",
            2,
            6,
            "Trailing whitespace detected.",
        )],
    );
}

/// 設定で無効にした cop も、ファイル中の `# rubocop:enable` から下では有効に戻る。
///
/// 本家は `CommentConfig#inject_disabled_cops_directives` が設定無効の cop へ `-∞` 行から
/// 始まる disable を注入し、実在する `enable` がその範囲を閉じる。cop 自体は `enable` が
/// 名前を挙げたときだけ動員される (`opt_in_cops`) ので、`enable all` や部門名では戻らない。
#[test]
fn a_config_disabled_cop_comes_back_from_an_enable_directive() {
    let long = "x = '0123456789012345678901234567890123456789'\n";
    let directory = project(&[(
        ".rubocop.yml",
        "Layout/LineLength:\n  Enabled: false\n  Max: 20\n",
    )]);
    let reported = |source: String| {
        let output = command(directory.path())
            .args(["--stdin", "example.rb", "--format", "json"])
            .write_stdin(source)
            .assert()
            .get_output()
            .stdout
            .clone();
        offense_tuples(&output)
            .into_iter()
            .filter(|(cop, ..)| cop == "Layout/LineLength")
            .map(|(_, line, column, _)| (line, column))
            .collect::<Vec<_>>()
    };

    // 名前を挙げた `enable` の下から報告される。書いた行そのものはまだ無効。
    assert_eq!(
        reported(format!("# rubocop:enable Layout/LineLength\n{long}")),
        vec![(2, 21)]
    );
    // `enable` より上は無効のまま。
    assert_eq!(
        reported(format!("{long}# rubocop:enable Layout/LineLength\n{long}")),
        vec![(3, 21)]
    );
    // `enable all` と部門名は cop を動員しない (`raw_cop_names` に cop 名が無い)。
    assert!(reported(format!("# rubocop:enable all\n{long}")).is_empty());
    assert!(reported(format!("# rubocop:enable Layout\n{long}")).is_empty());
    // ディレクティブが無ければ設定どおり無効。
    assert!(reported(long.to_owned()).is_empty());
}

/// 設定で無効にした cop は再有効化を期待されないので、閉じられていない `disable` も
/// 咎められない (`acceptable_range?`)。無効な cop と有効な cop を並べて書いたときは、
/// 有効なほうの名前で報告される。
#[test]
fn a_disable_of_a_config_disabled_cop_needs_no_enable() {
    let directory = project(&[(
        ".rubocop.yml",
        "Layout/LineLength:\n  Enabled: false\n  Max: 20\n",
    )]);
    let missing = |source: &str| {
        let output = command(directory.path())
            .args(["--stdin", "example.rb", "--format", "json"])
            .write_stdin(source.to_owned())
            .assert()
            .get_output()
            .stdout
            .clone();
        offense_tuples(&output)
            .into_iter()
            .filter(|(cop, ..)| cop == "Lint/MissingCopEnableDirective")
            .map(|(_, _, _, message)| message)
            .collect::<Vec<_>>()
    };

    assert!(missing("# rubocop:disable Layout/LineLength\nx = 1\n").is_empty());
    // 有効なほうの名前で報告される。書いた順ではない。
    assert_eq!(
        missing("# rubocop:disable Layout/LineLength, Style/Documentation\nclass A\nend\n"),
        vec![
            "Re-enable Style/Documentation cop with `# rubocop:enable` after disabling it."
                .to_owned()
        ]
    );
}

/// 実コードの形。`rubocop/rubocop` の
/// `lib/rubocop/cop/naming/memoized_instance_variable_name.rb` がこの並びで、設定で
/// Metrics を無効にすると本家は `enable` の下の定義だけを報告する。
///
/// 3 つの挙動が同時に噛み合う: 設定無効の cop が `enable` で戻ること、既に無効な cop への
/// `disable` は「余計」になること、`disable-next` は 1.89 のディレクティブではないので
/// 何も抑えないこと。**どれか 1 つ壊れると別の 2 つの結果も変わる**ので、まとめて固定する。
#[test]
fn a_disable_enable_pair_around_a_config_disabled_cop_matches_upstream() {
    let directory = project(&[(
        ".rubocop.yml",
        "Metrics/MethodLength:\n  Enabled: false\n  Max: 1\n",
    )]);
    let output = command(directory.path())
        .args(["--stdin", "example.rb", "--format", "json"])
        .write_stdin(concat!(
            "# frozen_string_literal: true\n",
            "\n",
            "# rubocop:disable Metrics/MethodLength\n",
            "def first\n",
            "  1\n",
            "  2\n",
            "end\n",
            "# rubocop:enable Metrics/MethodLength\n",
            "\n",
            "# rubocop:disable-next Metrics/MethodLength\n",
            "def second\n",
            "  1\n",
            "  2\n",
            "end\n",
        ))
        .assert()
        .get_output()
        .stdout
        .clone();

    let reported: Vec<(String, usize, usize, String)> = offense_tuples(&output)
        .into_iter()
        .filter(|(cop, ..)| {
            cop == "Metrics/MethodLength" || cop == "Lint/RedundantCopDisableDirective"
        })
        .collect();
    assert_eq!(
        reported,
        vec![
            (
                "Lint/RedundantCopDisableDirective".to_owned(),
                3,
                1,
                "Unnecessary disabling of `Metrics/MethodLength`.".to_owned()
            ),
            (
                "Metrics/MethodLength".to_owned(),
                11,
                1,
                "Method has too many lines. [2/1]".to_owned()
            ),
        ]
    );
}

/// `# rubocop:enable all` は、設定が何か 1 つでも cop を無効にしていれば戻すものがある。
/// 本家は無効な cop 全部に disable を注入するので、その最初の `enable all` は余計ではない。
#[test]
fn an_enable_all_undoes_the_cops_the_configuration_switched_off() {
    let directory = project(&[]);
    let redundant = |source: &str| {
        let output = command(directory.path())
            .args(["--stdin", "example.rb", "--format", "json"])
            .write_stdin(source.to_owned())
            .assert()
            .get_output()
            .stdout
            .clone();
        offense_tuples(&output)
            .into_iter()
            .filter(|(cop, ..)| cop == "Lint/RedundantCopEnableDirective")
            .count()
    };

    // 既定設定でも pending / 既定無効の cop が残っているので、戻すものがある。
    assert_eq!(
        redundant("# frozen_string_literal: true\n\n# rubocop:enable all\nx = 1\n"),
        0
    );
    // 名前を挙げた enable は、その cop に戻すものが無ければ余計。
    assert_eq!(
        redundant("# frozen_string_literal: true\n\n# rubocop:enable Style/Documentation\nx = 1\n"),
        1
    );
}

/// `# rubocop:enable` を書いた行は、それが閉じる範囲の最終行なので、まだ無効の
/// まま。効き始めるのは次の行から。
#[test]
fn the_line_an_enable_directive_is_written_on_is_still_disabled() {
    let directory = project(&[]);
    let output = lint_stdin(
        directory.path(),
        "Layout/TrailingWhitespace",
        "# rubocop:disable Layout/TrailingWhitespace\nx = 1  \n# rubocop:enable Layout/TrailingWhitespace  \ny = 2  \n",
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
            6,
            "Trailing whitespace detected.",
        )],
    );
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

    // RuboCop walks the arguments in the order they were given and only puts each directory's own
    // expansion in order, so the file named second stays second. Measured against 1.89.0.
    command(directory.path())
        .args(["--list-target-files", "lib", "lib.rb"])
        .assert()
        .success()
        .stdout("lib/a.rb\nlib.rb\n");
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

/// 既定の実行では stderr へ何も出さない。
///
/// 全 609 cop が実装済みなので、未実装を告げる警告が混ざる余地は無い。ここが崩れると
/// stderr を読んでいる検証スクリプトや計測が巻き添えになる。
#[test]
fn a_default_run_stays_silent_on_stderr() {
    let directory = project(&[("example.rb", "value = 1\n")]);
    let output = command(directory.path())
        .args(["--force-default-config", "--format", "quiet"])
        .assert()
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "",
        "既定実行の stderr に出力があった"
    );
}

/// 本家の全 cop が実装済みで、名前も 1 つ残らず一致している。
///
/// もともとは「未実装 cop を名指しすると stderr で警告する」ことを見るテストで、実装が
/// 進むたびに名指しする cop を差し替えていた。全部そろった今は名指しできる cop が無いので、
/// **そろっていること自体**を固定する形へ変えた。`--show-cops` は実装の有無に関わらず全 cop
/// を出し、各エントリに `Implemented:` を添える (この形式を取り違えて「名前の集合が一致 =
/// 全実装」と読み、実際には 25 個未実装だったのを見落としかけたことがある)。
///
/// 警告そのものは [`super`] の `warn_unimplemented_enabled` に残してある。本家が新しい cop を
/// 足し、`config/default.yml` を取り込んで名前だけ認識した状態になったときに効く。
#[test]
fn every_upstream_cop_is_implemented() {
    let directory = project(&[]);
    let output = command(directory.path())
        .args(["--force-default-config", "--show-cops"])
        .assert()
        .get_output()
        .stdout
        .clone();
    let listing = String::from_utf8(output).expect("--show-cops の出力が UTF-8 でなかった");

    let mut current = None;
    let mut unimplemented = Vec::new();
    let mut implemented = 0_usize;
    for line in listing.lines() {
        if let Some(name) = line.strip_suffix(':').filter(|name| name.contains('/')) {
            current = Some(name.to_owned());
        } else if let Some(state) = line.trim().strip_prefix("Implemented: ") {
            match (state, current.take()) {
                ("true", _) => implemented += 1,
                (_, Some(name)) => unimplemented.push(name),
                (_, None) => {}
            }
        }
    }

    assert!(
        unimplemented.is_empty(),
        "未実装の cop が残っている: {unimplemented:?}"
    );
    assert_eq!(
        implemented,
        sonicop::rules::rule_names().count(),
        "--show-cops が数える実装済み cop とレジストリの登録数が食い違う"
    );
}

/// 既定以外の設定で名指しされた cop も、実装がある以上は黙って素通りしない。
///
/// 警告が出るのは未実装のときだけなので、全実装のいまは**出ないこと**が正しい。stdout を
/// 汚さないことも合わせて見る (RuboCop は同種の注記を stdout に出して自分の JSON を壊す)。
#[test]
fn naming_any_cop_leaves_stdout_clean() {
    let directory = project(&[("example.rb", "value = 1\n")]);
    let output = command(directory.path())
        .args([
            "--force-default-config",
            "--only",
            "Style/ArgumentsForwarding",
            "--format",
            "json",
        ])
        .assert()
        .get_output()
        .clone();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("not implemented"),
        "実装済みの cop に未実装の警告が出た: {stderr}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with('{'),
        "JSON の前に何か出力された: {}",
        &stdout[..stdout.len().min(80)]
    );
}

/// `AutoCorrect: false` / `disabled` を指定した cop は書き換えず、correctable でもない。
///
/// 本家は `AutocorrectLogic#autocorrect_enabled?` (cop/autocorrect_logic.rb:31-46) が false を
/// 返した時点で `Base#use_corrector` (cop/base.rb:445-453) が `:unsupported` を返すので、offense
/// は報告されるが `correctable?` ですらない。ここを見ていないと「この cop の自動修正は切る」と
/// 明示したユーザのコードが `-a` で黙って書き換わる。
#[test]
fn a_cop_with_autocorrect_disabled_is_neither_applied_nor_correctable() {
    let source = "def foo\n  x = 1\n  2\nend\n";
    for setting in ["false", "disabled"] {
        let config = format!("Lint/UselessAssignment:\n  AutoCorrect: {setting}\n");
        let directory = project(&[(".rubocop.yml", config.as_str()), ("example.rb", source)]);

        let output = command(directory.path())
            .args([
                "-a",
                "--only",
                "Lint/UselessAssignment",
                "--format",
                "json",
                "example.rb",
            ])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();

        assert_eq!(
            fs::read_to_string(directory.path().join("example.rb")).unwrap(),
            source,
            "AutoCorrect: {setting} の cop が -a でファイルを書き換えた"
        );
        let offenses = offenses(&output);
        assert_eq!(
            offenses.len(),
            1,
            "AutoCorrect: {setting} で検出自体が消えた"
        );
        assert!(
            !offenses[0].correctable,
            "AutoCorrect: {setting} の offense が correctable のまま"
        );
        assert!(
            !offenses[0].corrected,
            "AutoCorrect: {setting} の offense が corrected と数えられた"
        );
    }
}

/// `--editor-mode` は `AutoCorrect: contextual` の cop の修正を止める。
///
/// 本家では `LSP.enabled?` が立ち、`autocorrect_enabled?` の contextual 分岐が false になる。
/// `config/default.yml` が `Lint/UselessAssignment` に contextual を与えているので、設定を足さ
/// なくても再現する — 打ちかけの代入が消えては困る場面そのもの。バッチ実行では従来どおり直る。
#[test]
fn editor_mode_withholds_a_contextual_autocorrect() {
    let source = "def foo\n  x = 1\n  2\nend\n";

    let editor = project(&[("example.rb", source)]);
    let output = command(editor.path())
        .args([
            "--editor-mode",
            "-a",
            "--only",
            "Lint/UselessAssignment",
            "--format",
            "json",
            "example.rb",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        fs::read_to_string(editor.path().join("example.rb")).unwrap(),
        source,
        "--editor-mode で contextual な cop が代入を消した"
    );
    assert!(
        !offenses(&output)[0].correctable,
        "--editor-mode の contextual な offense が correctable のまま"
    );

    let batch = project(&[("example.rb", source)]);
    command(batch.path())
        .args(["-a", "--only", "Lint/UselessAssignment", "example.rb"])
        .assert()
        // 唯一の offense が直りきるので、残りは無い。
        .code(0);
    assert_eq!(
        fs::read_to_string(batch.path().join("example.rb")).unwrap(),
        // `--only` で Lint/Void を外してあるので、残された `1` はそのまま。
        "def foo\n  1\n  2\nend\n",
        "エディタが動かしていない -a で contextual な cop が直さなくなった"
    );
}

/// `-A` は symlink を symlink のまま残し、その実体を書き換える。
///
/// 本家の `File.write` は path を開くので link をたどる。temp + rename はディレクトリ
/// エントリごと差し替えるため、symlink が通常ファイルへ化けて実体は元のまま残る。
#[cfg(unix)]
#[test]
fn autocorrect_writes_through_a_symlink_and_leaves_it_a_symlink() {
    let directory = project(&[("shared/real.rb", "y  = 2\nputs y\n")]);
    let link = directory.path().join("link.rb");
    std::os::unix::fs::symlink(directory.path().join("shared/real.rb"), &link).unwrap();

    command(directory.path())
        .args(["-A", "--only", "Layout/ExtraSpacing", "link.rb"])
        .assert()
        .code(0);

    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "-A が symlink を通常ファイルへ置き換えた"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("shared/real.rb")).unwrap(),
        "y = 2\nputs y\n",
        "symlink の実体が直っていない"
    );
}

/// `-A` は hard link を切らない。
///
/// rename は inode を unlink するので link 数が 1 に落ち、同じ inode を指していたもう一方の
/// 名前は古い本文のまま取り残される。どちらも警告なしに起きるので、link 数と本文の両方を見る。
#[cfg(unix)]
#[test]
fn autocorrect_keeps_a_hard_linked_file_shared() {
    use std::os::unix::fs::MetadataExt;

    let directory = project(&[("one.rb", "y  = 2\nputs y\n")]);
    let other = directory.path().join("other.rb");
    fs::hard_link(directory.path().join("one.rb"), &other).unwrap();

    command(directory.path())
        .args(["-A", "--only", "Layout/ExtraSpacing", "one.rb"])
        .assert()
        .code(0);

    assert_eq!(
        fs::metadata(&other).unwrap().nlink(),
        2,
        "-A が hard link を切った"
    );
    assert_eq!(
        fs::read_to_string(&other).unwrap(),
        "y = 2\nputs y\n",
        "同じ inode を指すもう一方の名前が古い本文のまま"
    );
}

/// 読めない `.rubocop.yml` は `--auto-gen-config` の生成物で上書きしない。
///
/// 本家は `File.exist?` を見てから読むので「無い」と「読めない」は別の答え。read の失敗を
/// 既定値へ潰すと、prepend のつもりが replace になってユーザの設定が消える。無い場合の
/// 「作る」経路は生き続けなければならないので、そちらも合わせて見る。
#[test]
fn auto_gen_config_refuses_to_replace_a_config_it_cannot_read() {
    let directory = project_without_pinned_ruby(&[
        ("other.yml", "AllCops:\n  TargetRubyVersion: '2.7'\n"),
        ("example.rb", "x = 1  \n"),
    ]);
    // 先頭行のコメントだけ CP932。YAML としては正しいが UTF-8 として読めない。
    let original = [
        b"# \x90\xdd\x92\xe8\n".as_slice(),
        b"Layout/LineLength:\n  Max: 100\n".as_slice(),
    ]
    .concat();
    let root_config = directory.path().join(".rubocop.yml");
    fs::write(&root_config, &original).unwrap();

    command(directory.path())
        .args(["-c", "other.yml", "--auto-gen-config"])
        .assert()
        .code(2);

    assert_eq!(
        fs::read(&root_config).unwrap(),
        original,
        "読めない .rubocop.yml が生成物で上書きされた"
    );

    let fresh = project_without_pinned_ruby(&[
        ("other.yml", "AllCops:\n  TargetRubyVersion: '2.7'\n"),
        ("example.rb", "x = 1  \n"),
    ]);
    command(fresh.path())
        .args(["-c", "other.yml", "--auto-gen-config"])
        .assert()
        .code(0);
    assert_eq!(
        fs::read_to_string(fresh.path().join(".rubocop.yml")).unwrap(),
        "inherit_from: .rubocop_todo.yml\n",
        ".rubocop.yml が無いときの「作る」経路が壊れた"
    );
}

/// `--fail-level autocorrect` は severity の閾値を置き換えるのではなく足す。
///
/// 本家 `Runner#considered_failure?` (runner.rb:561-569) は correctable なら早期に true を返し、
/// **その後も** severity 比較へ落ちる。correctable だけを見ると、直しようのない
/// `Metrics/MethodLength` で CI が緑になる。
#[test]
fn fail_level_autocorrect_still_honours_the_severity_threshold() {
    let offending = project(&[
        (".rubocop.yml", "Metrics/MethodLength:\n  Max: 2\n"),
        (
            "example.rb",
            "def foo\n  a = 1\n  b = 2\n  c = 3\n  d = 4\n  [a, b, c, d]\nend\n",
        ),
    ]);
    command(offending.path())
        .args([
            "--fail-level",
            "autocorrect",
            "--only",
            "Metrics/MethodLength",
            "--format",
            "quiet",
            "example.rb",
        ])
        .assert()
        .code(1);

    // 全 cop が走る側なので改行コードを固定する。既定の `native` は Windows では CRLF を
    // 期待し、LF で書いたフィクスチャに `Layout/EndOfLine` が付いて「無違反」でなくなる。
    let clean = project(&[
        (".rubocop.yml", "Layout/EndOfLine:\n  EnforcedStyle: lf\n"),
        ("example.rb", "# frozen_string_literal: true\n\nputs 1\n"),
    ]);
    command(clean.path())
        .args([
            "--fail-level",
            "autocorrect",
            "--format",
            "quiet",
            "example.rb",
        ])
        .assert()
        .code(0);
}

/// `--stdin` + 自動修正は formatter の出力を出したうえで、区切りと修正後ソースを足す。
///
/// 本家 `ExecuteRunner#maybe_print_corrected_source` (cli/command/execute_runner.rb:92-102)。
/// 修正後ソースだけを出すと、何を直したのかを告げるものが無くなる。
#[test]
fn stdin_autocorrect_appends_the_corrected_buffer_to_the_report() {
    let directory = project(&[]);
    let output = command(directory.path())
        .args([
            "--stdin",
            "example.rb",
            "-a",
            "--only",
            "Layout/ExtraSpacing",
            "--format",
            "simple",
        ])
        .write_stdin("y  = 2\n")
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("--stdin の出力が UTF-8 でなかった");
    assert!(
        stdout.contains("Layout/ExtraSpacing"),
        "formatter の出力ごと落ちている:\n{stdout}"
    );
    assert!(
        stdout.ends_with("====================\ny = 2\n"),
        "区切りと修正後ソースが末尾に無い:\n{stdout}"
    );
}

/// `--format json` などの統合向け formatter には何も足さない。
///
/// `INTEGRATION_FORMATTERS` に載る形式は出力全体を機械が読むので、生の Ruby を継ぎ足すと
/// パースが壊れる。判定は最後に指定された `--format` で行う (`@options[:format]`)。
#[test]
fn stdin_autocorrect_leaves_an_integration_formatter_alone() {
    let directory = project(&[]);
    let output = command(directory.path())
        .args([
            "--stdin",
            "example.rb",
            "-a",
            "--only",
            "Layout/ExtraSpacing",
            "--format",
            "json",
        ])
        .write_stdin("y  = 2\n")
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).expect("--stdin の出力が UTF-8 でなかった");
    assert!(
        !stdout.contains("===================="),
        "JSON に区切りと生ソースが継ぎ足された:\n{stdout}"
    );
    assert_offenses(
        stdout.as_bytes(),
        &[("Layout/ExtraSpacing", 1, 2, "Unnecessary spacing detected.")],
    );
}

/// UTF-8 でない `--stdin` は実行を止めず `Lint/Syntax` として報告する。
///
/// ディスク上のファイルは `decoded_source` 経由で既にそうなっている。stdin だけ
/// `read_to_string` で読むと、`--format json` を頼んだ呼び出し側が JSON を 1 バイトも
/// 受け取れないまま exit 2 を見ることになる。
#[test]
fn stdin_that_is_not_utf8_reports_a_syntax_offense_instead_of_aborting() {
    let directory = project(&[]);
    let output = command(directory.path())
        .args(["--stdin", "example.rb", "--format", "json"])
        .write_stdin(b"x = \"\xff\xfe\"\n".to_vec())
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    assert_offenses(
        &output,
        &[("Lint/Syntax", 1, 1, "Invalid byte sequence in utf-8.")],
    );
}

/// `--fail-fast` は「探した数」と「読んだ数」を別々に報告する。
///
/// `JSONFormatter` は `target_file_count` を `started(target_files)` から、
/// `inspected_file_count` を `finished(inspected_files)` から取る。途中で止まる実行では
/// 両者が食い違うので、reports の数で両方を埋めると対象を全部見たかのように見える。
#[test]
fn fail_fast_separates_the_targets_found_from_the_files_inspected() {
    let sources: Vec<(String, String)> = (1..=5)
        .map(|index| (format!("f{index}.rb"), format!("x{index} = 1  \n")))
        .collect();
    let files: Vec<(&str, &str)> = sources
        .iter()
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect();
    let directory = project(&files);

    let output = command(directory.path())
        .args([
            "--fail-fast",
            "--only",
            "Layout/TrailingWhitespace",
            "--format",
            "json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    let summary = report(&output).summary;
    assert_eq!(
        (summary.target_file_count, summary.inspected_file_count),
        (5, 1),
        "--fail-fast の対象数と検査数が食い違わない"
    );
}

/// `-o` は直前の `-f` に付く。どの `-f` よりも前に書かれたものは本家では
/// `output_path` に入り、`apply_default_formatter` の
/// `@options[:formatters] ||= [[format, output_path]]` だけがそれを読む。
/// `-f` が 1 つでもあれば `||=` は何もしないので、そのパスは捨てられて
/// フォーマッタは stdout に書く。第 1 フォーマッタに繋いでしまうと、
/// stdout をパイプで受ける CI が空を掴む。
#[test]
fn an_output_path_written_before_every_formatter_is_dropped() {
    let directory = project(&[("a.rb", "x  = 1\n")]);
    let out = directory.path().join("out.txt");

    let output = command(directory.path())
        .args([
            "a.rb",
            "-o",
            out.to_str().unwrap(),
            "--only",
            "Layout/ExtraSpacing",
            "-f",
            "json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    assert!(
        !output.is_empty(),
        "-f があるなら先行する -o は捨てられ、フォーマッタは stdout に書く"
    );
    assert!(
        !out.exists(),
        "捨てたはずのパスにファイルを作ってはならない"
    );
}

/// `-f` が 1 つも無ければ `||=` が働くので、先行する `-o` は既定フォーマッタの
/// 書き出し先になる。上のテストと対で「常に捨てる」実装への退行を止める。
#[test]
fn an_output_path_without_any_formatter_still_receives_the_default_one() {
    let directory = project(&[("a.rb", "x  = 1\n")]);
    let out = directory.path().join("out.txt");

    let output = command(directory.path())
        .args([
            "a.rb",
            "-o",
            out.to_str().unwrap(),
            "--only",
            "Layout/ExtraSpacing",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    assert!(
        output.is_empty(),
        "書き出し先を渡したなら stdout は空になる"
    );
    let written = fs::read_to_string(&out).expect("既定フォーマッタの出力が書かれている");
    assert!(written.contains("Layout/ExtraSpacing"));
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
