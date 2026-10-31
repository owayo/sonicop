//! 値なしCSVの拒否位置とRubyの副作用を、実RuboCopと両起動経路で比較する。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const LOADER: &str = "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; gem 'rubocop-ast','1.50.0'; gem 'rubocop','1.89.0'; gem 'rainbow','3.1.1'; require 'rubocop';";
const BASE: &str = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n";
const FIRST: &str = "puts 'FIRST_MARKER'\nFile.binwrite('first.loaded', \"first\\n\")\nat_exit { STDERR.puts(\"leave:#{$!.is_a?(SystemExit) ? $!.status : 'nil'}\") }\n";
const FORMS: &[(&str, &[&str])] = &[
    ("end", &["--SELECT"]),
    ("help", &["--SELECT", "--help"]),
    ("short-help", &["--SELECT", "-h"]),
    ("version", &["--SELECT", "--version"]),
    ("verbose", &["--SELECT", "--verbose-version"]),
    ("first-before", &["-r./first.rb", "--SELECT", "--help"]),
    ("later-after", &["--SELECT", "-r./later.rb", "--help"]),
    ("split-require", &["--require", "./first.rb", "--SELECT"]),
    (
        "both-before",
        &["-r./first.rb", "-r./later.rb", "--SELECT", "--help"],
    ),
    (
        "first-before-later-after",
        &["-r./first.rb", "--SELECT", "-r./later.rb", "--help"],
    ),
    (
        "repeated",
        &[
            "--SELECT",
            "Lint/Syntax",
            "-r./first.rb",
            "--SELECT",
            "--help",
        ],
    ),
    (
        "file-before",
        &["a.rb", "-r./first.rb", "--SELECT", "--help"],
    ),
    (
        "out-before",
        &["--out", "report.json", "-r./first.rb", "--SELECT"],
    ),
    ("correct-before", &["-A", "-r./first.rb", "--SELECT"]),
    ("color-before", &["--color", "--SELECT", "--help"]),
    ("force-color", &["--SELECT", "--help"]),
    (
        "force-no-color",
        &["--no-color", "-r./first.rb", "--SELECT", "--help"],
    ),
    ("zero-before", &["-r./zero.rb", "--SELECT", "--help"]),
    (
        "first-zero-before",
        &["-r./first.rb", "-r./zero.rb", "--SELECT"],
    ),
    ("stop-before", &["-r./stop.rb", "--SELECT", "--help"]),
    (
        "root-before",
        &[
            "--cache",
            "false",
            "--cache-root",
            "cache",
            "-r./first.rb",
            "--SELECT",
        ],
    ),
    (
        "root-after",
        &[
            "-r./first.rb",
            "--SELECT",
            "--cache",
            "false",
            "--cache-root",
            "cache",
            "--help",
        ],
    ),
    (
        "first-root-before",
        &[
            "-r./first.rb",
            "--cache",
            "false",
            "--cache-root",
            "cache",
            "--SELECT",
        ],
    ),
    ("help-first", &["-r./first.rb", "--help", "--SELECT"]),
    ("version-first", &["--version", "-r./first.rb", "--SELECT"]),
    ("empty-control", &["-r./first.rb", "--SELECT", "", "--help"]),
    (
        "valid-control",
        &["--SELECT", "Lint/Syntax", "-r./first.rb", "--help"],
    ),
    ("file-arguments", &["--help"]),
    ("env-arguments", &["--help"]),
    ("double-dash", &["-r./first.rb", "--SELECT", "--", "a.rb"]),
    ("dash-value", &["-r./first.rb", "--SELECT", "-", "--help"]),
    ("equals-empty", &["-r./first.rb", "--SELECT=", "--help"]),
];

struct Case {
    id: String,
    form: &'static str,
    args: Vec<String>,
    option: &'static str,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for option in ["only", "except"] {
        for &(form, args) in FORMS {
            cases.push(Case {
                id: format!("{option}-{form}"),
                form,
                args: args
                    .iter()
                    .map(|arg| arg.replace("SELECT", option))
                    .collect(),
                option,
            });
        }
        cases.push(Case {
            id: format!("{option}-mixed"),
            form: "mixed",
            args: vec![
                "-r./first.rb".into(),
                format!("--{option}"),
                format!("--{}", if option == "only" { "except" } else { "only" }),
                "--help".into(),
            ],
            option,
        });
    }
    for (form, args) in [
        (
            "after-double-dash",
            vec!["-r./first.rb", "--", "--only", "--help"],
        ),
        ("config-value", vec!["--config=--only", "--help"]),
    ] {
        cases.push(Case {
            id: form.into(),
            form,
            args: args.into_iter().map(str::to_owned).collect(),
            option: "only",
        });
    }
    assert_eq!(cases.len(), 68);
    cases
}

