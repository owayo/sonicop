//! RubyInterpretersと設定の訪問順を、実RuboCopの21条件42CLIで比較する。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use serde_json::Value;

const LOADER: &str = "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; gem 'rubocop-ast','1.50.0'; gem 'rubocop','1.89.0'; gem 'rainbow','3.1.1'; require 'rubocop';";
const BASE: &str = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n";
const MISSING: &str = "inherit_from: missing.yml\n";
const FORCE: &[&str] = &["--force-exclusion"];

fn environment(command: &mut Command, root: &Path) {
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "NO_COLOR",
        "SONICOP_RUNTIME_RUBY",
        "SONICOP_RUNTIME_HANDOFF",
        "SONICOP_RUNTIME_HANDOFF_TOKEN",
        "SONICOP_PROFILE",
    ] {
        command.env_remove(key);
    }
    command
        .current_dir(root)
        .env("TERM", "dumb")
        .env("CLICOLOR_FORCE", "0")
        .env("XDG_CONFIG_HOME", root.join("empty-config"))
        .env("RUBOCOP_CACHE_ROOT", root.join("cache"));
}

fn ruby(root: &Path) -> Option<PathBuf> {
    let path =
        PathBuf::from(std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into()));
    let mut command = Command::new(&path);
    environment(&mut command, root);
    let output = command.args(["-e", LOADER]).output();
    if output.as_ref().is_ok_and(|output| output.status.success()) {
        Some(path)
    } else {
        assert_ne!(
            std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
            Ok("1"),
            "固定Ruby/RuboCopが必要です: {output:?}"
        );
        eprintln!("未計測: 実Ruby/RuboCopがないためRubyInterpretersの比較を成功へ数えません");
        None
    }
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    bytes: Vec<u8>,
    modified: SystemTime,
    mode: u32,
}

fn snapshot(path: &Path) -> Snapshot {
    let info = fs::metadata(path).unwrap();
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        info.permissions().mode()
    };
    #[cfg(windows)]
    let mode = u32::from(info.permissions().readonly());
    Snapshot {
        bytes: fs::read(path).unwrap(),
        modified: info.modified().unwrap(),
        mode,
    }
}

#[derive(Debug, PartialEq)]
struct Capture {
    status: i32,
    report: Option<Value>,
    stderr: Vec<u8>,
}

fn capture(root: &Path, ruby: &Path, reference: bool, flags: &[&str], target: &str) -> Capture {
    let mut command = if reference {
        let mut command = Command::new(ruby);
        command.args([
            "-e",
            &format!("{LOADER}\nexit RuboCop::CLI.new.run(ARGV)\n"),
            "--",
        ]);
        command
    } else {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.env("PATH", "");
        command
    };
    environment(&mut command, root);
    let output = command
        .args(["--cache", "false", "--format", "json"])
        .args(flags)
        .arg(target)
        .output()
        .unwrap();
    let status = output.status.code().unwrap();
    assert!([0, 1, 2].contains(&status), "{output:?}");
    let report = if output.stdout.is_empty() {
        None
    } else {
        assert_eq!(output.stdout.first(), Some(&b'{'), "{output:?}");
        assert_eq!(output.stdout.last(), Some(&b'}'), "{output:?}");
        let mut value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["metadata"]["rubocop_version"], "1.89.0");
        assert_eq!(
            value["metadata"]["ruby_engine"],
            if reference { "ruby" } else { "sonicop" }
        );
        if !reference {
            assert_eq!(
                value["metadata"]["sonicop_version"],
                env!("CARGO_PKG_VERSION")
            );
        }
        for key in [
            "sonicop_version",
            "ruby_engine",
            "ruby_version",
            "ruby_platform",
        ] {
            value["metadata"].as_object_mut().unwrap().remove(key);
        }
        Some(value)
    };
    Capture {
        status,
        report,
        stderr: output.stderr,
    }
}

