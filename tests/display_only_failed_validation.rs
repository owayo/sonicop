//! RuboCop 1.89.0 / Ruby 4.0.7 の実 CLI で確認した引数検証の順序と本文。

use std::fs;
use std::path::Path;
use std::process::Command;

fn command(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command.current_dir(directory).env("CLICOLOR_FORCE", "0");
    for name in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        command.env_remove(name);
    }
    command
}

fn native_line(message: &str) -> Vec<u8> {
    format!("{message}{}", if cfg!(windows) { "\r\n" } else { "\n" }).into_bytes()
}

#[test]
fn incompatible_formats_fail_before_output_or_autocorrection() {
    let directory = tempfile::tempdir().unwrap();
    let inputs = [
        ("Gemfile", ""),
        ("case.rb", "x=1\n"),
        (
            "config.yml",
            "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n",
        ),
    ];
    let originals: Vec<_> = inputs
        .iter()
        .map(|(name, text)| {
            let path = directory.path().join(name);
            fs::write(&path, text).unwrap();
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            (path, text.as_bytes(), modified)
        })
        .collect();
    // OptionsValidator は通知・版表示・timer・書き戻しより前に、この全文で拒否する。
    let expected =
        native_line("--display-only-failed can only be used together with --format junit.");
    let scenarios: &[&[&str]] = &[
        &[
            "--config",
            "config.yml",
            "--only",
            "Layout/SpaceAroundOperators",
            "--no-display-cop-names",
            "--format",
            "json",
            "--no-parallel",
            "--cache",
            "false",
            "--display-only-failed",
            "case.rb",
        ],
        &["--display-only-failed"],
        &["--display-only-failed", "--format", "json"],
        &[
            "--version",
            "--display-only-failed",
            "--format",
            "json",
            "--cache",
            "false",
            "--parallel",
            "--display-time",
            "--debug",
        ],
        &[
            "--verbose-version",
            "--display-only-failed",
            "--format",
            "json",
            "--cache",
            "false",
            "--no-parallel",
        ],
        &["--display-only-failed", "--format", "json", "-a", "case.rb"],
        &["--display-only-failed", "--format", "json", "-A", "case.rb"],
        &["--version", "--display-only-failed", "--format", "ju"],
        &[
            "--version",
            "--display-only-failed",
            "--format",
            "junit",
            "--format",
            "json",
        ],
        &[
            "--version",
            "--display-only-failed",
            "--format",
            "RuboCop::Formatter::JUnitFormatter",
        ],
    ];
    for args in scenarios {
        let output = command(directory.path()).args(*args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert_eq!(output.stderr, expected, "{args:?}");
        assert!(!directory.path().join("report.json").exists());
        for (path, bytes, modified) in &originals {
            assert_eq!(fs::read(path).unwrap(), *bytes, "{args:?}: {path:?}");
            assert_eq!(
                fs::metadata(path).unwrap().modified().unwrap(),
                *modified,
                "{args:?}: {path:?}"
            );
        }
    }
}

#[test]
fn only_the_last_canonical_junit_format_permits_display_only_failed() {
    let directory = tempfile::tempdir().unwrap();
    for formats in [
        vec!["--format", "junit"],
        vec!["--format", "json", "--format", "junit"],
    ] {
        let output = command(directory.path())
            .args(["--version", "--display-only-failed"])
            .args(&formats)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{formats:?}");
        assert!(output.stderr.is_empty(), "{formats:?}: {output:?}");
        // 版の値は製品ごとのidentityなので、本家の1.89.0と等置しない。
        assert_eq!(output.stdout, native_line(env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn invalid_cache_keeps_precedence_over_display_only_failed() {
    let directory = tempfile::tempdir().unwrap();
    let output = command(directory.path())
        .args([
            "--display-only-failed",
            "--format",
            "json",
            "--cache",
            "invalid",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        native_line("-C/--cache argument must be true or false")
    );
}

#[test]
fn ordinary_config_errors_keep_the_error_prefix() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("bad.yml"), "true\n").unwrap();
    fs::write(directory.path().join("case.rb"), "x=1\n").unwrap();
    let output = command(directory.path())
        .args([
            "--config", "bad.yml", "--cache", "false", "--format", "json", "case.rb",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    // 一般設定エラーのpath表記と改行は、引数拒否の全文一致には含めない。
    assert!(output.stderr.starts_with(b"Error: Malformed configuration"));
}