fn environment(command: &mut Command, root: &Path, force: bool) {
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
        "SONICOP_BINARY",
    ] {
        command.env_remove(key);
    }
    command
        .current_dir(root)
        .env("TERM", "dumb")
        .env("CLICOLOR_FORCE", if force { "1" } else { "0" })
        .env("RUBOCOP_TARGET_RUBY_VERSION", "2.7")
        .env("RUBOCOP_CACHE_ROOT", root.join("cache"))
        .env("XDG_CACHE_HOME", root.join("xdg"))
        .env("XDG_CONFIG_HOME", root.join("empty-config"));
}

fn ruby(root: &Path) -> Option<PathBuf> {
    let path =
        PathBuf::from(std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into()));
    let mut command = Command::new(&path);
    environment(&mut command, root, false);
    let output = command.args(["-e", LOADER]).output();
    if output.as_ref().is_ok_and(|output| output.status.success()) {
        Some(path)
    } else {
        assert_ne!(
            std::env::var("SONICOP_RUNTIME_TESTS").as_deref(),
            Ok("1"),
            "固定Ruby/RuboCopが必要です: {output:?}"
        );
        eprintln!("未計測: 実RubyがないためCSVの68条件比較は成功へ数えません");
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
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    report: Option<serde_json::Value>,
    markers: BTreeMap<String, Vec<u8>>,
}