#[test]
fn ruby_interpreters_and_lookup_order_match_real_rubocop() {
    let directory = tempfile::tempdir().unwrap();
    let Some(ruby) = ruby(directory.path()) else {
        return;
    };
    let custom = "  RubyInterpreters: [crb]\n";
    let crb = b"#!/usr/bin/env crb\nx=1\n".as_slice();
    let standard = b"#!/usr/bin/env ruby\nx=1\n".as_slice();
    let cases = [
        (
            "default-ruby",
            "",
            "",
            "bin/run",
            standard,
            FORCE,
            "bin/run",
        ),
        ("custom-crb", custom, "", "bin/run", crb, FORCE, "bin/run"),
        (
            "removed-ruby",
            custom,
            "",
            "bin/run",
            standard,
            FORCE,
            "bin/run",
        ),
        (
            "empty-list",
            "  RubyInterpreters: []\n",
            "",
            "bin/run",
            b"#!/bin/sh\nx=1\n",
            FORCE,
            "bin/run",
        ),
        (
            "regex",
            "  RubyInterpreters: ['rb[0-9]+']\n",
            "",
            "bin/run",
            b"#! rb3\nx=1\n",
            FORCE,
            "bin/run",
        ),
        (
            "nested-crb",
            "",
            custom,
            "nested/run",
            crb,
            FORCE,
            "nested/run",
        ),
        (
            "nested-removed",
            "",
            custom,
            "nested/run",
            standard,
            FORCE,
            "nested/run",
        ),
        ("directory-crb", custom, "", "bin/run", crb, &[], "bin"),
        (
            "directory-removed",
            custom,
            "",
            "bin/run",
            standard,
            &[],
            "bin",
        ),
        (
            "directory-child-crb",
            "",
            custom,
            "bin/nested/run",
            crb,
            &[],
            "bin",
        ),
        (
            "directory-child-removed",
            "",
            custom,
            "bin/nested/run",
            standard,
            &[],
            "bin",
        ),
        (
            "unanchored",
            "",
            "",
            "bin/run",
            b"  #! ruby\nx=1\n",
            FORCE,
            "bin/run",
        ),
        (
            "newline-lf",
            "  RubyInterpreters: ['crb\\n']\n",
            "",
            "bin/run",
            crb,
            FORCE,
            "bin/run",
        ),
        (
            "newline-crlf",
            "  RubyInterpreters: ['crb\\n']\n",
            "",
            "bin/run",
            b"#! crb\r\nx=1\r\n",
            FORCE,
            "bin/run",
        ),
        (
            "invalid-utf8",
            "",
            "",
            "bin/run",
            b"#!ruby\xff\nx=1\n",
            FORCE,
            "bin/run",
        ),
        (
            "no-shebang-reads-child",
            "",
            MISSING,
            "nested/run",
            b"x=1\n",
            FORCE,
            "nested/run",
        ),
        (
            "empty-skips-child",
            "",
            MISSING,
            "nested/run",
            b"",
            FORCE,
            "nested/run",
        ),
        (
            "include-after-shebang",
            "  Include: ['nested/run']\n",
            MISSING,
            "nested/run",
            b"x=1\n",
            FORCE,
            "nested/run",
        ),
        (
            "lookbehind",
            "  RubyInterpreters: ['(?<=cr)b']\n",
            "",
            "bin/run",
            crb,
            FORCE,
            "bin/run",
        ),
        (
            "unicode",
            "  RubyInterpreters: ['サクラ']\n",
            "",
            "bin/run",
            "#! サクラ\nx=1\n".as_bytes(),
            FORCE,
            "bin/run",
        ),
        // basename shortcutはshebangより先。誤って子設定を先読みすると失敗の時期が変わる。
        (
            "basename-before-shebang",
            "",
            MISSING,
            "nested/Gemfile",
            b"x=1\n",
            FORCE,
            "nested/Gemfile",
        ),
    ];
    assert_eq!(cases.len(), 21);
    for (name, root_config, child_config, target, body, flags, argument) in cases {
        let root = directory.path().join(name);
        fs::create_dir(&root).unwrap();
        let mut files = BTreeMap::from([
            ("Gemfile".to_owned(), Vec::new()),
            (
                ".rubocop.yml".to_owned(),
                format!("{BASE}{root_config}").into_bytes(),
            ),
            (target.to_owned(), body.to_vec()),
            ("report.json".to_owned(), b"KEEP\0\r\n".to_vec()),
        ]);
        if !child_config.is_empty() {
            let child = Path::new(target).parent().unwrap().join(".rubocop.yml");
            files.insert(
                child.to_string_lossy().into_owned(),
                format!("{BASE}{child_config}").into_bytes(),
            );
        }
        for (path, bytes) in &files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let original: Vec<_> = files
            .keys()
            .map(|path| (path, snapshot(&root.join(path))))
            .collect();
        let mut measured = Vec::new();
        // 期待値は毎回本家を先に動かして取得し、NativeにはRubyの実行経路を与えない。
        for reference in [true, false] {
            measured.push(capture(&root, &ruby, reference, flags, argument));
            for (path, before) in &original {
                assert_eq!(
                    snapshot(&root.join(path)),
                    *before,
                    "{name} {reference} {path}"
                );
            }
        }
        assert_eq!(measured[1], measured[0], "{name}");
    }
}

