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

use serde_yaml_ng as serde_yaml;

use support::project::{
    assert_offenses, command, lint_stdin, offense_tuples, offenses, project, project_with_ruby,
    project_without_pinned_ruby, report,
};

/// Windows の Ruby File.write と同じ、補正済み通常コードの native 改行。
/// 未補正・拒否・CRLF 原本・混在改行・DATA/NUL を持つ期待値には使わない。
fn native_lf_disk_correction(expected: &[u8]) -> Vec<u8> {
    assert!(!expected.contains(&b'\r'));
    assert!(!expected.contains(&0));
    assert!(!expected.windows(7).any(|part| part == b"__END__"));
    if cfg!(windows) {
        expected
            .iter()
            .flat_map(|&byte| {
                if byte == b'\n' {
                    vec![b'\r', b'\n']
                } else {
                    vec![byte]
                }
            })
            .collect()
    } else {
        expected.to_vec()
    }
}

/// Windows Ruby の stdout/stderr/File の text mode。#102 の実 pipe 記録に従う。
/// 拒否した stdin 本文には適用せず、report・区切りと成功した本文を分けて使う。
fn native_textmode_expected(expected: &[u8]) -> Vec<u8> {
    if cfg!(windows) {
        expected
            .iter()
            .flat_map(|&byte| {
                if byte == b'\n' {
                    vec![b'\r', b'\n']
                } else {
                    vec![byte]
                }
            })
            .collect()
    } else {
        expected.to_vec()
    }
}

fn stdin_separator() -> &'static str {
    if cfg!(windows) {
        "====================\r\n"
    } else {
        "====================\n"
    }
}

/// 全 252 条件の本家 Windows CLI と、既存の物理 tail 保護から導出した本文。
fn windows_stdin_buffer_case(index: usize, case: &serde_json::Value) -> &'static serde_json::Value {
    static REFERENCE: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(|| {
        let recorded: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/windows_stdin_buffers.json")).unwrap();
        assert_eq!(recorded["cases"].as_object().unwrap().len(), 252);
        recorded
    });
    let expected = &REFERENCE["cases"][index.to_string()];
    for key in ["name", "ruby", "eol", "mode", "source_hex", "exit"] {
        assert_eq!(expected[key], case[key], "{index}: {key}");
    }
    expected
}

/// 本家実 CLI の補正済み code と原本 tail から導出した Windows の物理 byte と再検査。
fn windows_native_cli_case(
    fixture: &str,
    group: Option<usize>,
    case: usize,
) -> &'static serde_json::Value {
    static REFERENCE: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!(
            "fixtures/windows_native_cli_expectations.json"
        ))
        .unwrap()
    });
    let key = match group {
        Some(group) => format!("group:{group}:case:{case}"),
        None => format!("record:{case}"),
    };
    let expected = &REFERENCE["cases"][fixture][key];
    assert!(
        expected.is_object(),
        "Windows の実 CLI 記録が不足: {fixture}/{group:?}/{case}"
    );
    expected
}

/// `Lint/Syntax` が付ける版の注記。src 側の書式と一致させて固定する。
fn syntax_message(reason: &str, target_ruby: &str) -> String {
    format!(
        "{reason}\n(Using Ruby {target_ruby} parser; configure using `TargetRubyVersion` parameter, under `AllCops`)"
    )
}

/// 元の単独 CR は本家でも残る。LF だけが File.write の native 改行になる。
fn native_lf_code_with_bare_cr(expected: &[u8]) -> Vec<u8> {
    assert!(expected.contains(&b'\r'));
    assert!(!expected.windows(2).any(|pair| pair == b"\r\n"));
    assert!(!expected.contains(&0));
    assert!(!expected.windows(7).any(|part| part == b"__END__"));
    if cfg!(windows) {
        expected
            .iter()
            .flat_map(|&byte| {
                if byte == b'\n' {
                    vec![b'\r', b'\n']
                } else {
                    vec![byte]
                }
            })
            .collect()
    } else {
        expected.to_vec()
    }
}

#[test]
fn reports_keep_the_reference_file_and_argument_order() {
    let cases: [(&[&str], &[&str]); 3] = [
        (&["."], &["foo.rb", "foo/bar.rb", "foo1/bar.rb"]),
        (&["foo/bar.rb", "foo.rb"], &["foo/bar.rb", "foo.rb"]),
        (&["foo.rb", "foo/bar.rb"], &["foo.rb", "foo/bar.rb"]),
    ];
    // 本家 1.89.0 はディレクトリ内を文字列順に展開し、引数間の順番は保持する。
    for cache in ["false", "true"] {
        for parallel in [false, true] {
            let directory = project(&[
                ("foo.rb", "puts 1\n"),
                ("foo/bar.rb", "puts 2\n"),
                ("foo1/bar.rb", "puts 3\n"),
            ]);
            for (arguments, expected) in cases {
                for _ in 0..if cache == "true" { 2 } else { 1 } {
                    let mut invocation = command(directory.path());
                    invocation.args([
                        "--force-default-config",
                        "--cache",
                        cache,
                        "--only",
                        "Lint/Syntax",
                        "-f",
                        "json",
                    ]);
                    if cache == "true" {
                        invocation
                            .arg("--cache-root")
                            .arg(fs::canonicalize(directory.path()).unwrap().join("cache"));
                    }
                    if !parallel {
                        invocation.arg("--no-parallel");
                    }
                    let output = invocation
                        .args(arguments)
                        .assert()
                        .success()
                        .get_output()
                        .stdout
                        .clone();
                    let body = if cache == "false" && !parallel {
                        support::project::without_cache_parallel_notice(&output)
                    } else {
                        &output
                    };
                    let found: serde_json::Value = serde_json::from_slice(body).unwrap();
                    assert_eq!(found["summary"]["inspected_file_count"], expected.len());
                    assert_eq!(found["summary"]["target_file_count"], expected.len());
                    assert_eq!(found["summary"]["offense_count"], 0);
                    let paths: Vec<_> = found["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|file| file["path"].as_str().unwrap())
                        .collect();
                    assert_eq!(
                        paths, expected,
                        "{arguments:?}, cache={cache}, parallel={parallel}"
                    );
                }
            }
        }
    }
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
fn native_syntax_reports_match_recorded_upstream_without_ruby() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/native_syntax_reports.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 189);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let source = case["source"].as_str().unwrap();
        let directory = project_with_ruby(&[("example.rb", source)], "2.7");
        fs::write(
            directory.path().join(".rubocop.yml"),
            case["yaml"].as_str().unwrap(),
        )
        .unwrap();
        let output = command(directory.path())
            .env("PATH", "")
            .args([
                "--cache",
                "false",
                "--only",
                "Lint/Syntax",
                "-f",
                "json",
                "example.rb",
            ])
            .assert()
            .code(if case["offenses"].as_array().unwrap().is_empty() {
                0
            } else {
                1
            })
            .get_output()
            .stdout
            .clone();
        let found: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(found["summary"]["target_file_count"], 1, "{name}");
        assert_eq!(found["summary"]["inspected_file_count"], 1, "{name}");
        assert_eq!(found["files"][0]["offenses"], case["offenses"], "{name}");
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            source.as_bytes()
        );
    }
}

#[test]
fn an_unsupported_prism_target_fails_before_autocorrect() {
    let source = "foo( 1 )\n";
    let directory = project_with_ruby(&[("example.rb", source)], "3.2");
    fs::write(
        directory.path().join(".rubocop.yml"),
        "AllCops:\n  TargetRubyVersion: 3.2\n  ParserEngine: parser_prism\n  NewCops: disable\n",
    )
    .unwrap();
    let output = command(directory.path())
        .args(["-A", "example.rb"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "RuboCop supports target Ruby versions 3.3 and above with Prism. Specified target Ruby version: 3.2"
    ));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        source.as_bytes()
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
fn endless_method_with_a_command_rhs_is_valid_syntax() {
    let directory = project_with_ruby(&[], "4.0");
    let output = lint_stdin(
        directory.path(),
        "Lint/Syntax",
        "def foo = puts \"hello\"\n",
    )
    .success()
    .get_output()
    .stdout
    .clone();
    assert_offenses(&output, &[]);
}

#[test]
fn when_keyword_is_rejected_as_an_argument_but_allowed_as_a_method() {
    let directory = project_with_ruby(&[], "2.7");
    let invalid = lint_stdin(directory.path(), "Lint/Syntax", "foo(when)\n")
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_offenses(
        &invalid,
        &[(
            "Lint/Syntax",
            1,
            5,
            &syntax_message("unexpected token kWHEN", "2.7"),
        )],
    );

    let valid = lint_stdin(
        directory.path(),
        "Lint/Syntax",
        "case value\nwhen 1\n  :one\nelse\n  object.when 2\nend\n",
    )
    .success()
    .get_output()
    .stdout
    .clone();
    assert_offenses(&valid, &[]);
}

#[test]
fn spaced_index_assignment_requires_an_existing_local() {
    let directory = project_with_ruby(&[("example.rb", "v [0] += 1\n")], "2.7");
    for (source, token) in [
        ("v [0] += 1\n", "tOP_ASGN"),
        ("v [0] = 1\n", "tEQL"),
        ("v [0] ||= 1\n", "tOP_ASGN"),
    ] {
        let output = lint_stdin(directory.path(), "Lint/Syntax", source)
            .code(1)
            .get_output()
            .stdout
            .clone();
        assert_offenses(
            &output,
            &[(
                "Lint/Syntax",
                1,
                7,
                &syntax_message(&format!("unexpected token {token}"), "2.7"),
            )],
        );
    }

    for source in ["v = []\nv [0] += 1\n", "v[0] += 1\n"] {
        let output = lint_stdin(directory.path(), "Lint/Syntax", source)
            .success()
            .get_output()
            .stdout
            .clone();
        assert_offenses(&output, &[]);
    }

    command(directory.path())
        .args(["-A", "--force-default-config", "example.rb"])
        .assert()
        .code(1);
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        b"v [0] += 1\n"
    );
}

#[test]
fn spaced_index_assignment_to_a_command_is_not_rewritten_on_disk() {
    for source in [
        "Foo [0] = 1\n",
        "Foo::Bar [0] += 1\n",
        "self.value [0] = 1\n",
        "foo&.bar [0] = 1\n",
        "::Foo [0] = 1\n",
    ] {
        for mode in ["-a", "-A"] {
            let directory = project_with_ruby(&[("example.rb", source)], "2.7");
            let output = command(directory.path())
                .args([
                    mode,
                    "--force-default-config",
                    "--format",
                    "json",
                    "example.rb",
                ])
                .assert()
                .code(1)
                .get_output()
                .stdout
                .clone();
            let found = offenses(&output);
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].cop_name, "Lint/Syntax");
            assert_eq!(found[0].severity, "fatal");
            assert!(!found[0].correctable);
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                source.as_bytes()
            );
        }
    }
}

#[test]
fn visibility_splats_are_not_erased_by_autocorrect() {
    let source =
        "class Foo\n  METHOD_NAMES = [:bar]\n  def bar; end\n  protected *METHOD_NAMES\nend\n";
    for mode in ["-a", "-A"] {
        let directory = project_with_ruby(
            &[
                ("example.rb", source),
                (
                    ".rubocop.yml",
                    "Style/AccessModifierDeclarations:\n  AllowModifiersOnSymbols: false\n",
                ),
            ],
            "2.7",
        );
        let output = command(directory.path())
            .args([
                mode,
                "--only",
                "Style/AccessModifierDeclarations",
                "-f",
                "json",
                "example.rb",
            ])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        let found = offenses(&output);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].cop_name, "Style/AccessModifierDeclarations");
        assert!(!found[0].correctable);
        // 本家は splat が指定した既存メソッドを動かせず、可視性の宣言も残す。
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            source.as_bytes()
        );
    }
}

#[test]
fn yaml_legacy_booleans_preserve_quoted_strings() {
    for (value, count) in [
        ("no", 1),
        ("off", 1),
        ("NO", 1),
        ("Off", 1),
        ("!<tag:yaml.org,2002:bool> off", 1),
        ("!!str no", 0),
        ("nO", 1),
        ("oFf", 1),
        ("fAlSe", 1),
        ("!example no", 1),
        ("!!bool 'off'", 1),
        ("yes", 0),
        ("on", 0),
        ("'no'", 0),
        ("'off'", 0),
        ("'false'", 0),
    ] {
        let yaml = format!("Naming/PredicatePrefix:\n  UseSorbetSigs: {value}\n");
        let directory = project_with_ruby(&[("example.rb", "def is_attr; end\n")], "2.7");
        // YAML 1.2 のシリアライザを通すと 'no' の引用符が落ちるため、実際の設定をそのまま読む。
        fs::write(directory.path().join(".rubocop.yml"), yaml).unwrap();
        let output = command(directory.path())
            .args([
                "--only",
                "Naming/PredicatePrefix",
                "-f",
                "json",
                "example.rb",
            ])
            .assert()
            .code(if count == 0 { 0 } else { 1 })
            .get_output()
            .stdout
            .clone();
        assert_eq!(offenses(&output).len(), count, "UseSorbetSigs: {value}");
    }
}

#[test]
fn typed_cop_settings_match_recorded_upstream_reports_and_disk_bytes() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/typed_cop_settings.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 104);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let cop = case["cop"].as_str().unwrap();
        let directory =
            project_with_ruby(&[("example.rb", case["source"].as_str().unwrap())], "2.7");
        // 引用符とタグの区別を試すので、YAML の再シリアライズを経由しない。
        fs::write(
            directory.path().join(".rubocop.yml"),
            case["yaml"].as_str().unwrap(),
        )
        .unwrap();
        for (correcting, expected_key) in [(false, "offenses"), (true, "corrected_offenses")] {
            let expected = &case[expected_key];
            let uncorrected = expected
                .as_array()
                .unwrap()
                .iter()
                .any(|offense| offense["corrected"] != true);
            let mut invocation = command(directory.path());
            invocation.args([
                "--cache",
                "false",
                "--only",
                cop,
                "-f",
                "json",
                "example.rb",
            ]);
            if correcting {
                invocation.arg("-A");
            }
            let output = invocation
                .assert()
                .code(if uncorrected { 1 } else { 0 })
                .get_output()
                .stdout
                .clone();
            let found: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(found["summary"]["target_file_count"], 1, "{name}");
            assert_eq!(found["summary"]["inspected_file_count"], 1, "{name}");
            assert_eq!(
                &found["files"][0]["offenses"], expected,
                "{name}, {expected_key}"
            );
        }
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            if case["corrected_source"] != case["source"] {
                native_lf_disk_correction(case["corrected_source"].as_str().unwrap().as_bytes())
            } else {
                case["source"].as_str().unwrap().as_bytes().to_vec()
            },
            "{name}"
        );
    }
}

#[test]
fn string_concatenation_preserves_meta_hex_values_on_disk() {
    for (target, literal, corrected) in [
        ("2.7", "\\M-\\x61", "\\xE1"),
        ("3.3", "\\M-\\x61", "\\xE1"),
        ("2.7", "\\x61", "a"),
        ("2.7", "\\M-a", "\\xE1"),
    ] {
        let before = format!("name = 'x'\nputs((\"{literal}\" + name).b.unpack1('H*'))\n");
        for mode in ["-a", "-A"] {
            let directory = project_with_ruby(&[("example.rb", &before)], target);
            let applied = mode == "-A";
            let output = command(directory.path())
                .args([
                    "--cache",
                    "false",
                    "--only",
                    "Style/StringConcatenation",
                    "-f",
                    "json",
                    mode,
                    "example.rb",
                ])
                .assert()
                .code(if applied { 0 } else { 1 })
                .get_output()
                .stdout
                .clone();
            let document: serde_json::Value = serde_json::from_slice(&output).unwrap();
            let reported = document["files"][0]["offenses"].as_array().unwrap();
            assert_eq!(reported.len(), 1, "{target}: {literal}: {mode}");
            assert_eq!(reported[0]["corrected"], applied);
            assert_eq!(reported[0]["correctable"], true);
            assert_eq!(reported[0]["location"]["line"], 2);
            assert_eq!(reported[0]["location"]["column"], 7);
            let after = if applied {
                format!("name = 'x'\nputs((\"{corrected}#{{name}}\").b.unpack1('H*'))\n")
            } else {
                before.clone()
            };
            // 0xE1 のエスケープをファイル上で固定し、別の cop の補正に隠れない形で検査する。
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                if applied {
                    native_lf_disk_correction(after.as_bytes())
                } else {
                    after.as_bytes().to_vec()
                }
            );
            if applied {
                command(directory.path())
                    .args([
                        "--cache",
                        "false",
                        "--only",
                        "Style/StringConcatenation",
                        "-A",
                        "example.rb",
                    ])
                    .assert()
                    .success();
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    if applied {
                        native_lf_disk_correction(after.as_bytes())
                    } else {
                        after.as_bytes().to_vec()
                    }
                );
            }
        }
    }
}

#[test]
fn useless_assignment_keeps_a_valid_implicit_array_rhs_on_disk() {
    for mode in ["-a", "-A"] {
        for (before, after, corrected) in [
            ("result = foo, '\\n'\n", "result = foo, '\\n'\n", false),
            ("result = [foo, '\\n']\n", "[foo, '\\n']\n", true),
            ("result = foo\n", "foo\n", true),
        ] {
            let directory = project_with_ruby(&[("example.rb", before)], "2.7");
            let output = command(directory.path())
                .args([
                    "--cache",
                    "false",
                    "--only",
                    "Lint/UselessAssignment",
                    "-f",
                    "json",
                    mode,
                    "--fail-level",
                    "fatal",
                    "example.rb",
                ])
                .assert()
                .code(if corrected { 0 } else { 2 })
                .get_output()
                .clone();
            assert_eq!(
                String::from_utf8_lossy(&output.stderr).contains("introduced a syntax error"),
                !corrected,
                "{mode}: {before}"
            );
            let found: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let reported = found["files"][0]["offenses"].as_array().unwrap();
            assert_eq!(reported.len(), 1, "{mode}: {before}");
            assert_eq!(reported[0]["corrected"], corrected, "{mode}: {before}");
            assert_eq!(reported[0]["correctable"], true, "{mode}: {before}");
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                if corrected {
                    native_lf_disk_correction(after.as_bytes())
                } else {
                    after.as_bytes().to_vec()
                },
                "{mode}: {before}"
            );
        }
    }
}

#[test]
fn spaced_index_assignment_with_missing_rhs_stops_recovery_at_operator() {
    let directory = project_with_ruby(&[], "2.7");
    for source in ["v [0] += ;\n", "v [0] =\n", "v [0] += ;\nfoo(when)\n"] {
        let output = lint_stdin(directory.path(), "Lint/Syntax", source)
            .code(1)
            .get_output()
            .stdout
            .clone();
        assert_offenses(
            &output,
            &[(
                "Lint/Syntax",
                1,
                7,
                &syntax_message(
                    if source.contains("+=") {
                        "unexpected token tOP_ASGN"
                    } else {
                        "unexpected token tEQL"
                    },
                    "2.7",
                ),
            )],
        );
    }

    let earlier_error = lint_stdin(directory.path(), "Lint/Syntax", "foo(when)\nv [0] += ;\n")
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_offenses(
        &earlier_error,
        &[
            (
                "Lint/Syntax",
                1,
                5,
                &syntax_message("unexpected token kWHEN", "2.7"),
            ),
            (
                "Lint/Syntax",
                2,
                7,
                &syntax_message("unexpected token tOP_ASGN", "2.7"),
            ),
        ],
    );

    let later_error = lint_stdin(directory.path(), "Lint/Syntax", "v [0] += 1\nfoo(when)\n")
        .code(1)
        .get_output()
        .stdout
        .clone();
    // 本家 1.89.0 の実出力では、右辺がある場合もこの代入で打ち切り、後続の `when` は報告しない。
    assert_offenses(
        &later_error,
        &[(
            "Lint/Syntax",
            1,
            7,
            &syntax_message("unexpected token tOP_ASGN", "2.7"),
        )],
    );
}

#[test]
fn numbered_parameter_index_assignment_follows_target_ruby_version() {
    let source = "[[]].each { _1 [0] += 1 }\n";
    let older = project_with_ruby(&[], "3.1");
    let invalid = lint_stdin(older.path(), "Lint/Syntax", source)
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_offenses(
        &invalid,
        &[(
            "Lint/Syntax",
            1,
            20,
            &syntax_message("unexpected token tOP_ASGN", "3.1"),
        )],
    );

    let newer = project_with_ruby(&[], "3.2");
    for source in [source, "_1 [0] += 1\n", "[[]].each { _2 [0] += 1 }\n"] {
        let valid = lint_stdin(newer.path(), "Lint/Syntax", source)
            .success()
            .get_output()
            .stdout
            .clone();
        assert_offenses(&valid, &[]);
    }
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
        fs::read(directory.path().join("example.rb")).unwrap(),
        native_lf_disk_correction("value = 10_000\n".as_bytes())
    );
}

#[test]
fn indentation_keeps_a_nested_begin_in_an_ensure_body_on_disk() {
    // 本家 1.89.0 の lint / -a / -A の全診断と書き戻しを実測した最小再現。
    let before = "foo do\n  value\nensure\n    begin\n      cleanup\n    rescue StandardError\n      nil\n    end\nend\n";
    let after = "foo do\n  value\nensure\n  begin\n    cleanup\n  rescue StandardError\n    nil\n  end\nend\n";
    for mode in ["-a", "-A"] {
        let directory = project_with_ruby(&[("example.rb", before)], "2.7");
        for correcting in [false, true] {
            let mut invocation = command(directory.path());
            invocation.args([
                "--cache",
                "false",
                "--only",
                "Layout/IndentationWidth",
                "-f",
                "json",
                "example.rb",
            ]);
            if correcting {
                invocation.arg(mode);
            }
            let output = invocation
                .assert()
                .code(if correcting { 0 } else { 1 })
                .get_output()
                .stdout
                .clone();
            let found: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(
                found["files"][0]["offenses"],
                serde_json::json!([{
                    "severity": "convention",
                    "message": "Use 2 (not 4) spaces for indentation.",
                    "cop_name": "Layout/IndentationWidth",
                    "corrected": correcting,
                    "correctable": true,
                    "location": {
                        "start_line": 4, "start_column": 1,
                        "last_line": 4, "last_column": 4,
                        "length": 4, "line": 4, "column": 1
                    }
                }]),
                "{mode}, correcting={correcting}"
            );
        }
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            native_lf_disk_correction(after.as_bytes()),
            "{mode}"
        );
        // 適正な字下げでは診断も二度目の書き換えも起きない。
        command(directory.path())
            .args([mode, "--only", "Layout/IndentationWidth", "example.rb"])
            .assert()
            .success();
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            native_lf_disk_correction(after.as_bytes())
        );
    }
}

