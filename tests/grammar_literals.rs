//! grammar 更新を、実本家の既定全cop・診断全項目・補正後の物理bytesで検査する。
//! 38入力×対象2.7/3.4/4.0×通常/-a/-Aの342条件。空%Qの既知バグ#258は別追跡。
//! 製品identity四項目以外のreport、stdout前置き・stderr、保存bytesを比較する。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use serde_json::Value;

const LOADER: &str = "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; gem 'rubocop-ast','1.50.0'; gem 'rubocop','1.89.0'; gem 'rainbow','3.1.1'; require 'rubocop';";

fn inputs() -> BTreeMap<String, Vec<u8>> {
    let mut literals = Vec::new();
    for (prefix, suffix) in [
        ("\"", "\""),
        ("%Q[", "]"),
        ("/", "/im"),
        ("%r[", "]im"),
        (":\"", "\""),
        ("`", "`"),
        ("%x[", "]"),
    ] {
        for body in ["", "文字", "\\n", "#{value}", "before\\n#{value}after"] {
            // #258は更新前から本家と異なる。空%QのAST自体はparserの単体テストへ残す。
            if prefix == "%Q[" && body.is_empty() {
                continue;
            }
            literals.push(format!("{prefix}{body}{suffix}"));
        }
    }
    for prefix in ["%w[", "%W[", "%i[", "%I["] {
        literals.push(format!("{prefix}one two\\ three #{{value}}]"));
    }
    assert_eq!(literals.len(), 38);
    literals
        .into_iter()
        .enumerate()
        .map(|(index, literal)| {
            (
                format!("literal_{index:02}.rb"),
                format!(
                    "# frozen_string_literal: true\nvalue = 1\nresult = {literal}\nputs result\n"
                )
                .into_bytes(),
            )
        })
        .collect()
}

fn environment(command: &mut Command, root: &Path) {
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "RUBOCOP_CACHE_ROOT",
        "NO_COLOR",
        "SONICOP_RUNTIME_RUBY",
        "SONICOP_RUNTIME_HANDOFF",
        "SONICOP_RUNTIME_HANDOFF_TOKEN",
    ] {
        command.env_remove(key);
    }
    command
        .current_dir(root)
        .env("TERM", "dumb")
        .env("CLICOLOR_FORCE", "0")
        .env("XDG_CONFIG_HOME", root.join("empty-config"))
        .env("XDG_CACHE_HOME", root.join("xdg"));
}

fn ruby(root: &Path) -> Option<PathBuf> {
    let ruby =
        PathBuf::from(std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into()));
    let mut command = Command::new(&ruby);
    environment(&mut command, root);
    let done = command.args(["-e", LOADER]).output();
    if done.as_ref().is_ok_and(|output| output.status.success()) {
        Some(ruby)
    } else {
        assert_ne!(
            std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
            Ok("1"),
            "固定Ruby/RuboCopが必要です: {done:?}"
        );
        eprintln!("未計測: 実Ruby/RuboCopがないためgrammarのCLI比較を成功へ数えません");
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
    prefix: Vec<u8>,
    stderr: Vec<u8>,
    report: Value,
    files: BTreeMap<String, Vec<u8>>,
}