#[test]
fn utf8_matches_follow_real_ruby_regexp_results() {
    // MRI 4.0.7のRegexp.new/match?を先に実行した値。US-ASCIIでのbyte一致にしない。
    let cases = [
        (r"#!.*(ruby|rake|jruby)", "#!/usr/bin/env ruby\n", true),
        (r"#!.*(crb)", "#!/usr/bin/env ruby\n", false),
        (r"#!.*()", "#!/bin/sh\n", true),
        (r"#!.*(rb[0-9]+)", "#! rb3\n", true),
        (r"#!.*((?<=cr)b)", "#! crb\n", true),
        (r"#!.*(サクラ)", "#! サクラ\n", true),
        (r"#!.*(crb\n)", "#! crb\n", true),
        (r"#!.*(crb\n)", "#! crb\r\n", false),
        (r"#!.*((a)\2)", "#! aa\n", true),
        (r"#!.*(ruby)", "  #! ruby\n", true),
        ("", "", true),
        (r"#!.*(...)$", "#!あ\n", false),
    ];
    // 名前取得の前処理がarenaを解放しても、cacheに残したbytecodeは生存する。
    for _ in 0..2 {
        for (pattern, text, expected) in cases {
            sonicop_onigmo_compat::check(
                br"(?<other>\u3042)",
                sonicop_onigmo_compat::Encoding::Utf8,
                false,
            )
            .unwrap();
            assert_eq!(
                sonicop_onigmo_compat::matches_utf8(pattern.as_bytes(), text.as_bytes()),
                Ok(Some(expected)),
                "{pattern} {text:?}"
            );
        }
    }
    assert_eq!(
        sonicop_onigmo_compat::matches_utf8(b"#!.*(ruby)", b"#!ruby\xff\n"),
        Ok(None)
    );
    let rejected = Err(b"end pattern with unmatched parenthesis: /(/".to_vec());
    assert_eq!(sonicop_onigmo_compat::matches_utf8(b"(", b"x"), rejected);
    // Regexp.newの失敗は本文のencoding検証より先に返る。
    assert_eq!(sonicop_onigmo_compat::matches_utf8(b"(", b"\xff"), rejected);
}

#[test]
fn cached_matches_do_not_use_the_callers_small_stack() {
    let pattern = format!("{}a{}", "(".repeat(3000), ")".repeat(3000));
    let result = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || sonicop_onigmo_compat::matches_utf8(pattern.as_bytes(), b"a"))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(result, Ok(Some(true)));
}