#[test]
fn case_like_if_does_not_write_invalid_ruby() {
    for nested in [
        "if x == 3\n    c\n  end",
        "unless x == 3\n    c\n  end",
        "c if x == 3",
        "c unless x == 3",
    ] {
        let source = format!("if x == 1\n  a\nelsif x == 2\n  b\nelse\n  {nested}\nend\n");
        let directory = project(&[("example.rb", &source)]);

        command(directory.path())
            .args(["-A", "--only", "Style/CaseLikeIf", "example.rb"])
            .assert()
            .code(1);

        assert_eq!(
            fs::read_to_string(directory.path().join("example.rb")).unwrap(),
            source
        );
    }
}

#[test]
fn safe_autocorrect_keeps_hash_arguments_in_safe_navigation_calls() {
    let directory = project(&[("example.rb", "recv&.foo Hash.new, 1\n")]);
    command(directory.path())
        .args(["-a", "--only", "Style/EmptyLiteral", "example.rb"])
        .assert()
        .success();
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        native_lf_disk_correction(b"recv&.foo({}, 1)\n")
    );
}

#[test]
fn guard_clause_does_not_move_a_first_assignment_behind_its_read() {
    let before = "def configure(failures)\n  if error = failures.pop\n    raise error\n  else\n    super()\n  end\nend\n";
    let directory = project(&[("example.rb", before)]);
    command(directory.path())
        .args(["-a", "--only", "Style/GuardClause", "example.rb"])
        .assert()
        .code(1);
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        before.as_bytes()
    );
}

#[test]
fn guard_clause_can_move_a_read_of_an_existing_local() {
    let before = "def configure(failures)\n  error = nil\n  if error = failures.pop\n    raise error\n  else\n    super()\n  end\nend\n";
    let directory = project(&[("example.rb", before)]);
    command(directory.path())
        .args(["-a", "--only", "Style/GuardClause", "example.rb"])
        .assert()
        .success();
    let after = fs::read_to_string(directory.path().join("example.rb")).unwrap();
    assert!(
        after.contains("raise error if error = failures.pop"),
        "{after}"
    );
}

#[test]
fn local_variable_before_plus_is_not_an_ambiguous_call() {
    let source = "def run\n  [1].collect { |i| i +'1' }\nend\n";
    for source in [source.to_owned(), source.replace('\n', "\r\n")] {
        for correction in [None, Some("-a"), Some("-A")] {
            let directory = project(&[("example.rb", &source)]);
            let mut run = command(directory.path());
            run.args(["--only", "Lint/AmbiguousOperator", "--cache", "false"]);
            if let Some(flag) = correction {
                run.arg(flag);
            }
            run.arg("example.rb").assert().success();
            // 本家は曖昧性を報告せず、補正でも LF / CRLF の原本を変更しない。
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                source.as_bytes()
            );
        }
    }
}

/// ブロック付きの呼び出しは本家では `send` ではなく `block` ノードなので、括弧付きの引数の中に
/// あっても `Style/NestedParenthesizedCalls` の入れ子にならない。TargetRubyVersion 2.7 では本家が
/// この本文を構文エラーにする (#22) ので、3.4 で比べる。本家 1.89.0 はどの cop も報告しない。
#[test]
fn a_nested_call_with_a_do_block_is_not_a_nested_parenthesized_call() {
    let source = "# frozen_string_literal: true\n\nputs(foo bar do\n  1\nend)\n";
    for source in [source.to_owned(), source.replace('\n', "\r\n")] {
        for correction in [None, Some("-A")] {
            let directory = project_with_ruby(&[("example.rb", &source)], "3.4");
            let mut run = command(directory.path());
            run.args([
                "--only",
                "Style/NestedParenthesizedCalls",
                "--cache",
                "false",
            ]);
            if let Some(flag) = correction {
                run.arg(flag);
            }
            run.arg("example.rb").assert().success();
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                source.as_bytes()
            );
        }
    }
}

/// 本家が報告した cop と、その開始の行・桁。
type UpstreamOffense = (&'static str, u64, u64);

/// 既定設定で検査した診断 (cop・行・桁) と、`-A` で補正した本文を、本家 1.89.0 の実出力と比べる。
/// LF と CRLF の両方で動かす。`Layout/EndOfLine` の native は OS に従うため、Windows では LF、
/// それ以外では CRLF の原本に診断が出る。補正後も原本の改行を保つのは既存の意図的差分。
fn assert_matches_upstream(cases: &[(&str, &[UpstreamOffense], &str)]) {
    for (source, offenses, corrected) in cases {
        for ending in ["\n", "\r\n"] {
            let original = source.replace('\n', ending);
            let directory = project(&[("example.rb", &original)]);
            let output = command(directory.path())
                .args([
                    "--force-default-config",
                    "--cache",
                    "false",
                    "--format",
                    "json",
                    "example.rb",
                ])
                .output()
                .unwrap();
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let mut actual: Vec<(String, u64, u64)> = report["files"][0]["offenses"]
                .as_array()
                .unwrap()
                .iter()
                .map(|offense| {
                    (
                        offense["cop_name"].as_str().unwrap().to_owned(),
                        offense["location"]["start_line"].as_u64().unwrap(),
                        offense["location"]["start_column"].as_u64().unwrap(),
                    )
                })
                .collect();
            actual.sort();
            let mut expected: Vec<(String, u64, u64)> = offenses
                .iter()
                .map(|&(cop, line, column)| (cop.to_owned(), line, column))
                .collect();
            // 本家の native は Windows で CRLF、それ以外で LF。両設定を本家 CLI で実測した。
            let native = if cfg!(windows) { "\r\n" } else { "\n" };
            if ending != native {
                expected.push(("Layout/EndOfLine".to_owned(), 1, 1));
                expected.sort();
            }
            assert_eq!(actual, expected, "{original:?}");

            let directory = project(&[("example.rb", &original)]);
            command(directory.path())
                .args([
                    "--force-default-config",
                    "--cache",
                    "false",
                    "-A",
                    "example.rb",
                ])
                .output()
                .unwrap();
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                if ending == "\n" && source != corrected {
                    native_lf_disk_correction(corrected.as_bytes())
                } else {
                    corrected.replace('\n', ending).into_bytes()
                },
                "{original:?}"
            );
        }
    }
}

/// grammar が引数の始まりと読む `name +x` も、`name` がその位置でローカル変数なら二項演算。
/// 束縛は字句の順に決まり (修飾子の本文は条件より先、heredoc の本文は開始の直後)、`def` は
/// 外側の変数を見ない。全 cop を既定設定で動かし、変数の改名・代入の削除・括弧の追加で
/// ローカル変数をメソッド呼び出しへ変えないことを確かめる。期待値は本家 1.89.0 の既定設定での
/// 診断 (cop・行・桁) と `-A` の補正後の本文。CRLF は補正後も CRLF を保つ既存の意図的差分に従う。
#[test]
fn operators_after_local_variables_read_as_binary_in_every_cop() {
    let cases: &[(&str, &[UpstreamOffense], &str)] = &[
        (
            "# frozen_string_literal: true\n\n%w[1].collect { |i| i +'1' }\n",
            &[
                ("Layout/SpaceAroundOperators", 3, 23),
                ("Style/StringConcatenation", 3, 21),
            ],
            "# frozen_string_literal: true\n\n%w[1].collect { |i| \"#{i}1\" }\n",
        ),
        (
            "# frozen_string_literal: true\n\ndef run(count)\n  count +1\nend\n",
            &[("Layout/SpaceAroundOperators", 4, 9)],
            "# frozen_string_literal: true\n\ndef run(count)\n  count + 1\nend\n",
        ),
        (
            "# frozen_string_literal: true\n\n[1].collect { |i| i -1 }\n",
            &[("Layout/SpaceAroundOperators", 3, 21)],
            "# frozen_string_literal: true\n\n[1].collect { |i| i - 1 }\n",
        ),
        (
            "# frozen_string_literal: true\n\ndef run(args)\n  foo = 2\n  foo *args\nend\n",
            &[("Layout/SpaceAroundOperators", 5, 7)],
            "# frozen_string_literal: true\n\ndef run(args)\n  foo = 2\n  foo * args\nend\n",
        ),
        (
            "# frozen_string_literal: true\n\ndef run\n  total = 1\n  puts(total +1)\nend\n",
            &[("Layout/SpaceAroundOperators", 5, 14)],
            "# frozen_string_literal: true\n\ndef run\n  total = 1\n  puts(total + 1)\nend\n",
        ),
        (
            "# frozen_string_literal: true\n\ndef run\n  puts(<<~TEXT, value +1)\n    #{value = 2}\n  TEXT\nend\n",
            &[("Layout/SpaceAroundOperators", 4, 23)],
            "# frozen_string_literal: true\n\ndef run\n  puts(<<~TEXT, value + 1)\n    #{value = 2}\n  TEXT\nend\n",
        ),
        (
            "# frozen_string_literal: true\n\ndef run\n  value = 1 if value +1\nend\n",
            &[
                ("Layout/SpaceAroundOperators", 4, 22),
                ("Lint/UselessAssignment", 4, 3),
            ],
            "# frozen_string_literal: true\n\ndef run\n  1 if value + 1\nend\n",
        ),
        (
            "# frozen_string_literal: true\n\ndef run\n  value +1 if (value = 2)\nend\n",
            &[
                ("Lint/AmbiguousOperator", 4, 9),
                ("Lint/UselessAssignment", 4, 16),
            ],
            "# frozen_string_literal: true\n\ndef run\n  value(+1)\nend\n",
        ),
        (
            "# frozen_string_literal: true\n\nvalue = 1\ndef run\n  value +1\nend\nputs value\n",
            &[("Lint/AmbiguousOperator", 5, 9)],
            "# frozen_string_literal: true\n\nvalue = 1\ndef run\n  value(+1)\nend\nputs value\n",
        ),
        // 新しい grammar は `.ref_for('HEAD')` を括弧付きの引数と読む。ブロック付きの呼び出しは
        // 本家では `block` ノードなので、`Style/NestedParenthesizedCalls` の入れ子にならない。
        (
            "# frozen_string_literal: true\n\nref = update_git 'bar', gemspec: false do |s|\n  s.write 1\nend.ref_for('HEAD')\nputs ref\n",
            &[],
            "# frozen_string_literal: true\n\nref = update_git 'bar', gemspec: false do |s|\n  s.write 1\nend.ref_for('HEAD')\nputs ref\n",
        ),
        // 演算子が行継続の直後でも、ローカル変数の後なら二項演算。
        (
            "# frozen_string_literal: true\n\ncount = 1\ncount \\\n+1\n",
            &[],
            "# frozen_string_literal: true\n\ncount = 1\ncount \\\n+1\n",
        ),
        // 名前と行継続が接して空白が無くても、仮想の手がかりで二項演算として読む。
        (
            "# frozen_string_literal: true\n\ncount = 1\ncount\\\n+1\n",
            &[],
            "# frozen_string_literal: true\n\ncount = 1\ncount\\\n+1\n",
        ),
    ];
    assert_matches_upstream(cases);
}

/// ローカル変数の後の `+1` に `do` が続く形は、Ruby では `i + 1` の後に `do` がある構文エラー。
/// ブロック付きのメソッド呼び出しと読んで変数を使っていないと報告したり、補正したりしない。
/// 本家は `Lint/Syntax` を `do` の位置 (4:6) に報告する。sonicop の構文エラーの位置は main の時点
/// から本家と違うので、ここでは構文エラーだけが報告されることを確かめる。
#[test]
fn a_block_after_an_operator_on_a_local_variable_is_a_syntax_error() {
    let source = "# frozen_string_literal: true\n\ni = 1\ni +1 do\nend\n";
    let continued = source.replace("i +1", "i\\\n+1");
    for source in [source.to_owned(), continued]
        .into_iter()
        .flat_map(|source| {
            let crlf = source.replace('\n', "\r\n");
            [source, crlf]
        })
    {
        let directory = project(&[("example.rb", &source)]);
        let output = command(directory.path())
            .args([
                "--force-default-config",
                "--cache",
                "false",
                "--format",
                "json",
                "example.rb",
            ])
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let cops: Vec<&str> = report["files"][0]["offenses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|offense| offense["cop_name"].as_str().unwrap())
            .filter(|cop| *cop != "Layout/EndOfLine")
            .collect();
        assert_eq!(cops, ["Lint/Syntax"], "{source:?}");

        command(directory.path())
            .args([
                "--force-default-config",
                "--cache",
                "false",
                "-A",
                "example.rb",
            ])
            .output()
            .unwrap();
        assert_eq!(
            fs::read_to_string(directory.path().join("example.rb")).unwrap(),
            source,
            "{source:?}"
        );
    }
}

/// 本文の補間で開いた heredoc は、同じ行で後から開いた heredoc より先に読まれる。開始の順と
/// 本文の順が食い違うので、n 番目どうしで組にすると、字下げの補正が別の本文を書き換えたり、
/// 変数の読み取りを取り違えたりする。期待値は本家 1.89.0 の実出力。
#[test]
fn heredocs_pair_with_their_openers_in_reading_order() {
    assert_matches_upstream(&[(
        "# frozen_string_literal: true\n\ndef run\n  puts(<<~A, <<~B)\n    #{puts(<<~C)}\n      #{x}\n    C\n  A\n    #{x = 1}\n  B\nend\n",
        &[("Lint/UselessAssignment", 9, 7)],
        "# frozen_string_literal: true\n\ndef run\n  puts(<<~A, <<~B)\n    #{puts(<<~C)}\n      #{x}\n    C\n  A\n    1\n  B\nend\n",
    )]);
}

#[test]
fn modifier_body_does_not_use_a_variable_first_assigned_in_its_condition() {
    let directory = project(&[(
        "example.rb",
        "def run(failures)\n  raise error if error = failures.pop\nend\n",
    )]);
    let output = command(directory.path())
        .args(["--only", "Lint/UselessAssignment", "example.rb"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8_lossy(&output).contains("Lint/UselessAssignment"),
        "{}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn modifier_body_uses_a_variable_declared_before_its_condition() {
    let directory = project(&[(
        "example.rb",
        "def run(value)\n  puts value if value = 1\nend\n",
    )]);
    command(directory.path())
        .args(["--only", "Lint/UselessAssignment", "example.rb"])
        .assert()
        .success();
}

#[test]
fn modifier_body_keeps_a_prior_assignment_referenced() {
    let directory = project(&[(
        "example.rb",
        "def run(receiver)\n  value = nil\n  puts value if (value = receiver.call) != ''\nend\n",
    )]);
    command(directory.path())
        .args(["--only", "Lint/UselessAssignment", "example.rb"])
        .assert()
        .success();
}

#[test]
fn directory_discovery_uses_the_starting_directories_config() {
    let directory = project(&[
        (
            "nested/.rubocop.yml",
            "AllCops:\n  Exclude:\n    - skip.rb\n",
        ),
        ("nested/keep.rb", "x = 1\n"),
        ("nested/skip.rb", "x = 1\n"),
    ]);
    let root = command(directory.path())
        .args(["-L", "."])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        root,
        native_textmode_expected(b"nested/keep.rb\nnested/skip.rb\n")
    );

    let nested = command(directory.path())
        .args(["-L", "nested"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(nested, native_textmode_expected(b"nested/keep.rb\n"));
}

#[test]
fn crlf_range_length_and_end_location_match_rubocop() {
    let directory = project(&[
        (
            "example.rb",
            "# frozen_string_literal: true\r\n\r\nputs 1\r\n",
        ),
        (".rubocop.yml", "Layout/EndOfLine:\n  EnforcedStyle: lf\n"),
    ]);
    let output = command(directory.path())
        .args([
            "--only",
            "Layout/EndOfLine",
            "--format",
            "json",
            "example.rb",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let found = offenses(&output);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].location.last_line, 3);
    assert_eq!(found[0].location.last_column, 1);
    assert_eq!(found[0].location.length, 31);
}

#[test]
fn crlf_in_debugger_message_is_normalized() {
    let directory = project(&[("example.rb", "binding\r\n  .pry\r\n")]);
    let output = command(directory.path())
        .args(["--only", "Lint/Debugger", "--format", "json", "example.rb"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let found = offenses(&output);
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].message,
        "Remove debugger entry point `binding\n  .pry`."
    );
    assert_eq!(found[0].location.length, 14);
}

#[test]
fn crlf_comments_and_blank_lines_match_rubocop_locations() {
    let cases = [
        (
            "Layout/LeadingCommentSpace",
            "#foo\r\n",
            "Missing space after `#`.",
            (1, 1, 1, 4, 4),
        ),
        (
            "Style/CommentedKeyword",
            "def foo # bar\r\nend\r\n",
            "Do not place comments on the same line as the `def` keyword.",
            (1, 9, 1, 13, 5),
        ),
        (
            "Layout/EmptyLinesAroundModuleBody",
            "module A\r\n\r\n  x\r\nend\r\n",
            "Extra empty line detected at module body beginning.",
            (2, 1, 3, 1, 1),
        ),
    ];
    for (cop, source, message, location) in cases {
        let directory = project(&[("example.rb", source)]);
        let output = command(directory.path())
            .args(["--only", cop, "--format", "json", "example.rb"])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        let found = offenses(&output);
        assert_eq!(found.len(), 1, "{cop}");
        assert_eq!(found[0].message, message, "{cop}");
        let at = &found[0].location;
        assert_eq!(
            (
                at.start_line,
                at.start_column,
                at.last_line,
                at.last_column,
                at.length
            ),
            location,
            "{cop}"
        );
    }
}

#[test]
fn crlf_inside_cop_messages_matches_rubocop() {
    let cases = [
        (
            "Lint/AmbiguousBlockAssociation",
            "foo a.map { |x|\r\n  x\r\n}\r\n",
            "Parenthesize the param `a.map { |x|\n  x\n}` to make sure that the block will be associated with the `a.map` method call.",
        ),
        (
            "Style/GuardClause",
            "def f\r\n  if a\r\n    b or\r\n      raise(E)\r\n  else\r\n    c\r\n  end\r\n  d\r\nend\r\n",
            "Use a guard clause (`b or\n      raise(E) if a`) instead of wrapping the code inside a conditional expression.",
        ),
    ];
    for (cop, source, message) in cases {
        let directory = project(&[("example.rb", source)]);
        let output = command(directory.path())
            .args(["--only", cop, "--format", "json", "example.rb"])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        let found = offenses(&output);
        assert_eq!(found.len(), 1, "{cop}");
        assert_eq!(found[0].message, message, "{cop}");
    }
}

/// 本家 1.89.0 の CLI で検査・-a・-A の診断全項目と補正後の本文を記録した。
/// 表示の CRLF だけを LF にし、単独 CR・エスケープ文字・原本の改行形式は保つ。
#[test]
fn nested_call_messages_normalize_crlf_without_changing_disk_bytes() {
    let cop = "Style/NestedParenthesizedCalls";
    let cases = [
        (
            "puts(foo bar { |x|\n  x\n})\n",
            "puts(foo(bar { |x|\n  x\n}))\n",
            "foo bar { |x|\n  x\n}",
            (3, 1, 19),
        ),
        (
            "puts(foo bar { |猫|\n  猫\n})\n",
            "puts(foo(bar { |猫|\n  猫\n}))\n",
            "foo bar { |猫|\n  猫\n}",
            (3, 1, 19),
        ),
        (
            "puts(foo \"a\nb\")\n",
            "puts(foo(\"a\nb\"))\n",
            "foo \"a\nb\"",
            (2, 2, 9),
        ),
        (
            "puts(foo bar { 1 })\n",
            "puts(foo(bar { 1 }))\n",
            "foo bar { 1 }",
            (1, 18, 13),
        ),
        (
            "puts(foo \"a\rb\")\n",
            "puts(foo(\"a\rb\"))\n",
            "foo \"a\rb\"",
            (1, 14, 9),
        ),
        (
            "puts(foo \"a\\r\\nb\")\n",
            "puts(foo(\"a\\r\\nb\"))\n",
            "foo \"a\\r\\nb\"",
            (1, 17, 12),
        ),
    ];
    for (source, corrected, nested, (last_line, last_column, length)) in cases {
        for newline in ["\n", "\r\n"] {
            let source = source.replace('\n', newline);
            for mode in [None, Some("-a"), Some("-A")] {
                let directory = project(&[("example.rb", &source)]);
                let mut invocation = command(directory.path());
                invocation.args(["--cache", "false", "--only", cop, "--format", "json"]);
                if let Some(mode) = mode {
                    invocation.arg(mode);
                }
                let output = invocation
                    .arg("example.rb")
                    .assert()
                    .code(if mode.is_some() { 0 } else { 1 })
                    .get_output()
                    .stdout
                    .clone();
                let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
                assert_eq!(
                    report["files"][0]["offenses"],
                    serde_json::json!([{
                        "severity": "convention",
                        "message": format!("Add parentheses to nested method call `{nested}`."),
                        "cop_name": cop,
                        "corrected": mode.is_some(),
                        "correctable": true,
                        "location": {
                            "start_line": 1, "start_column": 6,
                            "last_line": last_line, "last_column": last_column,
                            "length": length, "line": 1, "column": 6
                        }
                    }]),
                    "{source:?} {mode:?}"
                );
                let expected = if mode.is_some() {
                    corrected.replace('\n', newline)
                } else {
                    source.clone()
                };
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    if mode.is_some() && newline == "\n" {
                        if expected.contains('\r') {
                            native_lf_code_with_bare_cr(expected.as_bytes())
                        } else {
                            native_lf_disk_correction(expected.as_bytes())
                        }
                    } else {
                        expected.as_bytes().to_vec()
                    },
                    "{source:?} {mode:?}"
                );
            }
        }
    }
}

/// CRLF を正規化する本家の RangeHelp と、原本のバイト位置を使う補正を照合する。
/// 3 cop の全診断項目・終了コード・ディスク本文を、本家 1.89.0 の実 CLI 記録で検査する。
#[test]
fn crlf_continuations_are_corrected_at_the_original_byte_boundaries() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/crlf_continuations.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 16);
    for case in cases {
        let cop = case["cop"].as_str().unwrap();
        for newline in ["\n", "\r\n"] {
            let original = case["source"].as_str().unwrap().replace('\n', newline);
            for mode in [None, Some("-a"), Some("-A")] {
                let expected = &case[if mode.is_some() { "correct" } else { "inspect" }];
                let directory = project(&[("example.rb", &original)]);
                let mut invocation = command(directory.path());
                invocation.args(["--cache", "false", "--only", cop, "--format", "json"]);
                if let Some(mode) = mode {
                    invocation.arg(mode);
                }
                let output = invocation
                    .arg("example.rb")
                    .assert()
                    .code(expected["exit"].as_i64().unwrap() as i32)
                    .get_output()
                    .clone();
                let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                let mut actual = report["files"][0]["offenses"].as_array().unwrap().clone();
                let mut expected_offenses = expected["offenses"].as_array().unwrap().clone();
                actual.sort_by_key(serde_json::Value::to_string);
                expected_offenses.sort_by_key(serde_json::Value::to_string);
                assert_eq!(report["summary"]["target_file_count"], 1);
                assert_eq!(report["summary"]["inspected_file_count"], 1);
                assert_eq!(report["summary"]["offense_count"], actual.len());
                assert_eq!(actual, expected_offenses, "{cop} {original:?} {mode:?}");
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    if newline == "\n" && expected["source"] != case["source"] {
                        native_lf_disk_correction(expected["source"].as_str().unwrap().as_bytes())
                    } else {
                        expected["source"]
                            .as_str()
                            .unwrap()
                            .replace('\n', newline)
                            .into_bytes()
                    },
                    "{cop} {original:?} {mode:?}"
                );
                assert!(output.stderr.is_empty(), "{cop} {original:?} {mode:?}");
            }
        }
    }
}