fn capture(root: &Path, ruby: &Path, reference: bool, target: &str, mode: &str) -> Capture {
    fs::create_dir(root).unwrap();
    let files = inputs();
    let mut originals = files.clone();
    originals.insert("Gemfile".to_owned(), Vec::new());
    originals.insert("report.json".to_owned(), b"KEEP\0\r\n".to_vec());
    originals.insert(".rubocop.yml".to_owned(), format!("AllCops:\n  NewCops: disable\n  TargetRubyVersion: {target}\n  SuggestExtensions: false\n").into_bytes());
    for (name, bytes) in &originals {
        fs::write(root.join(name), bytes).unwrap();
    }
    let before: BTreeMap<_, _> = originals
        .keys()
        .map(|name| (name, snapshot(&root.join(name))))
        .collect();
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
    command.args(["--cache", "false", "--no-parallel", "--format", "json"]);
    if !mode.is_empty() {
        command.arg(mode);
    }
    let output = command.args(files.keys()).output().unwrap();
    let status = output.status.code().unwrap();
    assert!(
        [0, 1].contains(&status),
        "{target} {mode} {reference}: {output:?}"
    );
    assert!(
        output.stderr.is_empty(),
        "{target} {mode} {reference}: {output:?}"
    );
    assert_eq!(output.stdout.last(), Some(&b'}'));
    let start = output.stdout.iter().position(|&byte| byte == b'{').unwrap();
    let mut report: Value =
        serde_json::from_slice(&output.stdout[start..]).unwrap_or_else(|error| {
            panic!(
                "{target} {mode} {reference}: {error}: {}",
                String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(300)])
            )
        });
    assert_eq!(report["summary"]["target_file_count"], files.len());
    assert_eq!(report["summary"]["inspected_file_count"], files.len());
    assert_eq!(report["files"].as_array().unwrap().len(), files.len());
    assert!(report["summary"]["offense_count"].as_u64().unwrap() > 0);
    assert_eq!(report["metadata"]["rubocop_version"], "1.89.0");
    assert_eq!(
        report["metadata"]["ruby_engine"],
        if reference { "ruby" } else { "sonicop" }
    );
    if !reference {
        assert_eq!(
            report["metadata"]["sonicop_version"],
            env!("CARGO_PKG_VERSION")
        );
    }
    for key in [
        "sonicop_version",
        "ruby_engine",
        "ruby_version",
        "ruby_platform",
    ] {
        report["metadata"].as_object_mut().unwrap().remove(key);
    }
    for (name, original) in before {
        let after = snapshot(&root.join(name));
        if mode.is_empty() || !files.contains_key(name) || after.bytes == original.bytes {
            assert_eq!(after, original, "{target} {mode} {reference} {name}");
        } else {
            assert_eq!(after.mode, original.mode, "{name}");
        }
    }
    Capture {
        status,
        prefix: output.stdout[..start].to_vec(),
        stderr: output.stderr,
        report,
        files: files
            .keys()
            .map(|name| (name.clone(), fs::read(root.join(name)).unwrap()))
            .collect(),
    }
}

#[test]
fn literal_reports_and_corrections_match_real_rubocop() {
    let directory = tempfile::tempdir().unwrap();
    let Some(ruby) = ruby(directory.path()) else {
        return;
    };
    // whitequarkとPrismを含め、同じ人工入力を別processで実本家から先に測る。
    for target in ["2.7", "3.4", "4.0"] {
        for (index, mode) in ["", "-a", "-A"].into_iter().enumerate() {
            let reference = capture(
                &directory.path().join(format!("reference-{target}-{index}")),
                &ruby,
                true,
                target,
                mode,
            );
            let native = capture(
                &directory.path().join(format!("native-{target}-{index}")),
                &ruby,
                false,
                target,
                mode,
            );
            assert_eq!(native.status, reference.status, "{target} {mode}");
            assert_eq!(native.prefix, reference.prefix, "{target} {mode}");
            assert_eq!(native.stderr, reference.stderr, "{target} {mode}");
            assert_eq!(
                native
                    .report
                    .as_object()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>(),
                reference
                    .report
                    .as_object()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
            );
            assert_eq!(native.report["metadata"], reference.report["metadata"]);
            for (actual, expected) in native.report["files"]
                .as_array()
                .unwrap()
                .iter()
                .zip(reference.report["files"].as_array().unwrap())
            {
                assert_eq!(actual, expected, "{target} {mode}: {}", expected["path"]);
            }
            assert_eq!(native.report["summary"], reference.report["summary"]);
            for (name, bytes) in reference.files {
                assert_eq!(native.files[&name], bytes, "{target} {mode} {name}");
            }
        }
    }
}
