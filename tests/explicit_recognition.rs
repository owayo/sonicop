//! 明示対象の認識・設定の参照位置・先行失敗を、実RuboCopから先に測る。
//! 13条件を比較し、RubyInterpretersと隠しpathのshortcut差は#256/#263で別に追跡する。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use serde_json::Value;

const LOADER: &str = "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; gem 'rubocop-ast','1.50.0'; gem 'rubocop','1.89.0'; gem 'rainbow','3.1.1'; require 'rubocop';";
const BASE: &str = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n";
const RB: &str = "  Include: ['**/*.rb']\n";
const TXT: &str = "  Include: ['**/*.txt']\n";
const MISSING: &str = "inherit_from: missing.yml\n";
const FORCE: &[&str] = &["--force-exclusion"];
const IGNORE: &[&str] = &["--force-exclusion", "--ignore-parent-exclusion"];

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
        eprintln!("未計測: 実Ruby/RuboCopがないため明示対象の比較を成功へ数えません");
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
fn explicit_recognition_and_configuration_visit_order_match_real_rubocop() {
    let directory = tempfile::tempdir().unwrap();
    let Some(ruby) = ruby(directory.path()) else {
        return;
    };
    // #236で直った基本形も残し、rootのIncludeと個別のExcludeを取り違えない。
    let cases = [
        ("plain-force", "", "", "notes.txt", "x=1\n", FORCE),
        ("plain-explicit", "", "", "notes.txt", "x=1\n", &[]),
        ("custom-include", TXT, "", "notes.txt", "x=1\n", FORCE),
        (
            "changed-extension",
            "  Include: ['**/*.ruby']\n",
            "",
            "a.rb",
            "x=1\n",
            FORCE,
        ),
        (
            "ruby-shebang",
            "",
            "",
            "bin/run",
            "#!/usr/bin/env ruby\nx=1\n",
            FORCE,
        ),
        (
            "extension-shebang",
            "",
            "",
            "run.txt",
            "#!/usr/bin/env ruby\nx=1\n",
            FORCE,
        ),
        ("root-include", TXT, RB, "nested/notes.txt", "x=1\n", IGNORE),
        (
            "nested-include",
            RB,
            TXT,
            "nested/notes.txt",
            "x=1\n",
            IGNORE,
        ),
        (
            "nested-exclude",
            TXT,
            "  Exclude: ['notes.txt']\n",
            "nested/notes.txt",
            "x=1\n",
            IGNORE,
        ),
        (
            "skip-before-child",
            RB,
            MISSING,
            "nested/notes.txt",
            "x=1\n",
            IGNORE,
        ),
        (
            "keep-reads-child",
            TXT,
            MISSING,
            "nested/notes.txt",
            "x=1\n",
            IGNORE,
        ),
        (
            "skip-without-ignore",
            RB,
            MISSING,
            "nested/notes.txt",
            "x=1\n",
            FORCE,
        ),
        (
            "ordinary-reads-child",
            RB,
            MISSING,
            "nested/notes.txt",
            "x=1\n",
            &[],
        ),
    ];
    assert_eq!(cases.len(), 13);
    for (name, root_config, child_config, target, body, flags) in cases {
        let root = directory.path().join(name);
        fs::create_dir(&root).unwrap();
        let mut files = BTreeMap::from([
            ("Gemfile", Vec::new()),
            (".rubocop.yml", format!("{BASE}{root_config}").into_bytes()),
            (target, body.as_bytes().to_vec()),
            ("report.json", b"KEEP\0\r\n".to_vec()),
        ]);
        if !child_config.is_empty() {
            files.insert(
                "nested/.rubocop.yml",
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
            .map(|path| (*path, snapshot(&root.join(path))))
            .collect();
        let mut measured = Vec::new();
        for reference in [true, false] {
            measured.push(capture(&root, &ruby, reference, flags, target));
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