/// 本家自身が壊す 3 形を記録する。2 形は原本を返し、1 形は本家の通常入力と同じ
/// 有効な補正を行う。診断・corrected・終了コード・本文をその違いまで監視する。
#[test]
fn upstream_continuation_syntax_bugs_keep_safe_candidate_behavior() {
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let cases = manifest["continuation_writeback_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        let cop = case["cop"].as_str().unwrap();
        assert_ne!(case["source"], case["upstream"]["source"]);
        assert!(
            case["upstream"]["offenses"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|offense| offense["cop_name"].as_str() == Some("Lint/Syntax"))
        );
        for newline in ["\n", "\r\n"] {
            let original = case["source"].as_str().unwrap().replace('\n', newline);
            for mode in [None, Some("-a"), Some("-A")] {
                let expected = &case[if mode.is_some() { "sonicop" } else { "initial" }];
                let directory = project(&[("example.rb", &original)]);
                let mut invocation = command(directory.path());
                invocation.args(["--cache", "false", "--only", cop, "--format", "json"]);
                if let Some(mode) = mode {
                    invocation.arg(mode);
                }
                let output = invocation
                    .arg("example.rb")
                    .assert()
                    .code(expected["exit"].as_i64().unwrap() as i32)
                    .get_output()
                    .clone();
                let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["summary"]["target_file_count"], 1);
                assert_eq!(report["summary"]["inspected_file_count"], 1);
                assert_eq!(
                    report["files"][0]["offenses"],
                    serde_json::to_value(&expected["offenses"]).unwrap(),
                    "{original:?} {mode:?}"
                );
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    if newline == "\n" && expected["source"] != case["source"] {
                        native_lf_disk_correction(expected["source"].as_str().unwrap().as_bytes())
                    } else {
                        expected["source"]
                            .as_str()
                            .unwrap()
                            .replace('\n', newline)
                            .into_bytes()
                    }
                );
                assert_eq!(
                    output.stderr,
                    expected["stderr"].as_str().unwrap_or("").as_bytes()
                );
            }
        }
    }
}

#[test]
fn autocorrect_preserves_crlf_in_the_data_section() {
    let source = b"x=1\r\nputs DATA.read.bytes.join(',')\r\n__END__\r\na\r\nb\r\n";
    let directory = project(&[("example.rb", std::str::from_utf8(source).unwrap())]);
    command(directory.path())
        .args(["-a", "--only", "Layout/SpaceAroundOperators", "example.rb"])
        .assert()
        .success();
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        b"x = 1\r\nputs DATA.read.bytes.join(',')\r\n__END__\r\na\r\nb\r\n"
    );
}

#[test]
fn autocorrect_inserts_crlf_in_a_crlf_file() {
    let directory = project(&[("example.rb", "# frozen_string_literal: true\r\nx = 1\r\n")]);
    command(directory.path())
        .args([
            "-a",
            "--only",
            "Layout/EmptyLineAfterMagicComment",
            "example.rb",
        ])
        .assert()
        .success();
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        b"# frozen_string_literal: true\r\n\r\nx = 1\r\n"
    );
}

/// `puts(foo bar { 1 })` には Lint/AmbiguousBlockAssociation と Style/NestedParenthesizedCalls の
/// 両方が括弧を足す。本家は後に結合する NestedParenthesizedCalls を衝突で捨て、
/// `puts(foo(bar { 1 }))` を書いて両方を corrected と報告し、終了 0 になる。2 つの補正が合流して
/// `)` が 2 つ入ると、構文の安全網が書き込みを止めて終了 2 になっていた。CRLF のファイルは CRLF の
/// まま書く (本家は LF で書き戻す。既知の差)。期待値は本家 1.89.0 の実出力。
#[test]
fn a_nested_call_both_cops_parenthesize_is_written_with_one_pair() {
    let only = "Lint/AmbiguousBlockAssociation,Style/NestedParenthesizedCalls";
    for newline in ["\n", "\r\n"] {
        let source =
            "# frozen_string_literal: true\n\nputs(foo bar { 1 })\n".replace('\n', newline);
        let corrected =
            "# frozen_string_literal: true\n\nputs(foo(bar { 1 }))\n".replace('\n', newline);
        for mode in ["-a", "-A"] {
            let directory = project(&[("example.rb", &source)]);
            let output = command(directory.path())
                .args([mode, "--only", only, "--format", "json", "example.rb"])
                .assert()
                .code(0)
                .get_output()
                .stdout
                .clone();
            let reported: Vec<_> = offenses(&output)
                .iter()
                .map(|offense| {
                    (
                        offense.cop_name.clone(),
                        offense.location.start_line,
                        offense.location.start_column,
                        offense.corrected,
                    )
                })
                .collect();
            assert_eq!(
                reported,
                [
                    ("Lint/AmbiguousBlockAssociation".to_owned(), 3, 6, true),
                    ("Style/NestedParenthesizedCalls".to_owned(), 3, 6, true),
                ],
                "{mode} {newline:?}"
            );
            let path = directory.path().join("example.rb");
            assert_eq!(
                fs::read(&path).unwrap(),
                if newline == "\n" {
                    native_lf_disk_correction(corrected.as_bytes())
                } else {
                    corrected.as_bytes().to_vec()
                },
                "{mode} {newline:?}"
            );
            // 次の実行では何も報告せず、本文も変わらない。
            command(directory.path())
                .args(["--only", only, "example.rb"])
                .assert()
                .code(0);
            assert_eq!(
                fs::read(&path).unwrap(),
                if newline == "\n" {
                    native_lf_disk_correction(corrected.as_bytes())
                } else {
                    corrected.as_bytes().to_vec()
                },
                "{mode} {newline:?}"
            );
        }
    }
    // Issue の再現を既定の cop で補正する。改行形式だけは native の OS 差を除く。
    let directory = project(&[(
        "example.rb",
        "# frozen_string_literal: true\n\nputs(foo bar { 1 })\n",
    )]);
    command(directory.path())
        .args(["-A", "--except", "Layout/EndOfLine", "example.rb"])
        .assert()
        .code(0);
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        native_lf_disk_correction(b"# frozen_string_literal: true\n\nputs(foo(bar { 1 }))\n")
    );
}

#[test]
fn autocorrect_rechecks_crlf_as_rubocop_does() {
    let source = "x=1\r\n\r\n\r\n# header\r\nputs x\r\n";
    let directory = project(&[("example.rb", source)]);
    command(directory.path())
        .args([
            "-a",
            "--only",
            "Layout/SpaceAroundOperators,Layout/EmptyLines",
            "example.rb",
        ])
        .assert()
        .success();
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        if cfg!(windows) {
            b"x = 1\r\n\r\n\r\n# header\r\nputs x\r\n".as_slice()
        } else {
            b"x = 1\r\n\r\n# header\r\nputs x\r\n".as_slice()
        }
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
        fs::read(directory.path().join("example.rb")).unwrap(),
        native_lf_disk_correction(expected.as_bytes())
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

    // 本家1.89.0は引数順を保ち、各directoryの展開内だけを並べる。実CLIで同じ入力を再測定した。
    command(directory.path())
        .args(["--list-target-files", "lib", "lib.rb"])
        .assert()
        .success()
        .stdout(native_textmode_expected(b"lib/a.rb\nlib.rb\n"));
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
        .stdout(native_textmode_expected(b"good.rb\n"));
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
/// 情報表示の項目に依存せず、実際のレジストリと設定の名前集合を直接照合する。
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

    let catalog: serde_yaml::Value = serde_yaml::from_str(&listing).unwrap();
    let known = catalog
        .as_mapping()
        .unwrap()
        .keys()
        .map(|name| name.as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    let implemented = sonicop::rules::rule_names().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        known, implemented,
        "本家の cop 名集合と実装のレジストリが食い違う"
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
        fs::read(editor.path().join("example.rb")).unwrap(),
        source.as_bytes(),
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
        fs::read(batch.path().join("example.rb")).unwrap(),
        // `--only` で Lint/Void を外してあるので、残された `1` はそのまま。
        native_lf_disk_correction(b"def foo\n  1\n  2\nend\n"),
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
#[cfg(any(unix, windows))]
#[test]
fn autocorrect_keeps_a_hard_linked_file_shared() {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    let directory = project(&[("one.rb", "y  = 2\nputs y\n")]);
    let other = directory.path().join("other.rb");
    fs::hard_link(directory.path().join("one.rb"), &other).unwrap();

    command(directory.path())
        .args(["-A", "--only", "Layout/ExtraSpacing", "one.rb"])
        .assert()
        .code(0);

    assert!(
        same_file::is_same_file(directory.path().join("one.rb"), &other).unwrap(),
        "-A が hard link の共有を失わせた"
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&other).unwrap().nlink(),
        2,
        "-A が hard link を切った"
    );
    assert_eq!(
        fs::read(&other).unwrap(),
        native_lf_disk_correction("y = 2\nputs y\n".as_bytes()),
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
        stdout.ends_with(
            &String::from_utf8(native_textmode_expected(b"====================\ny = 2\n")).unwrap()
        ),
        "区切りと修正後ソースが末尾に無い:\n{stdout}"
    );
}

#[test]
fn correction_merge_order_matches_recorded_upstream_on_disk() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/correction_merge_order_cases.json")).unwrap();
    for (index, case) in cases.iter().enumerate() {
        let directory = project(&[("example.rb", case["before"].as_str().unwrap())]);
        let args: Vec<_> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap())
            .collect();
        let output = command(directory.path())
            .args(&args)
            .assert()
            .code(case["exit"].as_i64().unwrap() as i32)
            .get_output()
            .stdout
            .clone();
        let actual: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(actual["files"], case["expected"], "case {index}: {args:?}");
        assert_eq!(actual["summary"], case["summary"], "case {index}: {args:?}");
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            if case["after"] != case["before"] {
                native_lf_disk_correction(case["after"].as_str().unwrap().as_bytes())
            } else {
                case["before"].as_str().unwrap().as_bytes().to_vec()
            },
            "case {index}: {args:?}"
        );
    }
}

#[test]
fn nested_configuration_keeps_the_process_directory_in_diagnostic_paths() {
    let directory = project(&[
        (
            "nested/.rubocop.yml",
            "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n",
        ),
        (
            "nested/lib/probe.rb",
            "class Foo\n  def bar; end\n\n  def bar; end\nend\n",
        ),
    ]);
    let canonical = directory
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    // Windows の拡張長接頭辞は、エディタが渡す通常の絶対パスには含まれない。
    let root = canonical.strip_prefix("//?/").unwrap_or(&canonical);
    // 本家も短縮名と長い名前の表記が違えば絶対表示を保つ。cwd と入力を揃え、
    // 設定探索による cwd のすり替えだけを検査する。
    let absolute = std::path::Path::new(root).join("nested/lib/probe.rb");
    let relative = std::path::Path::new("nested/lib/probe.rb");
    for target in [absolute.as_path(), relative] {
        let output = command(std::path::Path::new(root))
            .args([
                "--cache",
                "false",
                "--only",
                "Lint/DuplicateMethods",
                "-f",
                "json",
                "--stdin",
            ])
            .arg(target)
            .write_stdin("class Foo\n  def bar; end\n\n  def bar; end\nend\n")
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            offenses(&output)[0].message,
            "Method `Foo#bar` is defined at both nested/lib/probe.rb:2 and nested/lib/probe.rb:4."
        );
    }
    // 引数を省いた探索も絶対パスを渡すため、stdin 以外の経路を別に確かめる。
    let output = command(std::path::Path::new(root))
        .args([
            "--cache",
            "false",
            "--only",
            "Lint/DuplicateMethods",
            "-f",
            "json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        offenses(&output)[0].message,
        "Method `Foo#bar` is defined at both nested/lib/probe.rb:2 and nested/lib/probe.rb:4."
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

/// 長い連鎖式を条件に持つループでも、cop の走査がプロセスを落とさない。
///
/// `1+1+1+…` は左結合なので、木は項の数だけ深く入れ子になる。
/// `Lint/UnreachableLoop` の子孫探索が再帰だった頃は、rayon のワーカースタック
/// (メインスレッドよりずっと小さい) を食い潰して **1 ファイルの失敗ではなく
/// プロセスの abort** になっていた。`AstIndex::collect` が反復なのと同じ理由で、
/// cop 側の走査も反復でなければならない。
///
/// 実バイナリを起こすのは、この落ち方が in-process ハーネスではテストランナー
/// ごと落としてしまい、どのケースが原因か読めなくなるため。
#[test]
fn a_deeply_nested_loop_condition_does_not_overflow_the_stack() {
    let directory = project(&[]);
    let condition = vec!["1"; 300_000].join("+");
    let source = format!("while {condition}\n  break\nend\n");

    let output = lint_stdin(directory.path(), "Lint/UnreachableLoop", &source)
        .code(1)
        .get_output()
        .stdout
        .clone();

    assert_eq!(
        offense_tuples(&output)
            .iter()
            .map(|offense| offense.0.as_str())
            .collect::<Vec<_>>(),
        ["Lint/UnreachableLoop"],
        "ループ 1 個ぶんの offense が出て、プロセスは生きていること"
    );
}

#[test]
fn prism_frees_deep_left_associative_trees_on_parallel_workers() {
    let source = format!("{}\n", vec!["1"; 300_000].join("+"));
    let directory = project_with_ruby(&[("first.rb", &source), ("second.rb", &source)], "3.3");
    let output = command(directory.path())
        .args([
            "--cache",
            "false",
            "--only",
            "Lint/Syntax",
            "-f",
            "json",
            ".",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let found = report(&output);
    assert_eq!(found.summary.target_file_count, 2);
    assert_eq!(found.summary.inspected_file_count, 2);
    assert_eq!(found.summary.offense_count, 0);
}

/// 診断が旧パーサの設定でも、最後にスクリプトとして壊れた出力はディスクへ書かない。
fn assert_legacy_autocorrect_rollback(cop: &str, source: &str) {
    for version in ["2.7", "3.3"] {
        for mode in ["-a", "-A"] {
            let directory = project_with_ruby(
                &[("example.rb", source), ("other.rb", "puts :ok\n")],
                version,
            );
            let before = command(directory.path())
                .args(["--only", cop, "-f", "json", "example.rb"])
                .assert()
                .code(1)
                .get_output()
                .stdout
                .clone();
            for _ in 0..2 {
                let output = command(directory.path())
                    .args(["--only", cop, mode, "-f", "json", "example.rb", "other.rb"])
                    .assert()
                    .code(2)
                    .get_output()
                    .clone();
                assert!(String::from_utf8_lossy(&output.stderr).contains(
                    "Autocorrection was not written to example.rb because it introduced a syntax error."
                ));
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    source.as_bytes()
                );
                let before: serde_json::Value = serde_json::from_slice(&before).unwrap();
                let after: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(before["files"][0], after["files"][0]);
                assert_eq!(after["summary"]["inspected_file_count"], 2);
            }
        }
    }
}

#[test]
fn multiple_assignment_conditions_are_not_written_without_required_parentheses() {
    assert_legacy_autocorrect_rollback(
        "Style/Semicolon,Style/ParenthesesAroundCondition",
        "until (parent, base = File.split(path); parent == path or parent == \".\")\n  puts base\nend\n",
    );
}

#[test]
fn block_parameter_reflow_does_not_write_an_invalid_trailing_comma() {
    assert_legacy_autocorrect_rollback(
        "Layout/MultilineBlockLayout",
        "[1].each do\n  |b, e = \"end\", pre = nil, post = nil|\n  puts b, e, pre, post\nend\n",
    );
}

#[test]
fn defined_expressions_keep_the_begin_required_by_a_modifier_loop() {
    assert_legacy_autocorrect_rollback(
        "Style/RedundantBegin",
        "defined?(begin\n  value while condition\nend)\n",
    );
}

#[test]
fn removing_a_false_post_loop_does_not_leave_a_top_level_break() {
    assert_legacy_autocorrect_rollback(
        "Lint/LiteralAsCondition",
        "begin\n  if now\n    break if year\n    year = now.year\n  end\nend while false\n",
    );
}

#[test]
fn comment_spacing_preserves_the_ruby_entry_point_of_a_polyglot_script() {
    assert_legacy_autocorrect_rollback(
        "Layout/LeadingCommentSpace",
        "#!/bin/sh\n# -*- ruby -*-\nexec \"${RUBY-ruby}\" \"-x\" \"$0\" \"$@\" && [ ] if false\n#!ruby\nputs :ok\n",
    );
}

#[test]
fn the_upstream_correction_limit_reports_partial_progress_on_disk() {
    // RuboCop 1.89.0 の実測値。200 パスで終了しても、それまでの補正は書き戻す。
    // 収束した成功と区別できるよう、診断・終了コード・次回実行の実バイトを合わせて確認する。
    let before = format!("{}value\n{}", "run do\n".repeat(205), "end\n".repeat(205));
    let directory = project_with_ruby(&[("example.rb", &before)], "2.7");
    let path = directory.path().join("example.rb");
    let run = || {
        command(directory.path())
            .args([
                "--cache",
                "false",
                "--only",
                "Layout/IndentationWidth,Layout/BlockAlignment",
                "-A",
                "example.rb",
            ])
            .output()
            .unwrap()
    };
    let first = run();
    assert_eq!(first.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&first.stderr).contains("Infinite loop"));
    assert_eq!(
        fs::read(&path).unwrap().len(),
        if cfg!(windows) { 86_672 } else { 86_261 }
    );

    let mut expected = String::new();
    for depth in 0..205 {
        expected.push_str(&format!("{}run do\n", "  ".repeat(depth)));
    }
    expected.push_str(&format!("{}value\n", "  ".repeat(205)));
    for depth in (0..205).rev() {
        expected.push_str(&format!("{}end\n", "  ".repeat(depth)));
    }
    for _ in 0..2 {
        let output = run();
        assert_eq!(output.status.code(), Some(0));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Infinite loop"));
        assert_eq!(
            fs::read(&path).unwrap(),
            native_lf_disk_correction(expected.as_bytes())
        );
    }
}

#[test]
fn bom_frozen_comments_match_recorded_upstream_fields_and_bytes() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/frozen_string_bom_cases.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let before = case["before"].as_str().unwrap();
        let after = case["after"].as_str().unwrap();
        let directory = project_with_ruby(&[("example.rb", before)], "2.7");
        fs::write(
            directory.path().join(".rubocop.yml"),
            format!(
                "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\nStyle/FrozenStringLiteralComment:\n  EnforcedStyle: {}\n",
                case["style"].as_str().unwrap()
            ),
        )
        .unwrap();
        let output = command(directory.path())
            .args([
                "--cache",
                "false",
                "--only",
                "Style/FrozenStringLiteralComment",
                "-f",
                "json",
                "example.rb",
            ])
            .output()
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["files"][0]["offenses"], case["offenses"], "{case}");
        let expected_exit = i32::try_from(case["exit"].as_i64().unwrap()).unwrap();
        for _ in 0..2 {
            command(directory.path())
                .args([
                    "--only",
                    "Style/FrozenStringLiteralComment",
                    "-A",
                    "example.rb",
                ])
                .assert()
                .code(expected_exit);
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                if before != after {
                    native_lf_disk_correction(after.as_bytes())
                } else {
                    before.as_bytes().to_vec()
                }
            );
        }
    }
}

