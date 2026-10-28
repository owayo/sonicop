use std::process::Command;

#[test]
fn invalid_cache_is_rejected_before_version_notices_and_timers() {
    let directory = tempfile::tempdir().unwrap();
    // RuboCop 1.89.0 / Ruby 4.0.7 の実 CLI は版表示前に拒否し、timer も出さない。
    let expected: &[u8] = if cfg!(windows) {
        b"-C/--cache argument must be true or false\r\n"
    } else {
        b"-C/--cache argument must be true or false\n"
    };
    for version in ["--version", "--verbose-version"] {
        for flags in [
            vec!["--parallel"],
            vec!["--no-parallel"],
            vec!["--display-time"],
            vec!["--parallel", "--fail-fast", "--debug"],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_sonicop"))
                .current_dir(directory.path())
                .env_remove("RUBOCOP_OPTS")
                .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
                .args([version, "--cache", "invalid"])
                .args(&flags)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(2), "{version} {flags:?}");
            assert!(output.stdout.is_empty(), "{version} {flags:?}");
            assert_eq!(output.stderr, expected, "{version} {flags:?}");
        }
    }
}
