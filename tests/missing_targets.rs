//! 実 RuboCop と、不存在対象を含む検査の終了 report・補正順・物理バイトを比較する。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use serde_json::Value;

const LOADER: &str = "gem 'psych','5.3.1'; gem 'parser','3.3.12.0'; gem 'prism','1.8.1'; gem 'rubocop-ast','1.50.0'; gem 'rubocop','1.89.0'; gem 'rainbow','3.1.1'; require 'rubocop';";
const BASE: &str = "AllCops:\n  NewCops: disable\n  TargetRubyVersion: 2.7\n  SuggestExtensions: false\n  DisabledByDefault: true\n";
const FEATURE: &str = "File.binwrite('loaded_marker', \"loaded\\n\")\nat_exit { STDERR.puts(\"leave:#{$!.is_a?(SystemExit) ? $!.status : 'nil'}\") }\n";

struct Case {
    id: &'static str,
    args: &'static [&'static str],
    status: i32,
    targets: usize,
    inspected: usize,
}

const CASES: &[Case] = &[
    Case {
        id: "missing-dir",
        args: &["absent-dir"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "report",
        args: &["--out", "report.json", "absent-dir"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "missing-ruby",
        args: &["absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "unicode",
        args: &["猫/absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "cleaned",
        args: &["folder/../absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "absolute",
        args: &["<ABSENT>"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "duplicate",
        args: &["absent.rb", "<ABSENT>"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "listing",
        args: &["--list-target-files", "absent-dir", "a.rb"],
        status: 0,
        targets: 0,
        inspected: 0,
    },
    Case {
        id: "recognized-unknown",
        args: &["--only-recognized-file-types", "absent-dir"],
        status: 0,
        targets: 0,
        inspected: 0,
    },
    Case {
        id: "recognized-ruby",
        args: &["--only-recognized-file-types", "absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "force-excluded",
        args: &["--force-exclusion", "absent.rb"],
        status: 0,
        targets: 0,
        inspected: 0,
    },
    Case {
        id: "force-unrecognized",
        args: &["--force-exclusion", "absent-dir"],
        status: 0,
        targets: 0,
        inspected: 0,
    },
    Case {
        id: "prefix",
        args: &["a.rb", "absent.rb", "b.rb"],
        status: 2,
        targets: 3,
        inspected: 1,
    },
    Case {
        id: "correct-prefix",
        args: &["-A", "a.rb", "absent.rb", "b.rb"],
        status: 2,
        targets: 3,
        inspected: 1,
    },
    Case {
        id: "correct-prefix-crlf",
        args: &["-A", "a.rb", "absent.rb", "b.rb"],
        status: 2,
        targets: 3,
        inspected: 1,
    },
    Case {
        id: "correct-leading",
        args: &["-A", "absent.rb", "a.rb"],
        status: 2,
        targets: 2,
        inspected: 0,
    },
    Case {
        id: "parallel",
        args: &["--parallel", "a.rb", "absent.rb", "b.rb"],
        status: 2,
        targets: 3,
        inspected: 1,
    },
    Case {
        id: "stdin",
        args: &["--stdin", "absent.rb"],
        status: 0,
        targets: 1,
        inspected: 1,
    },
    Case {
        id: "color",
        args: &["--color", "absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "force-color",
        args: &["absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "no-color",
        args: &["--no-color", "absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "progress",
        args: &["absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "stderr-out",
        args: &["--stderr", "--out", "report.json", "absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
    Case {
        id: "required",
        args: &["--require", "./feature.rb", "absent.rb"],
        status: 2,
        targets: 1,
        inspected: 0,
    },
];

fn environment(command: &mut Command, root: &Path, force: bool) {
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "RUBOCOP_CACHE_ROOT",
        "NO_COLOR",
        "SONICOP_RUNTIME_RUBY",
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
            "固定 Ruby/RuboCop が必要です: {output:?}"
        );
        eprintln!("未計測: 実 Ruby がないため上流比較と require 条件は成功へ数えません");
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

struct Capture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    json: Option<Value>,
    files: BTreeMap<String, Vec<u8>>,
    marker: Option<Vec<u8>>,
}

fn transport(text: &str) -> Vec<u8> {
    if cfg!(windows) {
        text.replace('\n', "\r\n").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

fn normalize_report(bytes: &[u8], case: &Case) -> Value {
    // compact JSON の輸送へ終端改行を足すと、本家の File/stream と一致しなくなる。
    assert_eq!(bytes.first(), Some(&b'{'), "{}", case.id);
    assert_eq!(bytes.last(), Some(&b'}'), "{}", case.id);
    assert!(!bytes.contains(&b'\n'), "{}", case.id);
    let mut value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(value["metadata"]["rubocop_version"], "1.89.0");
    assert_eq!(
        value["summary"]["target_file_count"], case.targets,
        "{}",
        case.id
    );
    assert_eq!(
        value["summary"]["inspected_file_count"], case.inspected,
        "{}",
        case.id
    );
    for key in [
        "sonicop_version",
        "ruby_engine",
        "ruby_version",
        "ruby_platform",
    ] {
        value["metadata"].as_object_mut().unwrap().remove(key);
    }
    value
}

fn capture(root: &Path, case: &Case, reference: bool, ruby: Option<&Path>) -> Capture {
    fs::create_dir(root).unwrap();
    let mut config = BASE.to_owned();
    if case.id == "force-excluded" {
        config.push_str("  Exclude: ['absent.rb']\n");
    }
    if case.id.contains("prefix") || ["correct-leading", "parallel"].contains(&case.id) {
        config.push_str("Layout/SpaceAroundOperators:\n  Enabled: true\n");
    }
    let mut originals = BTreeMap::new();
    for (name, text) in [
        ("Gemfile", ""),
        ("a.rb", "x=1\n"),
        ("b.rb", "y=2\n"),
        ("config.yml", config.as_str()),
        ("report.json", "KEEP\n"),
        ("feature.rb", FEATURE),
    ] {
        let bytes = if case.id.ends_with("crlf") && matches!(name, "a.rb" | "b.rb") {
            text.replace('\n', "\r\n").into_bytes()
        } else {
            text.as_bytes().to_vec()
        };
        let path = root.join(name);
        fs::write(&path, bytes).unwrap();
        originals.insert(name, snapshot(&path));
    }
    let mut command = if reference {
        let mut command = Command::new(ruby.unwrap());
        command.args([
            "-e",
            &format!("{LOADER} exit RuboCop::CLI.new.run(ARGV)"),
            "--",
        ]);
        command
    } else {
        Command::new(env!("CARGO_BIN_EXE_sonicop"))
    };
    environment(
        &mut command,
        root,
        ["force-color", "no-color"].contains(&case.id),
    );
    if !reference {
        if case.id == "required" {
            command.env("SONICOP_RUNTIME_RUBY", ruby.unwrap());
        } else {
            command.env("PATH", "");
        }
    }
    command.args(["--config", "config.yml", "--cache", "false"]);
    if case.id != "progress" {
        command.args(["--format", "json"]);
    }
    for arg in case.args {
        if *arg == "<ABSENT>" {
            command.arg(root.join("absent.rb"));
        } else {
            command.arg(arg);
        }
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if case.id == "stdin" {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    if case.id == "stdin" {
        child.stdin.take().unwrap().write_all(b"x=1\n").unwrap();
    }
    let done = child.wait_with_output().unwrap();
    assert_eq!(
        done.status.code(),
        Some(case.status),
        "{}: {}",
        case.id,
        String::from_utf8_lossy(&done.stderr)
    );
    let out = case.args.contains(&"--out");
    let mut files = BTreeMap::new();
    for (name, before) in originals {
        let after = snapshot(&root.join(name));
        let may_correct = name == "a.rb" && case.id.starts_with("correct-prefix");
        if !(may_correct || (name == "report.json" && out)) {
            assert_eq!(after, before, "{}: 原本 {name}", case.id);
        }
        assert_eq!(after.mode, before.mode, "{}: mode {name}", case.id);
        files.insert(name.to_owned(), after.bytes);
    }
    let mut stdout = done.stdout;
    let report = if case.id == "listing" || case.id == "progress" {
        None
    } else if out {
        Some(normalize_report(&files["report.json"], case))
    } else {
        let start = stdout.iter().position(|byte| *byte == b'{').unwrap();
        let report = normalize_report(&stdout[start..], case);
        stdout.truncate(start);
        Some(report)
    };
    if out {
        files.remove("report.json");
    }
    let root_text = root.to_string_lossy().replace('\\', "/");
    let stderr = String::from_utf8(done.stderr)
        .unwrap()
        .replace(&root_text, "<ROOT>")
        .into_bytes();
    let marker = fs::read(root.join("loaded_marker")).ok();
    if case.status == 2 {
        let target = if ["missing-dir", "report"].contains(&case.id) {
            "absent-dir"
        } else if case.id == "unicode" {
            "猫/absent.rb"
        } else {
            "absent.rb"
        };
        let message = format!("Error: No such file or directory: <ROOT>/{target}");
        let message = if ["color", "force-color"].contains(&case.id) {
            format!("\x1b[31m{message}\x1b[0m\n")
        } else {
            format!("{message}\n")
        };
        let message = if case.id == "required" {
            message + "leave:2\n"
        } else {
            message
        };
        assert_eq!(stderr, transport(&message), "{}", case.id);
    } else {
        assert!(
            stderr.is_empty(),
            "{}: {:?}",
            case.id,
            String::from_utf8_lossy(&stderr)
        );
    }
    if case.id == "required" {
        assert_eq!(marker.as_deref(), Some(b"loaded\n".as_slice()));
    } else {
        assert!(marker.is_none());
    }
    if case.id == "listing" {
        assert_eq!(stdout, transport("absent-dir\na.rb\n"));
    }
    if case.id == "parallel" {
        assert_eq!(
            stdout,
            transport(
                "-P/--parallel is being ignored because it is not compatible with --cache false.\n"
            )
        );
    }
    Capture {
        stdout,
        stderr,
        json: report,
        files,
        marker,
    }
}

#[test]
fn missing_targets_finish_reports_and_keep_the_inspected_prefix() {
    let directory = tempfile::tempdir().unwrap();
    // macOS の /var alias を製品のパス差へ混ぜず、通常の実 cwd で両者を起動する。
    #[cfg(not(windows))]
    let base = directory.path().canonicalize().unwrap();
    #[cfg(windows)]
    let base = directory.path().to_path_buf();
    let ruby = ruby(&base);
    for case in CASES {
        if case.id == "required" && ruby.is_none() {
            continue;
        }
        let root = base.join(case.id);
        fs::create_dir(&root).unwrap();
        // 本家を先に動かし、その実出力を native の期待値にする。
        let reference = ruby
            .as_ref()
            .map(|ruby| capture(&root.join("reference"), case, true, Some(ruby)));
        let actual = capture(&root.join("native"), case, false, ruby.as_deref());
        if let Some(mut reference) = reference {
            assert_eq!(actual.stdout, reference.stdout, "{}: stdout", case.id);
            assert_eq!(actual.stderr, reference.stderr, "{}: stderr", case.id);
            assert_eq!(actual.json, reference.json, "{}: 全 report", case.id);
            if case.id == "correct-prefix-crlf" {
                // 本家のPOSIX書戻しはLFになるが、原本のCRLF保持は既存の意図的差分。
                // この補正済みfileだけを扱い、後続の未検査fileやstreamは正規化しない。
                let reference_bytes: &[u8] = if cfg!(windows) {
                    b"x = 1\r\n"
                } else {
                    b"x = 1\n"
                };
                assert_eq!(reference.files["a.rb"], reference_bytes);
                assert_eq!(actual.files["a.rb"], b"x = 1\r\n");
                reference
                    .files
                    .insert("a.rb".to_owned(), b"x = 1\r\n".to_vec());
            }
            assert_eq!(
                actual.files, reference.files,
                "{}: 補正/原本 bytes",
                case.id
            );
            assert_eq!(actual.marker, reference.marker, "{}: require", case.id);
        }
    }
}