#[test]
fn frozen_comment_insertion_uses_the_decoded_first_token() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/frozen_string_token_cases.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let before = case["before"].as_str().unwrap();
        let directory = project_with_ruby(&[("example.rb", before)], "2.7");
        fs::write(
            directory.path().join(".rubocop.yml"),
            case["config"].to_string(),
        )
        .unwrap();
        // 本家もこの形では 200 パスで停止する。補正の収束と互換性を混同しない。
        let output = command(directory.path())
            .args([
                "--cache",
                "false",
                "--only",
                "Style/FrozenStringLiteralComment",
                "-A",
                "example.rb",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{}", case["name"]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("Infinite loop"));
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            native_lf_disk_correction(case["after"].as_str().unwrap().as_bytes()),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn a_frozen_comment_after_bom_encoding_converges_on_disk() {
    for header in [
        "\u{feff}# encoding: utf-8",
        "\u{feff}#!/usr/bin/env ruby\n# encoding: utf-8",
    ] {
        for ending in ["\n", "\r\n"] {
            for mode in ["-a", "-A"] {
                let before = format!("{header}\nputs 'hello'\n").replace('\n', ending);
                let corrected = format!("{header}\n# frozen_string_literal: true\nputs 'hello'\n")
                    .replace('\n', ending);
                let expected = if mode == "-A" { &corrected } else { &before };
                let directory = project_with_ruby(&[("example.rb", &before)], "2.7");
                let path = directory.path().join("example.rb");
                for _ in 0..2 {
                    command(directory.path())
                        .args([
                            "--only",
                            "Style/FrozenStringLiteralComment",
                            mode,
                            "example.rb",
                        ])
                        .assert()
                        .code(if mode == "-A" { 0 } else { 1 });
                    assert_eq!(
                        fs::read(&path).unwrap(),
                        if mode == "-A" && ending == "\n" {
                            native_lf_disk_correction(expected.as_bytes())
                        } else {
                            expected.as_bytes().to_vec()
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn empty_interpolation_cannot_mix_source_encodings() {
    for header in [
        "# coding: US-ASCII",
        "# coding: Shift_JIS",
        "# coding: EUC-JP",
        "# coding: ISO-8859-1",
        "# coding: Windows-31J",
        "# coding: binary",
        "\u{feff}# coding: US-ASCII",
        "  # coding: US-ASCII",
        "# vim: set fileencoding=us-ascii :",
        "# -*- encoding: US-ASCII -*-",
        "#!/usr/bin/env ruby\n# coding: US-ASCII",
    ] {
        assert_legacy_autocorrect_rollback(
            "Lint/EmptyInterpolation",
            &format!("{header}\n\"/\\u1234#{{ }}\\x80/\"\n"),
        );
    }
    assert_legacy_autocorrect_rollback(
        "Lint/EmptyInterpolation",
        "# coding: 646\r\n\"/\\u1234#{ }\\x80/\"\r\n",
    );
}

#[test]
fn encoded_source_rollback_keeps_raw_disk_and_stdin_bytes() {
    let source = "# coding: Shift_JIS\ntext = \"日本語\"\n\"/\\u1234#{ }\\x80/\"\n";
    let (bytes, _, invalid) = encoding_rs::SHIFT_JIS.encode(source);
    assert!(!invalid);
    for mode in ["-a", "-A"] {
        let directory = project_with_ruby(&[], "2.7");
        let path = directory.path().join("example.rb");
        fs::write(&path, &bytes).unwrap();
        command(directory.path())
            .args(["--only", "Lint/EmptyInterpolation", mode, "example.rb"])
            .assert()
            .code(2);
        assert_eq!(fs::read(&path).unwrap(), bytes.as_ref());
        let output = command(directory.path())
            .args([
                "--only",
                "Lint/EmptyInterpolation",
                mode,
                "--stdin",
                "example.rb",
                "-f",
                "simple",
            ])
            .write_stdin(bytes.as_ref())
            .assert()
            .code(2)
            .get_output()
            .stdout
            .clone();
        assert!(output.ends_with(bytes.as_ref()));
        assert_eq!(fs::read(&path).unwrap(), bytes.as_ref());
    }
}

#[test]
fn percent_quoted_byte_escapes_preserve_the_literal_value_on_disk() {
    let source = "# coding: US-ASCII\n%{/\\u1234#{%q\"\\x80\"}/}\n";
    let expected = "# coding: US-ASCII\n%{/\\u1234\\\\x80/}\n";
    for mode in ["-a", "-A"] {
        let directory = project_with_ruby(&[("example.rb", source)], "2.7");
        command(directory.path())
            .args(["--only", "Lint/LiteralInInterpolation", mode, "example.rb"])
            .assert()
            .code(0);
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            native_lf_disk_correction(expected.as_bytes())
        );
    }
}

#[test]
fn legacy_script_validation_rejects_broken_lambdas_and_operator_calls() {
    assert_legacy_autocorrect_rollback("Style/Lambda", "a = lambda { || 1 }\n");
    assert_legacy_autocorrect_rollback("Style/OperatorMethodCall", "a + b.+(c)\n");
}

#[test]
fn empty_regexp_correction_is_withheld_only_in_unsafe_mode() {
    let source = "/#{[]}/\n";
    let directory = project_with_ruby(&[("example.rb", source)], "2.7");
    command(directory.path())
        .args(["--only", "Lint/ArrayLiteralInRegexp", "-a", "example.rb"])
        .assert()
        .code(1);
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        source.as_bytes()
    );
    command(directory.path())
        .args(["--only", "Lint/ArrayLiteralInRegexp", "-A", "example.rb"])
        .assert()
        .code(2);
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        source.as_bytes()
    );
}

#[test]
fn legacy_syntax_rollback_returns_the_original_stdin_buffer() {
    let source = "begin\n  break\nend while false\n";
    let directory = project_with_ruby(&[], "2.7");
    let output = command(directory.path())
        .args([
            "--only",
            "Lint/LiteralAsCondition",
            "-a",
            "--stdin",
            "example.rb",
            "-f",
            "simple",
        ])
        .write_stdin(source)
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .ends_with(&format!("{}{source}", stdin_separator()))
    );
    assert!(!directory.path().join("example.rb").exists());
}

#[test]
fn polyglot_ruby_body_corrections_preserve_a_crlf_launcher() {
    let source =
        "#!/bin/sh\nexec \"${RUBY-ruby}\" \"-x\" \"$0\" \"$@\" && [ ] if false\n#!ruby\nvalue=1\n";
    for newline in ["\n", "\r\n"] {
        let source = source.replace('\n', newline);
        let expected = source.replace("value=1", "value = 1");
        let directory = project_with_ruby(&[("example.rb", &source)], "2.7");
        command(directory.path())
            .args(["--only", "Layout/SpaceAroundOperators", "-a", "example.rb"])
            .assert()
            .code(0);
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            if newline == "\n" {
                native_lf_disk_correction(expected.as_bytes())
            } else {
                expected.as_bytes().to_vec()
            }
        );
        // 引用の無い exec ruby -x は本家 parser も拒否する。Ruby と shell の両方で
        // 有効な launcher を使い、コメントだけの補正を安全ゲートが許すことを確認する。
        let source = format!(
            "#!/bin/sh{newline}# comment {newline}exec \"${{RUBY-ruby}}\" \"-x\" \"$0\" \"$@\" && [ ] if false{newline}#!ruby{newline}puts :ok{newline}"
        );
        let expected = source.replace("# comment ", "# comment");
        fs::write(directory.path().join("example.rb"), &source).unwrap();
        command(directory.path())
            .args(["--only", "Layout/TrailingWhitespace", "-a", "example.rb"])
            .assert()
            .code(0);
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            if newline == "\n" {
                native_lf_disk_correction(expected.as_bytes())
            } else {
                expected.as_bytes().to_vec()
            }
        );
        let source = format!(
            "#!/bin/sh{newline}# example: <<EOF{newline}  exec \"${{RUBY-ruby}}\" \"-x\" \"$0\" \"$@\" && [ ] if false{newline}#!ruby{newline}puts :ok{newline}"
        );
        let expected = source.replace("  exec", "exec");
        fs::write(directory.path().join("example.rb"), &source).unwrap();
        command(directory.path())
            .args(["--only", "Layout/InitialIndentation", "-a", "example.rb"])
            .assert()
            .code(0);
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            if newline == "\n" {
                native_lf_disk_correction(expected.as_bytes())
            } else {
                expected.as_bytes().to_vec()
            }
        );
    }
}

#[test]
fn a_semicolon_in_a_heredoc_name_is_not_a_line_break() {
    let source = "module Example\n  class Test\n    def run\n      items.each do\n        assert_in_out_err([\"-W0\", *ARGV], <<-\"end;\", [\"[1, 1, 1, 400, nil, nil, nil, nil, nil, nil, nil]\"], [], bug13305)\n          puts :ok\n        end;\n      end\n    end\n  end\nend\n";
    // 本家の Layout/LineLength 単独の補正は、終端名の前で折って heredoc を保つ。
    let expected = source.replacen("<<-", "\n<<-", 1);
    for newline in ["\n", "\r\n"] {
        let source = source.replace('\n', newline);
        let expected = expected.replace('\n', newline);
        let directory = project_with_ruby(&[("example.rb", &source)], "2.7");
        command(directory.path())
            .args(["--only", "Layout/LineLength", "-a", "example.rb"])
            .assert()
            .code(0);
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            if newline == "\n" {
                native_lf_disk_correction(expected.as_bytes())
            } else {
                expected.as_bytes().to_vec()
            }
        );
    }
}

#[test]
fn prism_frees_deep_trees_discarded_during_error_recovery() {
    // 不正な代入先は pm_parse の途中で破棄されるので、最後の解放だけを直してもここで落ちる。
    let source = format!("({}) = 2\n", vec!["1"; 300_000].join("+"));
    let directory = project_with_ruby(&[("invalid.rb", &source), ("control.rb", "1\n")], "3.3");
    let output = command(directory.path())
        .args([
            "--cache",
            "false",
            "--only",
            "Lint/Syntax",
            "-f",
            "json",
            ".",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let found = report(&output);
    assert_eq!(found.summary.target_file_count, 2);
    assert_eq!(found.summary.inspected_file_count, 2);
    assert!(found.summary.offense_count > 0);
    assert!(
        found
            .files
            .iter()
            .find(|file| file.path == "control.rb")
            .unwrap()
            .offenses
            .is_empty()
    );
    assert_eq!(
        fs::read(directory.path().join("invalid.rb")).unwrap(),
        source.as_bytes()
    );
}

/// 本家は def・class・begin・括弧の閉じ位置で復帰する。ブロック内や右辺欠落では後続を捨てる。
#[test]
fn command_assignment_errors_resume_only_at_upstream_recovery_boundaries() {
    let directory = project(&[]);
    for (source, expected) in [
        (
            "def m\n foo [0] = 1\n foo(when)\nend\n",
            vec![(2, 10, "tEQL"), (3, 6, "kWHEN")],
        ),
        (
            "class A\n foo [0] = 1\n bar [0] = 2\nend\n",
            vec![(2, 10, "tEQL"), (3, 10, "tEQL")],
        ),
        (
            "while x\n foo [0] = 1\nend\nfoo(when)\n",
            vec![(2, 10, "tEQL"), (4, 5, "kWHEN")],
        ),
        (
            "def m\n foo [0] = 1\n x = {a:}\nend\nbar [0] = 2\n",
            vec![(2, 10, "tEQL"), (3, 9, "tRCURLY"), (5, 9, "tEQL")],
        ),
        (
            "-> { foo [0] = 1 }\nfoo(when)\n",
            vec![(1, 14, "tEQL"), (2, 5, "kWHEN")],
        ),
        (
            "if true\n foo [0] = 1\nend\nfoo(when)\n",
            vec![(2, 10, "tEQL"), (4, 5, "kWHEN")],
        ),
        (
            "def m\n foo [0] = 1\nend\nfoo(when)\n",
            vec![(2, 10, "tEQL"), (4, 5, "kWHEN")],
        ),
        (
            "begin\n ::Foo [0]\nend\nbar [0] = 2\n",
            vec![(2, 8, "tLBRACK"), (4, 9, "tEQL")],
        ),
        (
            "x = (foo [0] = 1)\nfoo(when)\n",
            vec![(1, 14, "tEQL"), (2, 5, "kWHEN")],
        ),
        (
            "class A\n foo [0] = 1\nend\nclass B\n bar [0] = 2\nend\n",
            vec![(2, 10, "tEQL"), (5, 10, "tEQL")],
        ),
        (
            "class A\n foo {Foo [0] = 1}\n foo(when)\nend\nfoo(when)\n",
            vec![(2, 15, "tEQL")],
        ),
        ("def m\n foo [0] =\nend\nfoo(when)\n", vec![(2, 10, "tEQL")]),
    ] {
        let output = lint_stdin(directory.path(), "Lint/Syntax", source)
            .code(1)
            .get_output()
            .stdout
            .clone();
        let actual: Vec<_> = offense_tuples(&output)
            .into_iter()
            .map(|(_, line, column, message)| {
                (line, column, message.split('\n').next().unwrap().to_owned())
            })
            .collect();
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(line, column, token)| (line, column, format!("unexpected token {token}")))
            .collect();
        assert_eq!(actual, expected, "{source}");
    }
}

/// 本家の ConfigLoader は UTF-8 BOM を含む設定を拒否し、検査に進まない。
#[test]
fn a_bom_configuration_is_rejected_before_inspection() {
    let directory = project(&[("example.rb", "def is_attr; end\n")]);
    fs::write(
        directory.path().join(".rubocop.yml"),
        "\u{feff}Naming/PredicatePrefix:\n  UseSorbetSigs: no\n",
    )
    .unwrap();
    let output = command(directory.path())
        .args(["--only", "Naming/PredicatePrefix", "-f", "json"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("did not find expected <document start>")
    );
}

/// 本家の同名引数の一括置換は内側の引数宣言を残し、Prism が拒否する Ruby を作る。
#[test]
fn nested_it_autocorrect_rolls_back_the_file_and_keeps_inspecting() {
    let source = "outer do |transaction|\n  real = transaction\n  inner do |transaction|\n    save = transaction\n  end\nend\n";
    let config = "Style/ItBlockParameter:\n  EnforcedStyle: always\n  Enabled: true\n";
    for mode in ["-a", "-A"] {
        let directory = project_with_ruby(
            &[
                (".rubocop.yml", config),
                ("example.rb", source),
                ("other.rb", "foo do |value|\n  puts value\nend\n"),
            ],
            "3.4",
        );
        let lint = command(directory.path())
            .args([
                "--only",
                "Style/ItBlockParameter",
                "-f",
                "json",
                "example.rb",
            ])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        let output = command(directory.path())
            .args([
                "--only",
                "Style/ItBlockParameter",
                mode,
                "--fail-level",
                "fatal",
                "-f",
                "json",
                "example.rb",
                "other.rb",
            ])
            .assert()
            .code(2)
            .get_output()
            .clone();
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("Autocorrection was not written to example.rb")
        );
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            source.as_bytes()
        );
        assert_eq!(
            fs::read(directory.path().join("other.rb")).unwrap(),
            native_lf_disk_correction(b"foo do \n  puts it\nend\n")
        );
        let before: serde_json::Value = serde_json::from_slice(&lint).unwrap();
        let after: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(before["files"][0], after["files"][0]);
        assert_eq!(after["summary"]["inspected_file_count"], 2);
        command(directory.path())
            .args(["--only", "Style/ItBlockParameter", mode, "example.rb"])
            .assert()
            .code(2);
        assert_eq!(
            fs::read(directory.path().join("example.rb")).unwrap(),
            source.as_bytes()
        );
    }
}

/// 保存時整形に使う stdin も、拒否した補正の代わりに元のバッファを返す。
#[test]
fn nested_it_autocorrect_returns_the_original_stdin_buffer() {
    let source = "outer do |transaction|\n  real = transaction\n  inner do |transaction|\n    save = transaction\n  end\nend\n";
    let directory = project_with_ruby(
        &[(
            ".rubocop.yml",
            "Style/ItBlockParameter:\n  EnforcedStyle: always\n  Enabled: true\n",
        )],
        "3.4",
    );
    for mode in ["-a", "-A"] {
        let output = command(directory.path())
            .args([
                "--only",
                "Style/ItBlockParameter",
                mode,
                "--stdin",
                "example.rb",
                "--fail-level",
                "fatal",
                "-f",
                "simple",
            ])
            .write_stdin(source)
            .assert()
            .code(2)
            .get_output()
            .clone();
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .ends_with(&format!("{}{source}", stdin_separator()))
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("introduced a syntax error"));
        assert!(!directory.path().join("example.rb").exists());
    }
}

/// ASCII の原本で安全な Unicode エスケープを、生の非 ASCII バイトに書き換えない。
#[test]
fn an_unencodable_correction_preserves_the_file_and_finishes_the_json_report() {
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let cases = manifest["encoding_writeback_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(cases.len(), 7);
    for case in cases {
        for mode in ["-a", "-A"] {
            let source = case["source"].as_str().unwrap();
            let directory = project_with_ruby(
                &[("example.rb", source), ("other.rb", "value = \"#{1}\"\n")],
                "2.7",
            );
            let lint = command(directory.path())
                .args([
                    "--only",
                    "Lint/LiteralInInterpolation",
                    "-f",
                    "json",
                    "example.rb",
                ])
                .assert()
                .code(1)
                .get_output()
                .stdout
                .clone();
            let output = command(directory.path())
                .args([
                    "--only",
                    "Lint/LiteralInInterpolation",
                    mode,
                    "--fail-level",
                    "fatal",
                    "-f",
                    "json",
                    "example.rb",
                    "other.rb",
                ])
                .assert()
                .code(2)
                .get_output()
                .clone();
            let refusal = format!(
                "refusing to rewrite example.rb: the correction cannot be written back as {}",
                case["encoding"].as_str().unwrap()
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains(&refusal));
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                source.as_bytes()
            );
            assert_eq!(
                fs::read(directory.path().join("other.rb")).unwrap(),
                native_lf_disk_correction(b"value = \"1\"\n")
            );
            let before: serde_json::Value = serde_json::from_slice(&lint).unwrap();
            let after: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(before["files"][0], after["files"][0]);
            assert_eq!(after["summary"]["target_file_count"], 2);
            assert_eq!(after["summary"]["inspected_file_count"], 2);
            command(directory.path())
                .args(["--only", "Lint/LiteralInInterpolation", mode, "example.rb"])
                .assert()
                .code(2);
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                source.as_bytes()
            );
        }
    }
}

/// エディタが受け取る stdin の戻り値も、有効な原本のバイト列を保持する。
#[test]
fn an_unencodable_stdin_correction_returns_the_original_bytes() {
    let source = "# coding: US-ASCII\nvalue = \"#{\"\\u00e9\"}\"\n";
    let directory = project_with_ruby(&[], "2.7");
    for mode in ["-a", "-A"] {
        let output = command(directory.path())
            .args([
                "--only",
                "Lint/LiteralInInterpolation",
                mode,
                "--stdin",
                "example.rb",
                "--fail-level",
                "fatal",
                "-f",
                "simple",
            ])
            .write_stdin(source)
            .assert()
            .code(2)
            .get_output()
            .clone();
        assert!(
            output
                .stdout
                .ends_with(format!("{}{source}", stdin_separator()).as_bytes())
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("cannot be written back as US-ASCII")
        );
        assert!(!directory.path().join("example.rb").exists());
    }
    // 補正不要な binary バッファを UTF-8 に変換して返さない。
    let binary = b"# coding: ASCII-8BIT\n# \xED\x40\nvalue = 1\n";
    let output = command(directory.path())
        .args([
            "--only",
            "Lint/LiteralInInterpolation",
            "-a",
            "--stdin",
            "example.rb",
            "-f",
            "simple",
        ])
        .write_stdin(binary.as_slice())
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    assert!(output.ends_with(&native_textmode_expected(binary)));
}

/// rollback 後も原本のディレクティブ診断を残す。--only ではこの集約 cop に届かない。
#[test]
fn a_syntax_rollback_keeps_the_original_directive_diagnostics() {
    let source = "# rubocop:disable Style/RedundantReturn\nouter do |transaction|\n  consume transaction\n  inner do |transaction|\n    use transaction\n  end\nend\n# rubocop:enable Style/RedundantReturn\n";
    for stdin in [false, true] {
        for mode in ["-a", "-A"] {
            let directory = project_with_ruby(
                &[
                    ("example.rb", source),
                    (
                        ".rubocop.yml",
                        "AllCops:\n  NewCops: disable\nStyle/ItBlockParameter:\n  Enabled: true\n  EnforcedStyle: always\n",
                    ),
                ],
                "3.4",
            );
            let lint = command(directory.path())
                .args(["--cache", "false", "-f", "json", "example.rb"])
                .assert()
                .code(1)
                .get_output()
                .stdout
                .clone();
            let before: serde_json::Value = serde_json::from_slice(&lint).unwrap();
            assert!(
                before["files"][0]["offenses"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|offense| offense["cop_name"] == "Lint/RedundantCopDisableDirective")
            );
            let mut run = command(directory.path());
            run.args([
                "--cache",
                "false",
                mode,
                "--fail-level",
                "fatal",
                "-f",
                "json",
            ]);
            if stdin {
                run.args(["--stdin", "example.rb"]).write_stdin(source);
            } else {
                run.arg("example.rb");
            }
            let output = run.assert().code(2).get_output().stdout.clone();
            let after: serde_json::Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(before["files"], after["files"]);
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                source.as_bytes()
            );
        }
    }
}

/// 宣言が無効な位置へ動くときは、本家が実際に出す有効な UTF-8 バイトへ合わせる。
#[test]
fn a_displaced_encoding_declaration_uses_the_upstream_utf8_output() {
    let records: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/encoding_header_corrections.json")).unwrap();
    for case in records.as_array().unwrap() {
        let decode = |key: &str| {
            let hex = case[key].as_str().unwrap();
            (0..hex.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
                .collect::<Vec<_>>()
        };
        let source = decode("source_hex");
        let expected = decode("upstream_hex");
        for stdin in [false, true] {
            let directory = project_with_ruby(&[], "2.7");
            fs::write(directory.path().join("example.rb"), &source).unwrap();
            let mut run = command(directory.path());
            run.args([
                "--force-default-config",
                "--cache",
                "false",
                "--only",
                case["cops"].as_str().unwrap(),
                "-A",
                "-f",
                "simple",
            ]);
            if stdin {
                run.args(["--stdin", "example.rb"])
                    .write_stdin(source.clone());
            } else {
                run.arg("example.rb");
            }
            let output = run.assert().code(0).get_output().clone();
            if stdin {
                assert!(
                    output
                        .stdout
                        .ends_with(&native_textmode_expected(&expected))
                );
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    source
                );
            } else {
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    native_lf_disk_correction(&expected)
                );
                command(directory.path())
                    .args([
                        "--force-default-config",
                        "--cache",
                        "false",
                        "--only",
                        case["cops"].as_str().unwrap(),
                        "-A",
                        "example.rb",
                    ])
                    .assert()
                    .code(0);
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    native_lf_disk_correction(&expected)
                );
            }
            assert!(!String::from_utf8_lossy(&output.stderr).contains("cannot be written back"));
        }
    }
}