fn transport(text: &str) -> Vec<u8> {
    if cfg!(windows) {
        text.replace('\n', "\r\n").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

fn capture(root: &Path, case: &Case, ruby: &Path, engine: &str) -> Capture {
    fs::create_dir(root).unwrap();
    let mut originals = BTreeMap::new();
    for (name, text) in [
        ("Gemfile", ""),
        ("a.rb", "x=1\n"),
        ("config.yml", BASE),
        ("report.json", "KEEP\n"),
        ("first.rb", FIRST),
        (
            "later.rb",
            "puts 'LATER_MARKER'\nFile.binwrite('later.loaded', \"later\\n\")\n",
        ),
        (
            "zero.rb",
            "puts 'ZERO_MARKER'\nFile.binwrite('zero.loaded', \"zero\\n\")\nexit 0\n",
        ),
        (
            "stop.rb",
            "puts 'STOP_MARKER'\nFile.binwrite('stop.loaded', \"stop\\n\")\nabort 'STOP'\n",
        ),
    ] {
        fs::write(root.join(name), text).unwrap();
        originals.insert(name.to_owned(), snapshot(&root.join(name)));
    }
    let option_arguments = format!("-r./first.rb --{}\n", case.option);
    if case.form == "file-arguments" {
        fs::write(root.join(".rubocop"), &option_arguments).unwrap();
        originals.insert(".rubocop".into(), snapshot(&root.join(".rubocop")));
    }
    let mut command = match engine {
        "reference" => {
            let mut command = Command::new(ruby);
            command.args([
                "-e",
                &format!("{LOADER} exit RuboCop::CLI.new.run(ARGV)"),
                "--",
            ]);
            command
        }
        "wrapper" => {
            let mut command = Command::new(ruby);
            command
                .arg("-I")
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("lib"))
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("exe/sonicop"));
            command
        }
        _ => Command::new(env!("CARGO_BIN_EXE_sonicop")),
    };
    environment(
        &mut command,
        root,
        matches!(case.form, "force-color" | "force-no-color"),
    );
    if case.form == "env-arguments" {
        command.env("RUBOCOP_OPTS", option_arguments.trim_end());
    }
    if engine == "wrapper" {
        command
            .env("SONICOP_BINARY", env!("CARGO_BIN_EXE_sonicop"))
            .env("SONICOP_RUNTIME_RUBY", ruby);
    } else if engine == "native" {
        if case
            .args
            .iter()
            .any(|arg| arg.starts_with("-r") || arg == "--require")
            || matches!(case.form, "file-arguments" | "env-arguments")
        {
            command.env("SONICOP_RUNTIME_RUBY", ruby);
        } else {
            command.env("PATH", "");
        }
    }
    let done = command
        .args([
            "--config",
            "config.yml",
            "--cache",
            "false",
            "--format",
            "json",
        ])
        .args(&case.args)
        .output()
        .unwrap();
    let help = matches!(
        case.form,
        "help-first"
            | "empty-control"
            | "valid-control"
            | "dash-value"
            | "equals-empty"
            | "config-value"
    );
    let status = if help || matches!(case.form, "zero-before" | "first-zero-before") {
        0
    } else if case.form == "stop-before" {
        1
    } else {
        2
    };
    assert_eq!(
        done.status.code(),
        Some(status),
        "{} / {engine}: {done:?}",
        case.id
    );
    for (name, original) in originals {
        assert_eq!(
            snapshot(&root.join(&name)),
            original,
            "{} / {engine}: 原本{name}",
            case.id
        );
    }
    let mut stdout = done.stdout;
    let mut report = None;
    if case.form == "after-double-dash" {
        let start = stdout.iter().position(|byte| *byte == b'{').unwrap();
        assert_eq!(stdout.last(), Some(&b'}'));
        let mut value: serde_json::Value = serde_json::from_slice(&stdout[start..]).unwrap();
        assert_eq!(value["metadata"]["rubocop_version"], "1.89.0");
        assert_eq!(value["summary"]["target_file_count"], 2);
        assert_eq!(value["summary"]["inspected_file_count"], 0);
        for key in [
            "sonicop_version",
            "ruby_engine",
            "ruby_version",
            "ruby_platform",
        ] {
            value["metadata"].as_object_mut().unwrap().remove(key);
        }
        report = Some(value);
        stdout.truncate(start);
    } else if help {
        let prefix = if case.form == "config-value" {
            Vec::new()
        } else {
            transport("FIRST_MARKER\n")
        };
        assert!(
            stdout.starts_with(&prefix),
            "{} / {engine}: {stdout:?}",
            case.id
        );
        let text = std::str::from_utf8(&stdout[prefix.len()..]).unwrap();
        // 製品固有のhelp本文は一致へ数えず、featureの境界とhelpへの到達を検査する。
        assert!(
            text.contains(if engine == "reference" {
                "Usage: rubocop"
            } else {
                "Usage: sonicop"
            }),
            "{text}"
        );
        assert!(!text.contains("_MARKER"));
        stdout = prefix;
    }
    let stderr = String::from_utf8(done.stderr)
        .unwrap()
        .replace(&root.to_string_lossy().replace('\\', "/"), "<ROOT>")
        .into_bytes();
    let markers = ["first", "later", "zero", "stop"]
        .into_iter()
        .filter_map(|name| {
            fs::read(root.join(format!("{name}.loaded")))
                .ok()
                .map(|bytes| (name.to_owned(), bytes))
        })
        .collect();
    Capture {
        stdout,
        stderr,
        report,
        markers,
    }
}

#[test]
fn missing_csv_callbacks_match_real_rubocop_in_native_and_wrapper() {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(not(windows))]
    let base = directory.path().canonicalize().unwrap();
    #[cfg(windows)]
    let base = directory.path().to_path_buf();
    let Some(ruby) = ruby(&base) else {
        return;
    };
    for case in cases() {
        let root = base.join(&case.id);
        fs::create_dir(&root).unwrap();
        // 本家の実出力を先に取り、Nativeとgem wrapperの両方を照合する。
        let reference = capture(&root.join("reference"), &case, &ruby, "reference");
        for engine in ["native", "wrapper"] {
            assert_eq!(
                capture(&root.join(engine), &case, &ruby, engine),
                reference,
                "{} / {engine}",
                case.id
            );
        }
    }
}

#[test]
fn missing_csv_before_later_features_keeps_the_ruby_free_path() {
    let directory = tempfile::tempdir().unwrap();
    for option in ["only", "except"] {
        for suffix in [vec![], vec!["--help"], vec!["-r./absent.rb", "--help"]] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
            environment(&mut command, directory.path(), false);
            let done = command
                .arg(format!("--{option}"))
                .args(suffix)
                .env("PATH", "")
                .env("SONICOP_RUNTIME_RUBY", "absent-ruby")
                .output()
                .unwrap();
            assert_eq!(done.status.code(), Some(2));
            assert!(done.stdout.is_empty());
            assert_eq!(
                done.stderr,
                transport(&format!("--{option} argument should be [COP1,COP2,...].\n"))
            );
        }
    }
}