/// 別の cop による非 ASCII の削除が、補間補正で追加された文字を相殺してはならない。
#[test]
fn unrelated_character_deletions_do_not_hide_an_unsafe_encoding_write() {
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let cases = manifest["encoding_edit_provenance_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(cases.len(), 8);
    for case in cases {
        let encoding = case["encoding"].as_str().unwrap();
        let hex = case["source_hex"].as_str().unwrap();
        let source = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let mode = if case["mode"].as_str().unwrap() == "safe" {
            "-a"
        } else {
            "-A"
        };
        for stdin in [false, true] {
            let directory = project_with_ruby(&[], "2.7");
            fs::write(directory.path().join("example.rb"), &source).unwrap();
            fs::write(directory.path().join("other.rb"), "value = \"#{1}\"\n").unwrap();
            let mut run = command(directory.path());
            run.args([
                "--force-default-config",
                "--cache",
                "false",
                "--only",
                case["cops"].as_str().unwrap(),
                mode,
                "--fail-level",
                "fatal",
                "-f",
                if stdin { "simple" } else { "json" },
            ]);
            if stdin {
                run.args(["--stdin", "example.rb"])
                    .write_stdin(source.clone());
            } else {
                run.args(["example.rb", "other.rb"]);
            }
            let output = run.assert().code(2).get_output().clone();
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                source
            );
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains(&format!("cannot be written back as {encoding}"))
            );
            if stdin {
                assert!(output.stdout.ends_with(&source));
            } else {
                let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["summary"]["inspected_file_count"], 2);
                assert_eq!(
                    fs::read(directory.path().join("other.rb")).unwrap(),
                    native_lf_disk_correction(b"value = \"1\"\n")
                );
            }
        }
    }
}

/// 宣言が有効なままなら、ASCII の引用符だけを直し、既存の非 ASCII バイトを保つ。
#[test]
fn an_active_legacy_declaration_preserves_the_original_character_bytes() {
    let cases: &[(&str, &[u8])] = &[
        ("cp932", b"\x93\xfa\x96\x7b"),
        ("Shift_JIS", b"\x81\x60"),
        ("ISO-8859-1", b"\x80"),
        ("EUC-JP", b"\xa1\xc1"),
        ("ASCII-8BIT", b"\xe9"),
    ];
    for (encoding, characters) in cases {
        let mut source = format!("# coding: {encoding}\nx = \"").into_bytes();
        source.extend_from_slice(characters);
        source.extend_from_slice(b"\"\n");
        let mut expected = format!("# coding: {encoding}\nx = '").into_bytes();
        expected.extend_from_slice(characters);
        expected.extend_from_slice(b"'\n");
        for stdin in [false, true] {
            for mode in ["-a", "-A"] {
                let directory = project_with_ruby(&[], "2.7");
                fs::write(directory.path().join("example.rb"), &source).unwrap();
                let mut run = command(directory.path());
                run.args([
                    "--force-default-config",
                    "--cache",
                    "false",
                    "--only",
                    "Style/StringLiterals",
                    mode,
                    "-f",
                    "simple",
                ]);
                if stdin {
                    run.args(["--stdin", "example.rb"])
                        .write_stdin(source.clone());
                } else {
                    run.arg("example.rb");
                }
                let output = run.assert().code(0).get_output().stdout.clone();
                if stdin {
                    assert!(output.ends_with(&native_textmode_expected(&expected)));
                    assert_eq!(
                        fs::read(directory.path().join("example.rb")).unwrap(),
                        source
                    );
                } else {
                    assert_eq!(
                        fs::read(directory.path().join("example.rb")).unwrap(),
                        native_lf_disk_correction(&expected)
                    );
                    command(directory.path())
                        .args([
                            "--force-default-config",
                            "--cache",
                            "false",
                            "--only",
                            "Style/StringLiterals",
                            mode,
                            "example.rb",
                        ])
                        .assert()
                        .code(0);
                    assert_eq!(
                        fs::read(directory.path().join("example.rb")).unwrap(),
                        native_lf_disk_correction(&expected)
                    );
                }
            }
        }
    }
}

/// 次のパスが条件式を広く置換しても、捨てる分岐で追加文字の由来を消さない。
#[test]
fn later_conditional_corrections_preserve_character_provenance() {
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let cases = manifest["encoding_multipass_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(cases.len(), 6);
    for case in cases {
        let condition = case["condition"].as_str().unwrap();
        let mode = if case["mode"].as_str().unwrap() == "safe" {
            "-a"
        } else {
            "-A"
        };
        let decode_hex = |hex: &str| {
            (0..hex.len())
                .step_by(2)
                .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
                .collect::<Vec<_>>()
        };
        for crlf in [false, true] {
            let mut source = decode_hex(case["source_hex"].as_str().unwrap());
            let mut expected = decode_hex(case["sonicop_bytes_hex"].as_str().unwrap());
            if crlf {
                let to_crlf = |bytes: Vec<u8>| {
                    bytes
                        .split(|byte| *byte == b'\n')
                        .enumerate()
                        .flat_map(|(index, line)| {
                            let mut part = Vec::new();
                            if index > 0 {
                                part.extend_from_slice(b"\r\n");
                            }
                            part.extend_from_slice(line);
                            part
                        })
                        .collect::<Vec<_>>()
                };
                source = to_crlf(source);
                expected = to_crlf(expected);
            }
            for stdin in [false, true] {
                let directory = project_with_ruby(&[], "2.7");
                fs::write(directory.path().join("example.rb"), &source).unwrap();
                let mut run = command(directory.path());
                run.args([
                    "--force-default-config",
                    "--cache",
                    "false",
                    "--only",
                    case["cops"].as_str().unwrap(),
                    mode,
                    "--fail-level",
                    "fatal",
                    "-f",
                    "simple",
                ]);
                if stdin {
                    run.args(["--stdin", "example.rb"])
                        .write_stdin(source.clone());
                } else {
                    run.arg("example.rb");
                }
                let failed = condition == "false";
                let output = run
                    .assert()
                    .code(case["sonicop_exit"].as_i64().unwrap() as i32)
                    .get_output()
                    .clone();
                // 既存の安全な文字 byte を符号化付き File.write で独立検証した。
                let disk_expected = if !stdin && !crlf && !failed {
                    native_lf_disk_correction(&expected)
                } else {
                    expected.clone()
                };
                let stdin_expected = if stdin && !crlf && !failed {
                    native_textmode_expected(&expected)
                } else {
                    expected.clone()
                };
                let wanted = if stdin {
                    &stdin_expected
                } else {
                    &disk_expected
                };
                if stdin {
                    assert!(output.stdout.ends_with(wanted), "{condition} {mode} {crlf}");
                    assert_eq!(
                        fs::read(directory.path().join("example.rb")).unwrap(),
                        source
                    );
                } else {
                    assert_eq!(
                        fs::read(directory.path().join("example.rb")).unwrap(),
                        *wanted
                    );
                }
                assert_eq!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains("cannot be written back as Shift_JIS"),
                    failed
                );
            }
        }
    }
}

#[test]
fn show_cops_prints_the_effective_configuration_and_correction_support() {
    let directory = project(&[(".rubocop.yml", "AllCops:\n  NewCops: disable\n")]);
    // RuboCop 1.89.0 の非 TTY CLI から採取した通常の表示をバイト単位で照合する。
    let expected = include_bytes!("fixtures/show_cops_operators.yml");
    command(directory.path())
        .args(["--show-cops", "Layout/SpaceAroundOperators"])
        .assert()
        .success()
        .stdout(native_textmode_expected(expected))
        .stderr("");

    fs::write(directory.path().join(".rubocop.yml"),
        "AllCops:\n  NewCops: disable\nLayout:\n  Enabled: false\nLayout/SpaceAroundOperators:\n  AllowForAlignment: false\n  SupportedStylesForExponentOperator: [no_space]\n").unwrap();
    let expected = String::from_utf8(expected.to_vec())
        .unwrap()
        .replace("Enabled: true", "Enabled: false")
        .replace("AllowForAlignment: true", "AllowForAlignment: false")
        .replacen("  - space\n  - no_space", "  - no_space", 1);
    command(directory.path())
        .args(["--show-cops", "Layout/SpaceAroundOperators"])
        .assert()
        .success()
        .stdout(native_textmode_expected(expected.as_bytes()))
        .stderr("");
}

#[test]
fn show_cops_keeps_the_upstream_scalar_types() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/show_cops_typed_values.json")).unwrap();
    for case in cases {
        let config = format!(
            "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\nStyle/For:\n  Description: {}\n",
            case["input"].as_str().unwrap()
        );
        let directory = project(&[]);
        // 設定ヘルパーの再直列化で !!binary の型を落とさず、本家と同じ原文を渡す。
        fs::write(directory.path().join(".rubocop.yml"), config).unwrap();
        let output = command(directory.path())
            .arg("--show-cops=Style/For")
            .assert()
            .success()
            .get_output()
            .clone();
        assert!(output.stderr.is_empty());
        assert_eq!(
            output.stdout,
            native_textmode_expected(case["stdout"].as_str().unwrap().as_bytes()),
            "{}",
            case["input"]
        );
    }
}

#[test]
fn show_cops_matches_the_complete_upstream_catalog() {
    let directory = project(&[]);
    let output = command(directory.path())
        .args(["--force-default-config", "--show-cops"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(output.stderr.is_empty());
    let cwd = if cfg!(windows) {
        directory.path().to_path_buf()
    } else {
        fs::canonicalize(directory.path()).unwrap()
    };
    let header = format!(
        "# Available cops (609) + config for {}: \n",
        cwd.to_string_lossy().replace('\\', "/")
    );
    let mut expected = header.into_bytes();
    let fixture = include_str!("fixtures/show_cops_default.yml");
    // Psych は / で始まる POSIX パスを引用し、C:/ で始まるパスは plain にする。
    let fixture = if cfg!(windows) {
        fixture
            .lines()
            .map(|line| {
                if line.contains("{{cwd}}") {
                    format!("{}\n", line.replacen('"', "", 1).trim_end_matches('"'))
                } else {
                    format!("{line}\n")
                }
            })
            .collect::<String>()
    } else {
        fixture.to_owned()
    };
    expected.extend_from_slice(
        fixture
            .replace("{{cwd}}", &cwd.to_string_lossy().replace('\\', "/"))
            .as_bytes(),
    );
    let expected = native_textmode_expected(&expected);
    // 全文を失敗ログへ展開せず、最初の差の位置を示す。
    assert_eq!(
        output
            .stdout
            .iter()
            .zip(&expected)
            .position(|(a, b)| a != b),
        None
    );
    assert_eq!(output.stdout.len(), expected.len());
}

#[test]
fn show_cops_preserves_the_upstream_enabled_catalog() {
    let directory = project(&[]);
    let output = command(directory.path())
        .args(["--force-default-config", "--show-cops"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let actual: serde_yaml::Value = serde_yaml::from_slice(&output).unwrap();
    let expected: serde_yaml::Value =
        serde_yaml::from_str(include_str!("../config/default.yml")).unwrap();
    let catalog = |value: &serde_yaml::Value| {
        value
            .as_mapping()
            .unwrap()
            .iter()
            .filter_map(|(name, config)| {
                let name = name.as_str()?;
                name.contains('/')
                    .then(|| (name.to_owned(), config["Enabled"].clone()))
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    // 本家 1.89.0 の CLI で確認した名前集合と true 394 / pending 159 / false 56 を保つ。
    let actual = catalog(&actual);
    let expected = catalog(&expected);
    assert!(
        actual.keys().eq(expected.keys()),
        "本家の cop 名集合と異なる"
    );
    for (name, enabled) in expected {
        assert_eq!(actual[&name], enabled, "{name}");
    }
}

#[test]
fn show_cops_separates_pending_settings_from_runtime_selection() {
    for (config, expected) in [
        ("{}\n", "pending"),
        ("AllCops:\n  NewCops: enable\n", "pending"),
        ("AllCops:\n  NewCops: disable\n", "pending"),
        ("AllCops:\n  EnabledByDefault: true\n", "true"),
        ("AllCops:\n  DisabledByDefault: true\n", "false"),
        ("Style:\n  Enabled: true\n", "pending"),
        ("Style:\n  Enabled: false\n", "false"),
        ("Style/MapToSet:\n  Enabled: true\n", "true"),
        ("Style/MapToSet:\n  Enabled: false\n", "false"),
        (
            "AllCops:\n  DisabledByDefault: true\nStyle/MapToSet:\n  Enabled: pending\n",
            "pending",
        ),
        (
            "AllCops:\n  EnabledByDefault: true\nStyle/MapToSet:\n  Enabled: pending\n",
            "pending",
        ),
        (
            "Style:\n  Enabled: false\nStyle/MapToSet:\n  Enabled: true\n",
            "true",
        ),
        (
            "Style:\n  Enabled: false\nStyle/MapToSet:\n  Enabled: pending\n",
            "false",
        ),
    ] {
        let directory = project(&[(".rubocop.yml", config)]);
        let output = command(directory.path())
            .args(["--show-cops=Style/MapToSet"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let listing: serde_yaml::Value = serde_yaml::from_slice(&output).unwrap();
        let expected: serde_yaml::Value = serde_yaml::from_str(expected).unwrap();
        // 期待値は NewCops と部門・個別指定を変えた本家 CLI の実出力。
        assert_eq!(listing["Style/MapToSet"]["Enabled"], expected, "{config}");
    }
}

#[test]
fn double_directive_locations_exclude_cr_and_corrections_preserve_line_endings() {
    for ending in ["\n", "\r\n"] {
        for correction in [None, Some("-a"), Some("-A")] {
            let original = format!(
                "text = '日本語' # rubocop:disable Style/For # rubocop:disable Metrics/AbcSize{ending}"
            );
            let directory = project(&[("example.rb", &original)]);
            let mut run = command(directory.path());
            run.args([
                "--force-default-config",
                "--cache",
                "false",
                "--only",
                "Style/DoubleCopDisableDirective",
                "--format",
                "json",
            ]);
            if let Some(flag) = correction {
                run.arg(flag);
            }
            let output = run
                .arg("example.rb")
                .assert()
                .code(if correction.is_some() { 0 } else { 1 })
                .get_output()
                .stdout
                .clone();
            let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
            // 全項目は本家 1.89.0 の LF / CRLF と検査・-a・-A の実出力から取得した。
            assert_eq!(
                report["files"][0]["offenses"],
                serde_json::json!([{
                    "severity": "convention",
                    "message": "More than one disable comment on one line.",
                    "cop_name": "Style/DoubleCopDisableDirective",
                    "corrected": correction.is_some(),
                    "correctable": true,
                    "location": {
                        "start_line": 1, "start_column": 14,
                        "last_line": 1, "last_column": 74,
                        "length": 61, "line": 1, "column": 14
                    }
                }])
            );
            let expected = if correction.is_some() {
                format!("text = '日本語' # rubocop:disable Style/For, Metrics/AbcSize{ending}")
            } else {
                original
            };
            // CRLF の保持は既存の意図的差分。二重 CR も、原本の LF 化も許さない。
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                if correction.is_some() && ending == "\n" {
                    native_lf_disk_correction(expected.as_bytes())
                } else {
                    expected.as_bytes().to_vec()
                }
            );
        }
    }
}

#[test]
fn block_comment_directive_ranges_reach_the_newline_after_end() {
    // (cop, メッセージ, 原本, 長さ, 補正後)。`{n}` は改行。CRLF の改行も 1 文字と数える。
    let cases = [
        (
            "Style/DoubleCopDisableDirective",
            "More than one disable comment on one line.",
            "=begin{n}# rubocop:disable Style/For # rubocop:disable Metrics/AbcSize{n}=end{n}x = 1{n}",
            74,
            "=begin{n}# rubocop:disable Style/For, Metrics/AbcSize{n}=end{n}x = 1{n}",
        ),
        (
            "Style/DisableCopsWithinSourceCodeDirective",
            "RuboCop disable/enable directives are not permitted.",
            "=begin{n}# rubocop:disable Layout/LineLength{n}=end{n}x = 1{n}",
            48,
            "x = 1{n}",
        ),
    ];
    for (cop, message, source, length, corrected) in cases {
        for ending in ["\n", "\r\n"] {
            for correction in [None, Some("-a"), Some("-A")] {
                let original = source.replace("{n}", ending);
                let directory = project(&[("example.rb", &original)]);
                let mut run = command(directory.path());
                run.args([
                    "--force-default-config",
                    "--cache",
                    "false",
                    "--only",
                    cop,
                    "--format",
                    "json",
                ]);
                if let Some(flag) = correction {
                    run.arg(flag);
                }
                let output = run
                    .arg("example.rb")
                    .assert()
                    .code(if correction.is_some() { 0 } else { 1 })
                    .get_output()
                    .stdout
                    .clone();
                let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
                // 全項目は本家 1.89.0 の LF / CRLF と検査・-a・-A の実出力から取得した。
                assert_eq!(
                    report["files"][0]["offenses"],
                    serde_json::json!([{
                        "severity": "convention",
                        "message": message,
                        "cop_name": cop,
                        "corrected": correction.is_some(),
                        "correctable": true,
                        "location": {
                            "start_line": 1, "start_column": 1,
                            "last_line": 4, "last_column": 1,
                            "length": length, "line": 1, "column": 1
                        }
                    }]),
                    "{cop} {ending:?} {correction:?}"
                );
                let expected = match correction {
                    Some(_) => corrected.replace("{n}", ending),
                    None => original,
                };
                // 本家は CRLF を LF で書き戻すが、CRLF を保つのは既存の意図的差分。
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    if correction.is_some() && ending == "\n" {
                        native_lf_disk_correction(expected.as_bytes())
                    } else {
                        expected.as_bytes().to_vec()
                    },
                    "{cop} {ending:?} {correction:?}"
                );
            }
        }
    }
}

#[test]
fn block_comment_upstream_internal_errors_are_registered_cli_differences() {
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let cases = manifest["block_comment_error_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        let cop = case["cop"].as_str().unwrap();
        let config = format!(
            "AllCops:\n  TargetRubyVersion: {}\nStyle/DisableCopsWithinSourceCodeDirective:\n  Enabled: true\n",
            case["target_ruby"].as_str().unwrap()
        );
        // 本家 1.89.0 の記録は内部例外の終了 1。架空の offense は追加しない。
        assert_eq!(case["upstream"]["exit"].as_i64(), Some(1));
        assert_eq!(case["sonicop"]["exit"].as_i64(), Some(0));
        assert_eq!(case["upstream"]["offenses"], case["sonicop"]["offenses"]);
        for ending in case["endings"].as_sequence().unwrap() {
            let ending = match ending.as_str().unwrap() {
                "lf" => "\n",
                "crlf" => "\r\n",
                other => panic!("未登録の改行: {other}"),
            };
            let original = case["source"].as_str().unwrap().replace('\n', ending);
            let upstream_source = case["upstream"]["source"]
                .as_str()
                .unwrap()
                .replace('\n', ending);
            let expected_source = case["sonicop"]["source"]
                .as_str()
                .unwrap()
                .replace('\n', ending);
            assert_eq!(upstream_source, original);
            assert_eq!(expected_source, original);
            for mode in case["modes"].as_sequence().unwrap() {
                // project() は対象版を 2.7 に戻すため、記録のパーサを明示して維持する。
                let directory = project_with_ruby(
                    &[(".rubocop.yml", &config), ("example.rb", &original)],
                    case["target_ruby"].as_str().unwrap(),
                );
                let mut run = command(directory.path());
                run.args(["--cache", "false", "--only", cop, "--format", "json"]);
                match mode.as_str().unwrap() {
                    "inspect" => {}
                    "safe" => {
                        run.arg("-a");
                    }
                    "unsafe" => {
                        run.arg("-A");
                    }
                    other => panic!("未登録の補正モード: {other}"),
                }
                let output = run
                    .arg("example.rb")
                    .assert()
                    .code(case["sonicop"]["exit"].as_i64().unwrap() as i32)
                    .get_output()
                    .stdout
                    .clone();
                let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
                assert_eq!(report["files"].as_array().unwrap().len(), 1);
                assert_eq!(
                    report["files"][0]["offenses"],
                    serde_json::to_value(&case["sonicop"]["offenses"]).unwrap(),
                    "{cop} {ending:?} {mode:?}"
                );
                assert_eq!(report["summary"]["offense_count"], 0);
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    expected_source.as_bytes(),
                    "{cop} {ending:?} {mode:?}"
                );
            }
        }
    }
}

#[test]
fn show_cops_descriptions_remain_loadable_yaml() {
    let directory = project(&[]);
    let output = command(directory.path())
        .args(["--force-default-config", "--show-cops"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listing: serde_yaml::Value = serde_yaml::from_slice(&output).unwrap();
    assert_eq!(listing.as_mapping().unwrap().len(), 609);
    // RuboCop 1.89.0 の --show-cops が返す文字列を保持する。
    assert_eq!(
        listing["Lint/BigDecimalNew"]["Description"].as_str(),
        Some("`BigDecimal.new()` is deprecated. Use `BigDecimal()` instead.")
    );
    for description in [
        "plain text",
        "no",
        "yes",
        "on",
        "value: with # punctuation",
        "quote ' and \"",
        "control \0 byte",
        "line\u{85}break",
        "line\u{2028}break",
        "control\u{7f}byte",
        "control\u{9f}byte",
        "text\u{fffe}byte",
        "文字 😀",
    ] {
        let config = format!(
            "Lint/BigDecimalNew:\n  Description: !!str {}",
            serde_yaml::to_string(description).unwrap()
        );
        std::fs::write(directory.path().join(".rubocop.yml"), config).unwrap();
        let output = command(directory.path())
            .args(["--show-cops=Lint/BigDecimalNew"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let listing: serde_yaml::Value = serde_yaml::from_slice(&output).unwrap();
        assert_eq!(
            listing["Lint/BigDecimalNew"]["Description"].as_str(),
            Some(description)
        );
        // 再読込にも Ruby の YAML 1.1 と同じスカラ解釈を使わせる。
        std::fs::write(directory.path().join(".rubocop.yml"), &output).unwrap();
        let repeated = command(directory.path())
            .args(["--show-cops=Lint/BigDecimalNew"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let repeated: serde_yaml::Value = serde_yaml::from_slice(&repeated).unwrap();
        assert_eq!(
            repeated["Lint/BigDecimalNew"]["Description"].as_str(),
            Some(description)
        );
    }
}

#[test]
fn parser_updates_do_not_add_a_star_error_to_valid_percent_literals() {
    let source = include_str!("fixtures/percent_literal_recovery.rb");
    let directory = project(&[("example.rb", source)]);
    let output = command(directory.path())
        .args([
            "--force-default-config",
            "--cache",
            "false",
            "--only",
            "Lint/Syntax",
            "-f",
            "json",
            "example.rb",
        ])
        .output()
        .unwrap();
    assert!(matches!(output.status.code(), Some(0 | 1)));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["summary"]["inspected_file_count"], 1);
    assert_eq!(report["summary"]["target_file_count"], 1);
    // 本家は構文診断0件。既知の EOF 回復差を正解へ固定せず、追加された誤診断を監視する。
    assert!(
        offenses(&output.stdout)
            .iter()
            .all(|offense| { !offense.message.starts_with("unexpected token tSTAR\n") })
    );
    assert_eq!(
        fs::read(directory.path().join("example.rb")).unwrap(),
        source.as_bytes()
    );
}

#[test]
fn numeric_predicate_keeps_the_original_when_upstream_writes_invalid_ruby() {
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let cases = manifest["numeric_predicate_syntax_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        let before = case["source"].as_str().unwrap();
        let target = case["target_ruby"].as_str().unwrap();
        // 本家の実出力には新しい fatal があり、候補は元の診断だけを返す。
        assert_eq!(case["upstream"]["exit"].as_i64(), Some(1));
        assert_ne!(case["upstream"]["source"].as_str().unwrap(), before);
        assert!(
            case["upstream"]["offenses"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|offense| offense["cop_name"].as_str() == Some("Lint/Syntax"))
        );
        assert_eq!(case["sonicop"]["source"].as_str().unwrap(), before);
        for ending in ["\n", "\r\n"] {
            for correction in [None, Some("-a"), Some("-A")] {
                let original = before.replace('\n', ending);
                let directory = project_with_ruby(&[("example.rb", &original)], target);
                let mut run = command(directory.path());
                run.args([
                    "--cache",
                    "false",
                    "--only",
                    case["cop"].as_str().unwrap(),
                    "--format",
                    "json",
                ]);
                if let Some(flag) = correction {
                    run.arg(flag);
                }
                let unsafe_correction = correction == Some("-A");
                let expected_exit = if unsafe_correction {
                    case["sonicop"]["exit"].as_i64().unwrap() as i32
                } else {
                    1
                };
                let output = run
                    .arg("example.rb")
                    .assert()
                    .code(expected_exit)
                    .get_output()
                    .clone();
                let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["files"].as_array().unwrap().len(), 1);
                assert_eq!(report["summary"]["inspected_file_count"], 1);
                assert_eq!(report["summary"]["offense_count"], 1);
                assert_eq!(
                    report["files"][0]["offenses"],
                    serde_json::to_value(&case["sonicop"]["offenses"]).unwrap(),
                    "{} {ending:?} {correction:?}",
                    case["id"].as_str().unwrap()
                );
                // 拒否時も検査と安全補正の陰性対照も、実際の原本バイトを守る。
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    original.as_bytes()
                );
                let expected_stderr = if unsafe_correction {
                    case["sonicop"]["stderr"].as_str().unwrap()
                } else {
                    ""
                };
                assert_eq!(output.stderr, expected_stderr.as_bytes());
            }
        }
    }
}

/// whitequark が入れ子 heredoc を重複して読む形でも、物理 Ruby の変数束縛を保つ。
/// 本家 Prism の実出力を物理ソースの期待値とし、whitequark との診断・補正の差は
/// conformance のマニフェストで別に監視する。Ruby のない環境でも全フィールドとバイトを検査する。
#[test]
fn nested_heredocs_preserve_physical_reads_across_parser_settings() {
    let fixture = support::nested_heredocs::fixture();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 7);
    assert_eq!(fixture["profiles"].as_array().unwrap().len(), 9);
    let mut checked = 0;
    for profile in fixture["profiles"].as_array().unwrap() {
        let target = profile["target"].as_str().unwrap();
        let engine = profile["engine"].as_str().unwrap();
        for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let unsupported = profile["effective"].is_null();
            if unsupported && index != 0 {
                continue;
            }
            let id = case["id"].as_str().unwrap();
            for (ending, newline) in [("lf", "\n"), ("crlf", "\r\n")] {
                let source = case["source"].as_str().unwrap().replace('\n', newline);
                let directory = project_with_ruby(&[("example.rb", &source)], target);
                fs::write(
                    directory.path().join(".rubocop.yml"),
                    profile["yaml"].as_str().unwrap(),
                )
                .unwrap();
                for mode in ["inspect", "-a", "-A"] {
                    let label = format!("{id}/{target}/{engine}/{ending}/{mode}");
                    fs::write(directory.path().join("example.rb"), source.as_bytes()).unwrap();
                    let mut invocation = command(directory.path());
                    invocation.env("PATH", "").args([
                        "--cache",
                        "false",
                        "--only",
                        fixture["only"].as_str().unwrap(),
                        "--format",
                        "json",
                        "example.rb",
                    ]);
                    if mode != "inspect" {
                        invocation.arg(mode);
                    }
                    let expected = if unsupported {
                        let key = fixture["unsupported"][ending][mode].as_str().unwrap();
                        &fixture["records"][key]
                    } else {
                        support::nested_heredocs::record(&fixture, case, "prism", ending, mode)
                    };
                    let output = invocation
                        .assert()
                        .code(expected["exit"].as_i64().unwrap() as i32)
                        .get_output()
                        .clone();
                    if unsupported {
                        assert!(
                            String::from_utf8_lossy(&output.stderr)
                                .contains(expected["error"].as_str().unwrap()),
                            "{label}"
                        );
                        assert!(output.stdout.is_empty(), "{label}");
                    } else {
                        let found: serde_json::Value =
                            serde_json::from_slice(&output.stdout).unwrap();
                        assert_eq!(found["summary"]["target_file_count"], 1, "{label}");
                        assert_eq!(found["summary"]["inspected_file_count"], 1, "{label}");
                        assert_eq!(
                            found["files"][0]["offenses"], expected["offenses"],
                            "{label}"
                        );
                        // 記録を入れ替えて危険な本家補正へ寄せる変更も拒む。実行可能な原本を保持する。
                        assert_eq!(
                            expected["after"].as_str().unwrap().as_bytes(),
                            source.as_bytes(),
                            "{label}"
                        );
                        assert_eq!(expected["execution"]["syntax_valid"], true, "{label}");
                        if case["run"] == true {
                            assert_eq!(expected["execution"]["exit"], 0, "{label}");
                        }
                    }
                    assert_eq!(
                        fs::read(directory.path().join("example.rb")).unwrap(),
                        source.as_bytes(),
                        "{label}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 342);
    let used = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "used_assignment")
        .unwrap();
    for ending in ["lf", "crlf"] {
        for mode in ["-a", "-A"] {
            let reference =
                support::nested_heredocs::record(&fixture, used, "whitequark", ending, mode);
            assert_eq!(reference["execution"]["syntax_valid"], true);
            assert_eq!(reference["execution"]["exit"], 1);
            assert_eq!(reference["execution"]["error_class"], "NameError");
        }
    }
}

/// Metrics の違反が残る終了 1 で、UselessAssignment 単独の診断・補正の差を隠さない。
#[test]
fn a_nested_heredoc_assignment_stays_bound_after_both_correction_modes() {
    let fixture = support::nested_heredocs::fixture();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "used_assignment")
        .unwrap();
    let mut checked = 0;
    for profile in fixture["profiles"].as_array().unwrap() {
        if profile["effective"].is_null() {
            continue;
        }
        for (ending, newline) in [("lf", "\n"), ("crlf", "\r\n")] {
            let source = case["source"].as_str().unwrap().replace('\n', newline);
            let directory = project_with_ruby(
                &[("example.rb", &source)],
                profile["target"].as_str().unwrap(),
            );
            fs::write(
                directory.path().join(".rubocop.yml"),
                profile["yaml"].as_str().unwrap(),
            )
            .unwrap();
            for mode in ["inspect", "-a", "-A"] {
                let label = format!(
                    "{}/{}/{ending}/{mode}",
                    profile["target"], profile["engine"]
                );
                fs::write(directory.path().join("example.rb"), source.as_bytes()).unwrap();
                let mut invocation = command(directory.path());
                invocation.env("PATH", "").args([
                    "--cache",
                    "false",
                    "--only",
                    "Lint/UselessAssignment",
                    "--format",
                    "json",
                    "example.rb",
                ]);
                if mode != "inspect" {
                    invocation.arg(mode);
                }
                let output = invocation.assert().success().get_output().stdout.clone();
                let found: serde_json::Value = serde_json::from_slice(&output).unwrap();
                let physical = support::nested_heredocs::variable_record(
                    &fixture, case, "prism", ending, mode,
                );
                assert_eq!(physical["exit"], 0);
                assert_eq!(found["summary"]["target_file_count"], 1, "{label}");
                assert_eq!(found["summary"]["inspected_file_count"], 1, "{label}");
                assert_eq!(
                    found["files"][0]["offenses"], physical["offenses"],
                    "{label}"
                );
                assert!(physical["offenses"].as_array().unwrap().is_empty());
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    source.as_bytes(),
                    "{label}"
                );
                let whitequark = support::nested_heredocs::variable_record(
                    &fixture,
                    case,
                    "whitequark",
                    ending,
                    mode,
                );
                assert_eq!(whitequark["exit"], if mode == "inspect" { 1 } else { 0 });
                assert_eq!(whitequark["offenses"].as_array().unwrap().len(), 1);
                assert_eq!(whitequark["offenses"][0]["corrected"], mode != "inspect");
                assert_eq!(
                    whitequark["execution"]["exit"],
                    if mode == "inspect" { 0 } else { 1 }
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 48);
}

/// ブロック付き send の範囲だけを診断・置換する。本家 1.89.0 の実出力を全項目で照合し、
/// `-a` は変更せず、`-A` はブロックと原本の LF/CRLF を残す。
#[test]
fn numeric_predicate_preserves_blocks_on_disk() {
    for (source, send, replacement, corrected, length) in [
        (
            "x.>(0) { }\n",
            "x.>(0)",
            "x.positive?",
            "x.positive? { }\n",
            6,
        ),
        (
            "0.<(x) { }\n",
            "0.<(x)",
            "x.positive?",
            "x.positive? { }\n",
            6,
        ),
        ("x.==(0) { }\n", "x.==(0)", "x.zero?", "x.zero? { }\n", 7),
        (
            "日本語.>(0) { 'é' }\n",
            "日本語.>(0)",
            "日本語.positive?",
            "日本語.positive? { 'é' }\n",
            8,
        ),
        (
            "x.>(0) \\\n { }\n",
            "x.>(0)",
            "x.positive?",
            "x.positive? \\\n { }\n",
            6,
        ),
        (
            "x.>(0)\\\n{ }\n",
            "x.>(0)",
            "x.positive?",
            "x.positive?\\\n{ }\n",
            6,
        ),
        (
            "x.>(0) \\\n\\\n { }\n",
            "x.>(0)",
            "x.positive?",
            "x.positive? \\\n\\\n { }\n",
            6,
        ),
        (
            "x.>(0)\u{b}{ }\n",
            "x.>(0)",
            "x.positive?",
            "x.positive?\u{b}{ }\n",
            6,
        ),
        (
            "x.>(0)\u{c}{ }\n",
            "x.>(0)",
            "x.positive?",
            "x.positive?\u{c}{ }\n",
            6,
        ),
        (
            "x.>(0)\t{ }\n",
            "x.>(0)",
            "x.positive?",
            "x.positive?\t{ }\n",
            6,
        ),
    ] {
        for ending in ["\n", "\r\n"] {
            for mode in ["inspect", "-a", "-A"] {
                let original = source.replace('\n', ending);
                let directory = project_with_ruby(&[("example.rb", &original)], "2.7");
                let mut run = command(directory.path());
                run.args([
                    "--cache",
                    "false",
                    "--only",
                    "Style/NumericPredicate",
                    "--format",
                    "json",
                ]);
                if mode != "inspect" {
                    run.arg(mode);
                }
                let correcting = mode == "-A";
                let output = run
                    .arg("example.rb")
                    .assert()
                    .code(if correcting { 0 } else { 1 })
                    .get_output()
                    .clone();
                let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["summary"]["target_file_count"], 1);
                assert_eq!(report["summary"]["inspected_file_count"], 1);
                assert_eq!(
                    report["files"][0]["offenses"],
                    serde_json::json!([{
                        "severity": "convention",
                        "message": format!("Use `{replacement}` instead of `{send}`."),
                        "cop_name": "Style/NumericPredicate",
                        "corrected": correcting,
                        "correctable": true,
                        "location": {"start_line": 1, "start_column": 1, "last_line": 1,
                            "last_column": length, "length": length, "line": 1, "column": 1}
                    }]),
                    "{source:?} {ending:?} {mode}"
                );
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    if correcting && ending == "\n" {
                        native_lf_disk_correction(corrected.as_bytes())
                    } else if correcting {
                        corrected.replace('\n', ending).into_bytes()
                    } else {
                        original.into_bytes()
                    }
                );
                assert!(output.stderr.is_empty());
            }
        }
    }
}

/// comparison の本家補正はブロックを残して構文を壊す。ブロックを消す回避はせず、既存の
/// 構文保護で原本と未補正の診断を返す。同じファイルの無害な別の補正も途中まで書かない。
#[test]
fn numeric_predicate_rejects_upstream_comparisons_that_leave_invalid_blocks() {
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let cases = manifest["numeric_predicate_block_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(cases.len(), 6);
    for case in cases {
        let before = case["source"].as_str().unwrap();
        let target = case["target_ruby"].as_str().unwrap();
        assert_ne!(case["upstream"]["source"].as_str().unwrap(), before);
        assert!(
            case["upstream"]["offenses"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|offense| offense["cop_name"].as_str() == Some("Lint/Syntax"))
        );
        let expected = serde_json::to_value(&case["sonicop"]["offenses"]).unwrap();
        for ending in ["\n", "\r\n"] {
            for mode in ["inspect", "-a", "-A"] {
                let original = before.replace('\n', ending);
                let directory = project_with_ruby(&[("example.rb", &original)], target);
                fs::write(directory.path().join(".rubocop.yml"), format!(
                    "AllCops:\n  TargetRubyVersion: {target}\n  NewCops: enable\nStyle/NumericPredicate:\n  EnforcedStyle: comparison\n"
                )).unwrap();
                // 拒否した -A を再実行しても同じ原本・診断・終了コードになる。
                for _ in 0..if mode == "-A" { 2 } else { 1 } {
                    let mut run = command(directory.path());
                    run.args([
                        "--cache",
                        "false",
                        "--only",
                        "Style/NumericPredicate",
                        "--format",
                        "json",
                    ]);
                    if mode != "inspect" {
                        run.arg(mode);
                    }
                    let output = run
                        .arg("example.rb")
                        .assert()
                        .code(if mode == "-A" { 2 } else { 1 })
                        .get_output()
                        .clone();
                    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                    assert_eq!(report["summary"]["target_file_count"], 1);
                    assert_eq!(report["summary"]["inspected_file_count"], 1);
                    assert_eq!(
                        report["files"][0]["offenses"],
                        expected,
                        "{} {ending:?} {mode}",
                        case["id"].as_str().unwrap()
                    );
                    assert_eq!(
                        fs::read(directory.path().join("example.rb")).unwrap(),
                        original.as_bytes()
                    );
                    let stderr = if mode == "-A" {
                        case["sonicop"]["stderr"].as_str().unwrap()
                    } else {
                        ""
                    };
                    assert_eq!(output.stderr, stderr.as_bytes());
                }
            }
        }
    }
}

/// 構文を壊すファイルを拒否しても、別ファイルの有効な補正は続ける。
#[test]
fn numeric_predicate_block_rejection_does_not_stop_other_files() {
    for ending in ["\n", "\r\n"] {
        let original = "foo.zero? { }\n".replace('\n', ending);
        let other = "bar.zero?\n".replace('\n', ending);
        let directory = project_with_ruby(&[("bad.rb", &original), ("good.rb", &other)], "2.7");
        fs::write(directory.path().join(".rubocop.yml"),
            "AllCops:\n  TargetRubyVersion: 2.7\n  NewCops: enable\nStyle/NumericPredicate:\n  EnforcedStyle: comparison\n").unwrap();
        let output = command(directory.path())
            .args([
                "--cache",
                "false",
                "--only",
                "Style/NumericPredicate",
                "-A",
                "--format",
                "json",
                "bad.rb",
                "good.rb",
            ])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["summary"]["target_file_count"], 2);
        assert_eq!(report["summary"]["inspected_file_count"], 2);
        assert_eq!(report["files"][0]["offenses"][0]["corrected"], false);
        assert_eq!(report["files"][1]["offenses"][0]["corrected"], true);
        assert_eq!(
            fs::read(directory.path().join("bad.rb")).unwrap(),
            original.as_bytes()
        );
        assert_eq!(
            fs::read(directory.path().join("good.rb")).unwrap(),
            if ending == "\n" {
                native_lf_disk_correction(b"bar == 0\n")
            } else {
                b"bar == 0\r\n".to_vec()
            }
        );
        assert_eq!(
            output.stderr,
            b"Autocorrection was not written to bad.rb because it introduced a syntax error.\n"
        );
        command(directory.path())
            .args(["--only", "Style/NumericPredicate", "-A", "good.rb"])
            .assert()
            .success();
        assert_eq!(
            fs::read(directory.path().join("good.rb")).unwrap(),
            if ending == "\n" {
                native_lf_disk_correction(b"bar == 0\n")
            } else {
                b"bar == 0\r\n".to_vec()
            }
        );
    }
}

#[test]
fn ambiguous_arguments_keep_ruby_whitespace_and_reference_correction_bytes() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/ambiguity_whitespace_cli.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 126);
    // 本家 1.89.0 の2版・2改行・3実行を記録し、全項目が同じことを確かめた。
    for target in ["2.7", "3.4"] {
        for ending in ["\n", "\r\n"] {
            for correction in [None, Some("-a"), Some("-A")] {
                let directory = project_with_ruby(&[], target);
                fs::write(
                    directory.path().join(".rubocop.yml"),
                    format!("AllCops:\n  TargetRubyVersion: {target}\n  NewCops: disable\n"),
                )
                .unwrap();
                for (index, case) in cases.iter().enumerate() {
                    fs::write(
                        directory.path().join(format!("input_{index:03}.rb")),
                        case["source"].as_str().unwrap().replace('\n', ending),
                    )
                    .unwrap();
                }
                let mut run = command(directory.path());
                run.args([
                    "--config",
                    ".rubocop.yml",
                    "--cache",
                    "false",
                    "--only",
                    "Lint/AmbiguousOperator,Lint/AmbiguousRegexpLiteral",
                    "--format",
                    "json",
                ]);
                if let Some(flag) = correction {
                    run.arg(flag);
                }
                let output = run
                    .arg(".")
                    .assert()
                    .code(if correction.is_some() { 0 } else { 1 })
                    .get_output()
                    .stdout
                    .clone();
                let found: serde_json::Value = serde_json::from_slice(&output).unwrap();
                assert_eq!(found["summary"]["inspected_file_count"], cases.len());
                assert_eq!(found["summary"]["target_file_count"], cases.len());
                assert_eq!(found["files"].as_array().unwrap().len(), cases.len());
                assert_eq!(found["summary"]["offense_count"], 94);
                for (index, case) in cases.iter().enumerate() {
                    let filename = format!("input_{index:03}.rb");
                    let file = found["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|file| file["path"] == filename)
                        .unwrap();
                    let mode = if correction.is_some() {
                        "corrected_offenses"
                    } else {
                        "inspected_offenses"
                    };
                    assert_eq!(
                        file["offenses"], case[mode],
                        "{} / {target} / {ending:?} / {correction:?}",
                        case["name"]
                    );
                    let source = if correction.is_some() {
                        "corrected_source"
                    } else {
                        "source"
                    };
                    // CRLF保持は既存の意図的差分。挿入した改行も元の形式へ戻す。
                    assert_eq!(
                        fs::read(directory.path().join(&filename)).unwrap(),
                        if ending == "\n" && case[source] != case["source"] {
                            native_lf_disk_correction(case[source].as_str().unwrap().as_bytes())
                        } else {
                            case[source]
                                .as_str()
                                .unwrap()
                                .replace('\n', ending)
                                .into_bytes()
                        },
                        "{} / {target} / {ending:?} / {correction:?}",
                        case["name"]
                    );
                }
            }
        }
    }
}

/// 本家 1.89.0 の実 CLI で記録した parser 回帰の診断全項目と補正バイト。
/// CRLF は既存の意図的差分に従ってディスクでも保つ。colon 欠落などは原本を読む診断 backend が
/// 判定し、tree-sitter が回復した木を受理しても補正で原本を書き換えない。
fn assert_recorded_parser_reports(fixture: &str, only: &str) {
    let fixture: serde_json::Value = serde_json::from_str(fixture).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        for ending in ["\n", "\r\n"] {
            let source = case["source"].as_str().unwrap().replace('\n', ending);
            let directory = project_without_pinned_ruby(&[
                ("example.rb", &source),
                (".rubocop.yml", case["yaml"].as_str().unwrap()),
            ]);
            let mut run = command(directory.path());
            run.args([
                "--config",
                ".rubocop.yml",
                "--cache",
                "false",
                "--only",
                only,
                "--format",
                "json",
            ]);
            if let Some(mode) = case["mode"].as_str() {
                run.arg(mode);
            }
            let output = run.arg("example.rb").output().unwrap();
            assert_eq!(
                output.status.code(),
                case["exit"].as_i64().map(|code| code as i32),
                "{name} {ending:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut report: serde_json::Value = serde_json::from_slice(&output.stdout)
                .unwrap_or_else(|error| {
                    panic!(
                        "{name}: {error}: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )
                });
            report.as_object_mut().unwrap().remove("metadata");
            assert_eq!(report, case["report"], "{name} {ending:?}");
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                if ending == "\n" && case["after"] != case["source"] {
                    let expected = case["after"].as_str().unwrap().as_bytes();
                    if expected.contains(&b'\r') {
                        native_lf_code_with_bare_cr(expected)
                    } else {
                        native_lf_disk_correction(expected)
                    }
                } else {
                    case["after"]
                        .as_str()
                        .unwrap()
                        .replace('\n', ending)
                        .into_bytes()
                },
                "{name} {ending:?}",
            );
        }
    }
}

/// parser 構成回帰。Windows 本家の改行変換と元形式保持で診断が変わる EndOfLine だけを
/// 両者で無効にし、残る既定 cop の全診断と実際の補正バイトを本家と照合する。
#[test]
fn percent_after_a_string_matches_upstream_without_end_of_line_cop() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/string_percent_reports.json")).unwrap();
    assert_eq!(
        fixture["excluded_cops"],
        serde_json::json!(["Layout/EndOfLine"])
    );
    let groups = fixture["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 12);
    for group in groups {
        let directory = project_without_pinned_ruby(&[]);
        fs::write(
            directory.path().join(".rubocop.yml"),
            group["yaml"].as_str().unwrap(),
        )
        .unwrap();
        let cases = group["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 6);
        for case in cases {
            fs::write(
                directory.path().join(case["filename"].as_str().unwrap()),
                case["source"].as_str().unwrap().as_bytes(),
            )
            .unwrap();
        }
        let mut run = command(directory.path());
        run.args([
            "--config",
            ".rubocop.yml",
            "--cache",
            "false",
            "--format",
            "json",
        ]);
        if let Some(mode) = group["mode"].as_str() {
            run.arg(mode);
        }
        let output = run.arg(".").output().unwrap();
        assert_eq!(
            output.status.code(),
            group["exit"].as_i64().map(|exit| exit as i32),
            "{} / {:?} / {}: {}",
            group["target"],
            group["ending"],
            group["mode"],
            String::from_utf8_lossy(&output.stderr)
        );
        let mut report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        report.as_object_mut().unwrap().remove("metadata");
        assert_eq!(
            report, group["report"],
            "{} / {:?} / {}",
            group["target"], group["ending"], group["mode"]
        );
        for case in cases {
            // CRLF 原本の保持方針を維持し、LF 原本への実補正だけ native の byte にする。
            let expected =
                if cfg!(windows) && group["ending"] == "\n" && case["after"] != case["source"] {
                    // 本家 Windows の実補正 byte を使う。無変更の LF 原本は変換しない。
                    case["after"].as_str().unwrap().to_owned()
                } else {
                    case["after"]
                        .as_str()
                        .unwrap()
                        .replace("\r\n", "\n")
                        .replace('\n', group["ending"].as_str().unwrap())
                };
            assert_eq!(
                fs::read(directory.path().join(case["filename"].as_str().unwrap())).unwrap(),
                expected.as_bytes(),
                "{} / {} / {:?} / {}",
                case["name"],
                group["target"],
                group["ending"],
                group["mode"]
            );
        }
        if group["mode"] == "-A" {
            let before: Vec<_> = cases
                .iter()
                .map(|case| {
                    fs::read(directory.path().join(case["filename"].as_str().unwrap())).unwrap()
                })
                .collect();
            let repeated = command(directory.path())
                .args(["--config", ".rubocop.yml", "--cache", "false", "-A", "."])
                .output()
                .unwrap();
            assert!(
                matches!(repeated.status.code(), Some(0 | 1)),
                "{}",
                String::from_utf8_lossy(&repeated.stderr)
            );
            for (case, expected) in cases.iter().zip(before) {
                assert_eq!(
                    fs::read(directory.path().join(case["filename"].as_str().unwrap())).unwrap(),
                    expected,
                    "二度目の -A: {} / {} / {:?}",
                    case["name"],
                    group["target"],
                    group["ending"]
                );
            }
        }
    }
}

#[test]
fn local_ternary_reports_and_corrections_match_recorded_upstream() {
    assert_recorded_parser_reports(
        include_str!("fixtures/local_ternary_reports.json"),
        "Layout/SpaceAroundOperators,Lint/Syntax",
    );
}

/// 元 VT と通常の改行、literal 内容、文字区切りとブロックを同じ本家記録で照合する。
#[test]
fn vertical_tab_reports_and_corrections_match_recorded_upstream() {
    assert_recorded_parser_reports(
        include_str!("fixtures/vertical_tab_reports.json"),
        "Lint/Syntax,Layout/SpaceAroundOperators,Style/BlockDelimiters",
    );
}

#[test]
fn nul_parse_projection_does_not_change_the_corrected_file_bytes() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/nul_writeback_cli.json")).unwrap();
    let groups = fixture["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 36);
    for (group_index, group) in groups.iter().enumerate() {
        let label = format!(
            "{} / {} / {} / {} / {}",
            group["ruby"], group["engine"], group["eol"], group["mode"], group["selection"]
        );
        let ruby = group["ruby"].as_str().unwrap();
        let directory = project_with_ruby(&[], ruby);
        fs::write(
            directory.path().join(".rubocop.yml"),
            format!(
                "AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {}\n",
                group["engine"].as_str().unwrap()
            ),
        )
        .unwrap();
        let cases = group["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 34);
        let decode = |hex: &str| {
            (0..hex.len())
                .step_by(2)
                .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
                .collect::<Vec<_>>()
        };
        for (index, case) in cases.iter().enumerate() {
            fs::write(
                directory.path().join(format!("input_{index:03}.rb")),
                decode(case["source_hex"].as_str().unwrap()),
            )
            .unwrap();
        }
        let only = if group["selection"] == "operator" {
            "Layout/SpaceAroundOperators"
        } else {
            "Layout/SpaceAroundOperators,Layout/TrailingWhitespace,Layout/TrailingEmptyLines,Layout/LineLength,Lint/EmptyFile"
        };
        let mut run = command(directory.path());
        run.args([
            "--config",
            ".rubocop.yml",
            "--cache",
            "false",
            "--format",
            "json",
            "--only",
            only,
        ]);
        if group["mode"] != "inspect" {
            run.arg(group["mode"].as_str().unwrap());
        }
        let output = run
            .arg(".")
            .assert()
            .code(group["exit"].as_i64().unwrap() as i32)
            .get_output()
            .stdout
            .clone();
        let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(report["summary"]["target_file_count"], cases.len());
        assert_eq!(report["summary"]["inspected_file_count"], cases.len());
        let files = report["files"].as_array().unwrap();
        assert_eq!(files.len(), cases.len());
        let paths: std::collections::BTreeSet<_> = files
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths.len(), cases.len());
        for (index, case) in cases.iter().enumerate() {
            let filename = format!("input_{index:03}.rb");
            let file = files.iter().find(|file| file["path"] == filename).unwrap();
            assert_eq!(
                file["offenses"], case["offenses"],
                "{label} / {}",
                case["name"]
            );
            let expected = decode(case["after_hex"].as_str().unwrap());
            // 本家の書き戻しはLFへ正規化するが、Sonicopは原本のCRLFを保つ。
            let expected = if decode(case["source_hex"].as_str().unwrap())
                .windows(2)
                .any(|pair| pair == b"\r\n")
            {
                String::from_utf8(expected)
                    .unwrap()
                    .replace("\r\n", "\n")
                    .replace('\n', "\r\n")
                    .into_bytes()
            } else {
                expected
            };
            assert_eq!(
                fs::read(directory.path().join(&filename)).unwrap(),
                if cfg!(windows) {
                    decode(
                        windows_native_cli_case("nul_writeback_cli.json", Some(group_index), index)
                            ["safe_after_hex"]
                            .as_str()
                            .unwrap(),
                    )
                } else {
                    expected
                },
                "{label} / {}",
                case["name"]
            );
        }
    }
}

#[test]
fn nul_parse_projection_preserves_the_stdin_buffer() {
    let records: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/nul_writeback_stdin.json")).unwrap();
    let records = records.as_array().unwrap();
    assert_eq!(records.len(), 252);
    let decode = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    for (index, case) in records.iter().enumerate() {
        let label = format!(
            "{} / {} / {} / {}",
            case["ruby"], case["eol"], case["mode"], case["name"]
        );
        let ruby = case["ruby"].as_str().unwrap();
        let directory = project_with_ruby(&[], ruby);
        fs::write(
            directory.path().join(".rubocop.yml"),
            format!(
                "AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {}\n",
                case["engine"].as_str().unwrap()
            ),
        )
        .unwrap();
        let source = decode(case["source_hex"].as_str().unwrap());
        for formatter in ["json", "simple"] {
            let mut run = command(directory.path());
            run.args(["--config", ".rubocop.yml", "--cache", "false", "--only", "Layout/SpaceAroundOperators,Layout/TrailingWhitespace,Layout/TrailingEmptyLines,Layout/LineLength,Lint/EmptyFile", "--stdin", "example.rb", "-f", formatter]);
            if case["mode"] != "inspect" {
                run.arg(case["mode"].as_str().unwrap());
            }
            let output = run
                .write_stdin(source.clone())
                .assert()
                .code(case["exit"].as_i64().unwrap() as i32)
                .get_output()
                .clone();
            assert!(output.stderr.is_empty(), "{label}: {:?}", output.stderr);
            if formatter == "json" {
                let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["summary"]["target_file_count"], 1, "{label}");
                assert_eq!(report["summary"]["inspected_file_count"], 1, "{label}");
                let files = report["files"].as_array().unwrap();
                assert_eq!(files.len(), 1, "{label}");
                assert_eq!(files[0]["path"], "example.rb", "{label}");
                assert_eq!(files[0]["offenses"], case["offenses"], "{label}");
            } else {
                let separator = stdin_separator().as_bytes();
                let position = output
                    .stdout
                    .windows(separator.len())
                    .position(|bytes| bytes == separator);
                if let Some(expected) = case["after_hex"].as_str() {
                    let expected = decode(expected);
                    // 原本にCRLFがある場合だけ、既存方針の改行保持を反映する。
                    let expected = if source.windows(2).any(|bytes| bytes == b"\r\n") {
                        String::from_utf8(expected)
                            .unwrap()
                            .replace("\r\n", "\n")
                            .replace('\n', "\r\n")
                            .into_bytes()
                    } else {
                        expected
                    };
                    let expected = if cfg!(windows) {
                        decode(
                            windows_stdin_buffer_case(index, case)["expected_stdout_hex"]
                                .as_str()
                                .unwrap(),
                        )
                    } else {
                        expected
                    };
                    let start = position.unwrap_or_else(|| panic!("{label}: missing stdin buffer"))
                        + separator.len();
                    if cfg!(windows) {
                        assert_eq!(
                            &output.stdout[..start],
                            decode(
                                windows_stdin_buffer_case(index, case)["upstream_stderr_hex"]
                                    .as_str()
                                    .unwrap(),
                            ),
                            "{label}"
                        );
                    }
                    assert_eq!(&output.stdout[start..], expected, "{label}");
                } else {
                    assert!(position.is_none(), "{label}");
                    if cfg!(windows) {
                        assert_eq!(
                            output.stdout,
                            decode(
                                windows_stdin_buffer_case(index, case)["upstream_stderr_hex"]
                                    .as_str()
                                    .unwrap(),
                            ),
                            "{label}"
                        );
                    }
                }
            }
            assert!(!directory.path().join("example.rb").exists(), "{label}");
        }
    }
}

#[test]
fn nul_truncated_tokens_limit_the_line_ending_inspection() {
    let records: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/nul_line_endings.json")).unwrap();
    let groups = records.as_array().unwrap();
    assert_eq!(groups.len(), 9);
    let decode = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    for (group_index, group) in groups.iter().enumerate() {
        let label = format!(
            "{} / {} / {}",
            group["ruby"], group["engine"], group["mode"]
        );
        let ruby = group["ruby"].as_str().unwrap();
        let directory = project_with_ruby(&[], ruby);
        fs::write(directory.path().join(".rubocop.yml"), format!(
            "AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {}\nLayout/EndOfLine:\n  EnforcedStyle: lf\n", group["engine"].as_str().unwrap()
        )).unwrap();
        let cases = group["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 10);
        for (index, case) in cases.iter().enumerate() {
            fs::write(
                directory.path().join(format!("input_{index:03}.rb")),
                decode(case["source_hex"].as_str().unwrap()),
            )
            .unwrap();
        }
        let mut run = command(directory.path());
        run.args([
            "--config",
            ".rubocop.yml",
            "--cache",
            "false",
            "--only",
            "Layout/SpaceAroundOperators,Layout/EndOfLine,Style/FrozenStringLiteralComment,Layout/SpaceAfterSemicolon",
            "-f",
            "json",
        ]);
        if group["mode"] != "inspect" {
            run.arg(group["mode"].as_str().unwrap());
        }
        let output = run
            .arg(".")
            .assert()
            .code(group["exit"].as_i64().unwrap() as i32)
            .get_output()
            .clone();
        assert!(output.stderr.is_empty(), "{label}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["summary"]["target_file_count"], cases.len());
        assert_eq!(report["summary"]["inspected_file_count"], cases.len());
        let files = report["files"].as_array().unwrap();
        assert_eq!(files.len(), cases.len());
        let paths: std::collections::BTreeSet<_> = files
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths.len(), cases.len());
        for (index, case) in cases.iter().enumerate() {
            let filename = format!("input_{index:03}.rb");
            let file = files.iter().find(|file| file["path"] == filename).unwrap();
            let windows =
                windows_native_cli_case("nul_line_endings.json", Some(group_index), index);
            assert_eq!(
                &file["offenses"],
                if cfg!(windows) {
                    &windows["expected_first_offenses"]
                } else {
                    &case["offenses"]
                },
                "{label} / {}",
                case["name"]
            );
            // 既存のCRLF保持方針はmixed入力の未変更行にも適用する。
            // 本家の生after_hexは残し、原本でCRLFだった同じ本文の行だけを戻す。
            let original = String::from_utf8(decode(case["source_hex"].as_str().unwrap())).unwrap();
            let crlf_lines: std::collections::BTreeSet<_> = original
                .split_inclusive('\n')
                .filter_map(|line| line.strip_suffix("\r\n"))
                .collect();
            let reference = String::from_utf8(decode(case["after_hex"].as_str().unwrap())).unwrap();
            let mut expected: String = reference
                .split_inclusive('\n')
                .map(|line| {
                    let content = line
                        .strip_suffix("\r\n")
                        .or_else(|| line.strip_suffix('\n'))
                        .unwrap_or(line);
                    if line.ends_with('\n') && crlf_lines.contains(content) {
                        format!("{content}\r\n")
                    } else {
                        line.to_owned()
                    }
                })
                .collect();
            if original.contains("\r\n") && !original.replace("\r\n", "").contains('\n') {
                // CRLFだけの原本では、新しく挿入された改行も同じ形式へ戻す。
                expected = expected.replace("\r\n", "\n").replace('\n', "\r\n");
            }
            assert_eq!(
                fs::read(directory.path().join(filename)).unwrap(),
                if cfg!(windows) {
                    decode(windows["safe_after_hex"].as_str().unwrap())
                } else {
                    expected.as_bytes().to_vec()
                },
                "{label} / {}",
                case["name"]
            );
        }
    }
}

#[test]
fn nul_truncated_tokens_bound_the_semicolon_scan() {
    let records: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/nul_semicolon_cli.json")).unwrap();
    let groups = records.as_array().unwrap();
    assert_eq!(groups.len(), 18);
    let decode = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    for (group_index, group) in groups.iter().enumerate() {
        let label = format!(
            "{} / {} / {} / {}",
            group["ruby"], group["engine"], group["eol"], group["mode"]
        );
        let ruby = group["ruby"].as_str().unwrap();
        let directory = project_with_ruby(&[], ruby);
        fs::write(
            directory.path().join(".rubocop.yml"),
            format!(
                "AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {}\n",
                group["engine"].as_str().unwrap()
            ),
        )
        .unwrap();
        let cases = group["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 16);
        for (index, case) in cases.iter().enumerate() {
            fs::write(
                directory.path().join(format!("input_{index:03}.rb")),
                decode(case["source_hex"].as_str().unwrap()),
            )
            .unwrap();
        }
        let mut run = command(directory.path());
        run.args([
            "--config",
            ".rubocop.yml",
            "--cache",
            "false",
            "--only",
            "Style/Semicolon",
            "--format",
            "json",
        ]);
        if group["mode"] != "inspect" {
            run.arg(group["mode"].as_str().unwrap());
        }
        let output = run
            .arg(".")
            .assert()
            .code(group["exit"].as_i64().unwrap() as i32)
            .get_output()
            .clone();
        assert!(output.stderr.is_empty(), "{label}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["summary"]["target_file_count"], cases.len());
        assert_eq!(report["summary"]["inspected_file_count"], cases.len());
        let files = report["files"].as_array().unwrap();
        assert_eq!(files.len(), cases.len());
        let paths: std::collections::BTreeSet<_> = files
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths.len(), cases.len());
        for (index, case) in cases.iter().enumerate() {
            let filename = format!("input_{index:03}.rb");
            let file = files.iter().find(|file| file["path"] == filename).unwrap();
            assert_eq!(
                file["offenses"], case["offenses"],
                "{label} / {}",
                case["name"]
            );
            let original = decode(case["source_hex"].as_str().unwrap());
            let reference = decode(case["after_hex"].as_str().unwrap());
            // 本家の生期待は残し、均一CRLF原本の補正だけ既存の保存方針を適用する。
            let expected = if group["mode"] != "inspect"
                && original.windows(2).any(|bytes| bytes == b"\r\n")
            {
                String::from_utf8(reference)
                    .unwrap()
                    .replace("\r\n", "\n")
                    .replace('\n', "\r\n")
                    .into_bytes()
            } else {
                reference
            };
            assert_eq!(
                fs::read(directory.path().join(filename)).unwrap(),
                if cfg!(windows) {
                    decode(
                        windows_native_cli_case("nul_semicolon_cli.json", Some(group_index), index)
                            ["safe_after_hex"]
                            .as_str()
                            .unwrap(),
                    )
                } else {
                    expected
                },
                "{label} / {}",
                case["name"]
            );
        }
    }
}

#[test]
fn logical_last_tokens_limit_the_line_ending_inspection() {
    let records: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/end_of_line_tokens.json")).unwrap();
    let windows_records: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/end_of_line_tokens_windows.json")).unwrap();
    let windows_groups = windows_records["groups"].as_array().unwrap();
    assert_eq!(windows_groups.len(), 2);
    let groups = records["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 6);
    let decode = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    for group in groups {
        // native には明示 crlf の EOF 例外がないため、Windows 実 CLI の別記録を使う。
        let expected = if cfg!(windows) && group["style"] == "native" {
            windows_groups
                .iter()
                .find(|candidate| candidate["engine"] == group["engine"])
                .unwrap()
        } else {
            group
        };
        let files = expected["files"].as_array().unwrap();
        assert_eq!(
            files.len(),
            records["cases"].as_object().unwrap().len()
                - if group["engine"] == "parser_whitequark" {
                    2
                } else {
                    0
                }
        );
        let directory = project(&[]);
        fs::write(
            directory.path().join(".rubocop.yml"),
            group["config"].as_str().unwrap(),
        )
        .unwrap();
        for mode in [None, Some("-a"), Some("-A")] {
            let label = format!("{} / {} / {mode:?}", group["engine"], group["style"]);
            for file in files {
                let name = file["path"].as_str().unwrap();
                fs::write(
                    directory.path().join(name),
                    decode(records["cases"][name].as_str().unwrap()),
                )
                .unwrap();
            }
            // EndOfLineは補正しない。再実行しても診断・終了値・原本全bytesが変わらない。
            for repeat in 0..2 {
                let mut run = command(directory.path());
                run.args([
                    "--config",
                    ".rubocop.yml",
                    "--cache",
                    "false",
                    "--only",
                    "Layout/EndOfLine",
                    "--format",
                    "json",
                ]);
                if let Some(mode) = mode {
                    run.arg(mode);
                }
                run.args(files.iter().map(|file| file["path"].as_str().unwrap()));
                let output = run
                    .assert()
                    .code(expected["exit"].as_i64().unwrap() as i32)
                    .get_output()
                    .clone();
                assert!(output.stderr.is_empty(), "{label} / {repeat}");
                let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["files"], expected["files"], "{label} / {repeat}");
                assert_eq!(
                    report["summary"]["target_file_count"],
                    files.len(),
                    "{label}"
                );
                assert_eq!(
                    report["summary"]["inspected_file_count"],
                    files.len(),
                    "{label}"
                );
                let count: usize = files
                    .iter()
                    .map(|file| file["offenses"].as_array().unwrap().len())
                    .sum();
                assert_eq!(report["summary"]["offense_count"], count, "{label}");
                for file in files {
                    let name = file["path"].as_str().unwrap();
                    assert_eq!(
                        fs::read(directory.path().join(name)).unwrap(),
                        decode(records["cases"][name].as_str().unwrap()),
                        "{label} / {name} / {repeat}"
                    );
                }
            }
        }
    }
}

/// 本家の先頭token実測を、診断全項目・ディスクの原本バイト・同mode再実行で検査する。
#[test]
fn leading_empty_lines_matches_recorded_real_tokens() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/leading_empty_lines.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 56);
    let groups = fixture["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 12);
    let decode = |value: &str| {
        (0..value.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    for (group_index, group) in groups.iter().enumerate() {
        let ruby = group["ruby"].as_str().unwrap();
        let directory = project_with_ruby(&[], ruby);
        fs::write(
            directory.path().join(".rubocop.yml"),
            format!(
                "AllCops:\n  NewCops: disable\n  TargetRubyVersion: {ruby}\n  ParserEngine: {}\n",
                group["engine"].as_str().unwrap()
            ),
        )
        .unwrap();
        let sources = group["source_hex"].as_array().unwrap();
        assert_eq!(sources.len(), cases.len());
        for (index, source) in sources.iter().enumerate() {
            fs::write(
                directory.path().join(format!("input_{index:03}.rb")),
                decode(source.as_str().unwrap()),
            )
            .unwrap();
        }
        for pass in ["first", "second"] {
            let label = format!(
                "{} / {} / {} / {} / {pass}",
                group["ruby"], group["engine"], group["ending"], group["mode"]
            );
            let expected = &group[pass];
            let mut run = command(directory.path());
            run.args([
                "--config",
                ".rubocop.yml",
                "--cache",
                "false",
                "--only",
                "Layout/LeadingEmptyLines",
                "--format",
                "json",
            ]);
            if group["mode"] != "inspect" {
                run.arg(group["mode"].as_str().unwrap());
            }
            let output = run
                .arg(".")
                .assert()
                .code(expected["exit"].as_i64().unwrap() as i32)
                .get_output()
                .clone();
            assert!(output.stderr.is_empty(), "{label}: {:?}", output.stderr);
            let mut report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            report.as_object_mut().unwrap().remove("metadata");
            assert_eq!(report, expected["report"], "{label}");
            let after = expected["after_hex"].as_array().unwrap();
            assert_eq!(after.len(), cases.len());
            for (index, value) in after.iter().enumerate() {
                let mut bytes = decode(value.as_str().unwrap());
                // 本家の生出力は保ち、既存のCRLF保持方針だけを期待値へ反映する。
                if group["ending"] == "CRLF" {
                    bytes = String::from_utf8(bytes)
                        .unwrap()
                        .replace("\r\n", "\n")
                        .replace('\n', "\r\n")
                        .into_bytes();
                }
                if cfg!(windows) {
                    bytes = decode(
                        windows_native_cli_case(
                            "leading_empty_lines.json",
                            Some(group_index),
                            index,
                        )["safe_after_hex"]
                            .as_str()
                            .unwrap(),
                    );
                }
                assert_eq!(
                    fs::read(directory.path().join(format!("input_{index:03}.rb"))).unwrap(),
                    bytes,
                    "{label} / {}",
                    cases[index]
                );
            }
        }
    }
}

/// 本家 1.89.0 は AST のない空文だけのファイルを補正せず、NUL 後や DATA も原本のまま返す。
/// LF/CRLF と両 engine で全 JSON・終了値・disk bytes を照合し、再実行でも消さない。
#[test]
fn semicolon_empty_sources_preserve_bytes_in_both_correction_modes() {
    let sources = [
        ";",
        ";;;\n",
        " \t; \n",
        ";\n;\n",
        "; # note\n",
        "# note\n;\n",
        ";\n__END__\nkept;\n",
        ";\0kept;\n",
    ];
    for (ruby, engine) in [("2.7", "parser_whitequark"), ("3.4", "parser_prism")] {
        for ending in ["\n", "\r\n"] {
            for mode in [None, Some("-a"), Some("-A")] {
                let config = format!("AllCops:\n  ParserEngine: {engine}\n  NewCops: disable\n");
                let directory = project_with_ruby(&[(".rubocop.yml", &config)], ruby);
                let mut files = Vec::new();
                let mut expected_files = Vec::new();
                for (index, source) in sources.iter().enumerate() {
                    let name = format!("input_{index}.rb");
                    let bytes = source.replace('\n', ending).into_bytes();
                    fs::write(directory.path().join(&name), &bytes).unwrap();
                    expected_files.push(serde_json::json!({"path": name, "offenses": []}));
                    files.push((name, bytes));
                }
                for _ in 0..2 {
                    let mut invocation = command(directory.path());
                    invocation.args([
                        "--cache",
                        "false",
                        "--only",
                        "Style/Semicolon",
                        "--format",
                        "json",
                    ]);
                    if let Some(mode) = mode {
                        invocation.arg(mode);
                    }
                    let output = invocation
                        .args(files.iter().map(|(name, _)| name))
                        .assert()
                        .success()
                        .stderr("")
                        .get_output()
                        .stdout
                        .clone();
                    let mut actual: serde_json::Value = serde_json::from_slice(&output).unwrap();
                    actual.as_object_mut().unwrap().remove("metadata");
                    assert_eq!(
                        actual,
                        serde_json::json!({
                            "files": expected_files,
                            "summary": {
                                "offense_count": 0,
                                "target_file_count": sources.len(),
                                "inspected_file_count": sources.len(),
                            },
                        }),
                        "{ruby}/{engine}/{ending:?}/{mode:?}",
                    );
                    for (name, bytes) in &files {
                        assert_eq!(fs::read(directory.path().join(name)).unwrap(), *bytes);
                    }
                }
            }
        }
    }
}

/// 同じ制御文字でも Buffer の encoding が診断の escape を変えるため、原文と実補正の記録で照合する。
#[test]
fn control_syntax_reports_match_recorded_upstream() {
    assert_recorded_parser_reports(
        include_str!("fixtures/control_syntax_reports.json"),
        "Lint/Syntax",
    );
}

/// 宣言名だけを変えた実 EUC-JP バイトで、SourceFile と構文 backend の双方を通す。
#[test]
fn euc_jp_alias_reports_and_raw_bytes_match_recorded_upstream() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/euc_jp_alias_reports.json")).unwrap();
    let decode_hex = |hex: &str| -> Vec<u8> {
        assert_eq!(hex.len() % 2, 0);
        (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect()
    };
    for case in fixture["records"].as_array().unwrap() {
        let directory =
            project_without_pinned_ruby(&[(".rubocop.yml", case["yaml"].as_str().unwrap())]);
        fs::write(
            directory.path().join("example.rb"),
            decode_hex(case["source_hex"].as_str().unwrap()),
        )
        .unwrap();
        for attempt in 0..2 {
            let reference = if attempt == 1 && !case["rerun_reference"].is_null() {
                &case["rerun_reference"]
            } else {
                &case["reference"]
            };
            let mut run = command(directory.path());
            run.args([
                "--config",
                ".rubocop.yml",
                "--cache",
                "false",
                "--only",
                "Layout/SpaceAroundOperators,Lint/Syntax",
                "--format",
                "json",
            ]);
            if let Some(mode) = case["mode"].as_str() {
                run.arg(mode);
            }
            let output = run.arg("example.rb").output().unwrap();
            assert_eq!(
                output.status.code(),
                reference["exit"].as_i64().map(|code| code as i32),
                "{case} / {attempt}"
            );
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                reference["stderr"].as_str().unwrap(),
                "{case} / {attempt}"
            );
            let mut report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            report.as_object_mut().unwrap().remove("metadata");
            assert_eq!(report, reference["report"], "{case} / {attempt}");
            let mut expected = decode_hex(reference["after_hex"].as_str().unwrap());
            // 本家の CRLF→LF 書戻しとは既存の意図的差分。生の本家 byte も記録に残す。
            if case["ending"] == "crlf"
                && !case["mode"].is_null()
                && case["reference"]["after_hex"] != case["source_hex"]
            {
                assert!(!expected.contains(&b'\r'));
                expected = expected
                    .into_iter()
                    .flat_map(|byte| {
                        if byte == b'\n' {
                            vec![b'\r', b'\n']
                        } else {
                            vec![byte]
                        }
                    })
                    .collect();
            }
            if case["ending"] == "lf"
                && !case["mode"].is_null()
                && case["reference"]["after_hex"] != case["source_hex"]
            {
                // この fixture で補正するのは ASCII の operator だけ。無変更の符号化原本は保つ。
                assert!(expected.is_ascii());
                expected = native_lf_disk_correction(&expected);
            }
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                expected,
                "{case} / {attempt}"
            );
        }
    }
}

#[test]
fn mixed_crlf_reinspection_preserves_copied_endings() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/mixed_reinspection.json")).unwrap();
    let cases = fixture["records"].as_array().unwrap();
    assert_eq!(cases.len(), 60);
    let decode = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    for (case_index, case) in cases.iter().enumerate() {
        let label = format!("{} / {} / {}", case["name"], case["engine"], case["mode"]);
        let directory = project(&[]);
        fs::write(
            directory.path().join(".rubocop.yml"),
            case["config"].as_str().unwrap(),
        )
        .unwrap();
        fs::write(
            directory.path().join("example.rb"),
            decode(case["source_hex"].as_str().unwrap()),
        )
        .unwrap();
        for key in ["first", "repeated"] {
            let windows = windows_native_cli_case("mixed_reinspection.json", None, case_index);
            let mut run = command(directory.path());
            run.args([
                "--config",
                ".rubocop.yml",
                "--cache",
                "false",
                "--only",
                "Layout/EndOfLine,Style/FrozenStringLiteralComment",
                "--format",
                "json",
            ]);
            if let Some(mode) = case["mode"].as_str() {
                run.arg(mode);
            }
            let output = run
                .arg("example.rb")
                .assert()
                .code(if cfg!(windows) {
                    windows[if key == "first" {
                        "expected_first_exit"
                    } else {
                        "expected_repeated_exit"
                    }]
                    .as_i64()
                    .unwrap() as i32
                } else {
                    case[key]["exit"].as_i64().unwrap() as i32
                })
                .get_output()
                .clone();
            assert!(output.stderr.is_empty(), "{label} / {key}");
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let expected_offenses = if cfg!(windows) {
                &windows[if key == "first" {
                    "expected_first_offenses"
                } else {
                    "expected_repeated_offenses"
                }]
            } else {
                &case[key]["offenses"]
            };
            assert_eq!(
                report["files"][0]["offenses"], *expected_offenses,
                "{label} / {key}"
            );
            assert_eq!(
                report["summary"]["inspected_file_count"], 1,
                "{label} / {key}"
            );
            let actual = fs::read(directory.path().join("example.rb")).unwrap();
            assert_eq!(
                actual,
                decode(if cfg!(windows) {
                    windows["safe_after_hex"].as_str().unwrap()
                } else {
                    case["after_hex"].as_str().unwrap()
                }),
                "{label} / {key}"
            );
            if let Some(expected) = case["data_hex"].as_str() {
                let marker = actual
                    .windows(7)
                    .position(|part| part == b"__END__")
                    .unwrap();
                let newline = actual[marker..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .unwrap();
                assert_eq!(
                    &actual[marker + newline + 1..],
                    decode(expected),
                    "{label} / DATA"
                );
            }
        }
    }
}

#[test]
fn mixed_bare_cr_copied_data_survives_multiple_correction_passes() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/mixed_bare_cr.json")).unwrap();
    let cases = fixture["records"].as_array().unwrap();
    assert_eq!(cases.len(), 36);
    let decode = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    for (case_index, case) in cases.iter().enumerate() {
        let label = format!("{} / {} / {}", case["name"], case["engine"], case["mode"]);
        let directory = project(&[]);
        fs::write(
            directory.path().join(".rubocop.yml"),
            case["config"].as_str().unwrap(),
        )
        .unwrap();
        fs::write(
            directory.path().join("example.rb"),
            decode(case["source_hex"].as_str().unwrap()),
        )
        .unwrap();
        for key in ["first", "repeated"] {
            let mut run = command(directory.path());
            run.args([
                "--config",
                ".rubocop.yml",
                "--cache",
                "false",
                "--only",
                fixture["only"].as_str().unwrap(),
                "--format",
                "json",
            ]);
            if let Some(mode) = case["mode"].as_str() {
                run.arg(mode);
            }
            let output = run
                .arg("example.rb")
                .assert()
                .code(case[key]["exit"].as_i64().unwrap() as i32)
                .get_output()
                .clone();
            assert!(output.stderr.is_empty(), "{label} / {key}");
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                report["files"][0]["offenses"], case[key]["offenses"],
                "{label} / {key}"
            );
            assert_eq!(report["summary"]["inspected_file_count"], 1, "{label}");
            assert_eq!(report["summary"]["target_file_count"], 1, "{label}");
            let actual = fs::read(directory.path().join("example.rb")).unwrap();
            assert_eq!(
                actual,
                decode(if cfg!(windows) {
                    windows_native_cli_case("mixed_bare_cr.json", None, case_index)["safe_after_hex"].as_str().unwrap()
                } else {
                    case["after_hex"].as_str().unwrap()
                }),
                "{label} / {key}"
            );
            let marker = actual
                .windows(7)
                .position(|part| part == b"__END__")
                .unwrap();
            let newline = actual[marker..]
                .iter()
                .position(|byte| *byte == b'\n')
                .unwrap();
            assert_eq!(
                &actual[marker + newline + 1..],
                decode(case["data_hex"].as_str().unwrap()),
                "{label} / DATA"
            );
        }
    }
}

#[test]
fn pure_crlf_preserves_original_bare_cr_through_correction_and_writeback() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pure_cr_preservation.json")).unwrap();
    let cases = fixture["records"].as_array().unwrap();
    assert_eq!(cases.len(), 192);
    let decode = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    for (case_index, case) in cases.iter().enumerate() {
        let label = format!("{} / {} / {}", case["name"], case["engine"], case["mode"]);
        let directory = project(&[]);
        fs::write(
            directory.path().join(".rubocop.yml"),
            case["config"].as_str().unwrap(),
        )
        .unwrap();
        fs::write(
            directory.path().join("example.rb"),
            decode(case["source_hex"].as_str().unwrap()),
        )
        .unwrap();
        let only = case["cops"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cop| cop.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(",");
        for key in ["first", "repeated"] {
            let mut run = command(directory.path());
            run.args([
                "--config",
                ".rubocop.yml",
                "--cache",
                "false",
                "--only",
                &only,
                "--format",
                "json",
            ]);
            if let Some(mode) = case["mode"].as_str() {
                run.arg(mode);
            }
            let output = run
                .arg("example.rb")
                .assert()
                .code(case[key]["exit"].as_i64().unwrap() as i32)
                .get_output()
                .clone();
            assert!(output.stderr.is_empty(), "{label} / {key}");
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                report["files"][0]["offenses"], case[key]["offenses"],
                "{label} / {key}"
            );
            assert_eq!(report["summary"]["inspected_file_count"], 1, "{label}");
            assert_eq!(report["summary"]["target_file_count"], 1, "{label}");
            let actual = fs::read(directory.path().join("example.rb")).unwrap();
            assert_eq!(
                actual,
                decode(if cfg!(windows) {
                    windows_native_cli_case("pure_cr_preservation.json", None, case_index)["safe_after_hex"].as_str().unwrap()
                } else {
                    case["after_hex"].as_str().unwrap()
                }),
                "{label} / {key}"
            );
            let marker = actual
                .windows(7)
                .position(|part| part == b"__END__")
                .unwrap();
            let offset = marker
                + actual[marker..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .unwrap()
                + 1;
            assert_eq!(
                &actual[offset..],
                decode(case["data_hex"].as_str().unwrap()),
                "{label} / DATA"
            );
        }
    }
}

#[test]
fn show_docs_urls_matches_the_upstream_cli() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/show_docs_urls.json")).unwrap();
    for (index, case) in cases.iter().enumerate() {
        let directory = project(&[]);
        fs::write(
            directory.path().join("config.yml"),
            case["config"].as_str().unwrap(),
        )
        .unwrap();
        let mut cmd = command(directory.path());
        cmd.args(["--config", "config.yml"]);
        if let Some(selector) = case["selector"].as_str() {
            cmd.arg(format!("--show-docs-url={selector}"));
        } else {
            cmd.arg("--show-docs-url");
        }
        let output = cmd.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case["exit"].as_i64().unwrap() as i32),
            "{index}"
        );
        assert_eq!(
            output.stdout,
            native_textmode_expected(case["stdout"].as_str().unwrap().as_bytes()),
            "{index}"
        );
        assert_eq!(
            output.stderr,
            native_textmode_expected(case["stderr"].as_str().unwrap().as_bytes()),
            "{index}"
        );
    }
}

#[test]
fn syntax_selection_errors_match_the_upstream_early_rejection() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/syntax_selection_error.json")).unwrap();
    for crlf in [false, true] {
        let source: &[u8] = if crlf { b"x=1\r\n" } else { b"x=1\n" };
        let config = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n";
        let directory = project(&[]);
        fs::write(directory.path().join("config.yml"), config).unwrap();
        fs::write(directory.path().join("case.rb"), source).unwrap();
        for case in &cases {
            let mut cmd = command(directory.path());
            cmd.args(["--config", "config.yml", "--except", "Lint/Syntax"]);
            for flag in case["flags"].as_array().unwrap() {
                cmd.arg(flag.as_str().unwrap());
            }
            let output = cmd.arg("case.rb").output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(case["exit"].as_i64().unwrap() as i32)
            );
            assert_eq!(output.stdout, case["stdout"].as_str().unwrap().as_bytes());
            assert_eq!(
                output.stderr,
                native_textmode_expected(case["stderr"].as_str().unwrap().as_bytes())
            );
            // 拒否は設定ロード・補正より前なので、両方の原本を保持する。
            assert_eq!(fs::read(directory.path().join("case.rb")).unwrap(), source);
            assert_eq!(
                fs::read_to_string(directory.path().join("config.yml")).unwrap(),
                config
            );
        }
    }
}

#[test]
fn explicit_parallel_option_notices_match_the_upstream_cli() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/parallel_option_notices.json")).unwrap();
    for crlf in [false, true] {
        for (index, case) in cases.iter().enumerate() {
            let directory = project(&[]);
            fs::write(
                directory.path().join(".rubocop.yml"),
                "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n",
            )
            .unwrap();
            let source: &[u8] = if crlf { b"x=1\r\n" } else { b"x=1\n" };
            fs::write(directory.path().join("case.rb"), source).unwrap();
            let mut cmd = command(directory.path());
            for arg in case["args"].as_array().unwrap() {
                cmd.arg(arg.as_str().unwrap());
            }
            let output = cmd.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(case["exit"].as_i64().unwrap() as i32),
                "{index}"
            );
            let prefix = native_textmode_expected(case["prefix"].as_str().unwrap().as_bytes());
            let body = output
                .stdout
                .strip_prefix(prefix.as_slice())
                .unwrap_or_else(|| panic!("通知が一致しない: {index}"));
            if case["error_after_notice"].as_bool() == Some(true) {
                assert!(!output.stderr.is_empty(), "{index}");
            } else if let Some(expected) = case.get("stderr") {
                assert_eq!(
                    output.stderr,
                    native_textmode_expected(expected.as_str().unwrap().as_bytes()),
                    "{index}"
                );
            }
            if let Some(expected) = case.get("stdout") {
                // 製品番号だけは意図した差であり、本家から通知や本文の期待値を変えない。
                let expected = if case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|arg| arg == "--version")
                {
                    expected
                        .as_str()
                        .unwrap()
                        .replace("1.89.0", env!("CARGO_PKG_VERSION"))
                } else {
                    expected.as_str().unwrap().to_owned()
                };
                assert_eq!(
                    body,
                    native_textmode_expected(expected.as_bytes()),
                    "{index}"
                );
            }
            if let Some(expected) = case.get("report") {
                let file_report;
                let bytes = match case["report_destination"].as_str().unwrap() {
                    "file" => {
                        file_report = fs::read(directory.path().join("report.json")).unwrap();
                        file_report.as_slice()
                    }
                    "stderr" => output.stderr.as_slice(),
                    "stdout" => body,
                    other => panic!("未知の出力先: {other}"),
                };
                let mut actual: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                actual.as_object_mut().unwrap().remove("metadata");
                assert_eq!(&actual, expected, "{index}");
            }
            assert_eq!(
                fs::read(directory.path().join("case.rb")).unwrap(),
                source,
                "{index}"
            );
        }
    }
}

#[test]
fn config_parameter_warnings_match_the_upstream_cli() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/config_parameter_warnings.json")).unwrap();
    let manifest: serde_yaml::Value =
        serde_yaml::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let divergence = &manifest["unsupported_parameter_parallel_warning_divergences"];
    assert_eq!(
        divergence["fixture"].as_str(),
        Some("config_parameter_warnings.json")
    );
    assert!(!divergence["reason"].as_str().unwrap().trim().is_empty());
    let reference = cases
        .iter()
        .find(|case| case["name"].as_str() == divergence["case"].as_str())
        .unwrap();
    let shared = cases
        .iter()
        .find(|case| case["name"].as_str() == divergence["candidate_case"].as_str())
        .unwrap();
    assert_eq!(reference["files"], shared["files"]);
    assert_eq!(reference["report"], shared["report"]);
    assert_eq!(reference["exit"], shared["exit"]);
    assert_ne!(reference["stderr"], shared["stderr"]);
    for (case, key) in [
        (reference, "upstream_warning_blocks"),
        (shared, "sonicop_warning_blocks"),
    ] {
        assert_eq!(
            case["stderr"].as_str().unwrap().matches("Warning:").count() as u64,
            divergence[key].as_u64().unwrap()
        );
    }
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        // 本家の fork fallback を、この POSIX 記録で Windows の成功とみなさない。
        if cfg!(windows) && name.starts_with("parallel explicit files ") {
            continue;
        }
        let expected = if name == divergence["case"].as_str().unwrap() {
            shared
        } else {
            case
        };
        let directory = project(&[]);
        for (path, body) in case["files"].as_object().unwrap() {
            let path = directory.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body.as_str().unwrap()).unwrap();
        }
        for _ in 0..case["repeat"].as_u64().unwrap_or(1) {
            let mut cmd = command(directory.path());
            cmd.env_remove("CLICOLOR_FORCE");
            cmd.env_remove("RUBOCOP_CACHE_ROOT");
            if case["cache_root_override"].as_bool() == Some(true) {
                cmd.env(
                    "RUBOCOP_CACHE_ROOT",
                    fs::canonicalize(directory.path()).unwrap().join("cache"),
                );
            }
            for arg in case["args"].as_array().unwrap() {
                cmd.arg(arg.as_str().unwrap());
            }
            if let Some(env) = case["env"].as_object() {
                for (key, value) in env {
                    cmd.env(key, value.as_str().unwrap());
                }
            }
            if let Some(stdin) = case["stdin"].as_str() {
                cmd.write_stdin(stdin);
            }
            let output = cmd.output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected["exit"].as_i64().unwrap() as i32),
                "{name}"
            );
            if case["prefix_only"].as_bool() == Some(true) {
                let prefix =
                    native_textmode_expected(case["warning_prefix"].as_str().unwrap().as_bytes());
                assert!(
                    output.stderr.starts_with(&prefix),
                    "{name}: {:?}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    !String::from_utf8_lossy(&output.stderr[prefix.len()..]).contains("Warning:"),
                    "{name}"
                );
            } else {
                assert_eq!(
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&native_textmode_expected(
                        expected["stderr"].as_str().unwrap().as_bytes()
                    )),
                    "{name}"
                );
            }
            // 子設定エラー時の終了 JSON 差 #152 は別件。警告 prefix と終了値は検査する。
            if case["warning_only"].as_bool() != Some(true) {
                if let Some(wanted) = expected.get("report") {
                    let mut stream = serde_json::Deserializer::from_slice(&output.stdout)
                        .into_iter::<serde_json::Value>();
                    let mut found = stream.next().unwrap().unwrap();
                    found.as_object_mut().unwrap().remove("metadata");
                    assert_eq!(&found, wanted, "{name}");
                    let tail = expected["stdout_tail"].as_str().unwrap_or("");
                    assert_eq!(
                        &output.stdout[stream.byte_offset()..],
                        native_textmode_expected(tail.as_bytes()),
                        "{name}"
                    );
                } else if let Some(wanted) = expected["stdout"].as_str() {
                    assert_eq!(
                        output.stdout,
                        native_textmode_expected(wanted.as_bytes()),
                        "{name}"
                    );
                }
            }
            for (path, body) in case["files"].as_object().unwrap() {
                assert_eq!(
                    fs::read(directory.path().join(path)).unwrap(),
                    body.as_str().unwrap().as_bytes(),
                    "{name} / {path}"
                );
            }
        }
    }
}
