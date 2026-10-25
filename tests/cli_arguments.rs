#![cfg(any(unix, windows))]

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

#[cfg(unix)]
fn non_unicode_name() -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(b"raw-\xff.rb".to_vec())
}

#[cfg(windows)]
fn non_unicode_name() -> OsString {
    use std::os::windows::ffi::OsStringExt;
    OsString::from_wide(&[114, 97, 119, 45, 0xd800, 46, 114, 98])
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(root)
        .env_remove("RUBOCOP_OPTS")
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION");
    command
}

fn assert_rejected(output: &Output, index: usize) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr.clone()).unwrap().trim(),
        format!("Error: command-line argument {index} is not valid Unicode")
    );
}

fn recorded_non_unicode_fixture() -> serde_json::Value {
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let scope = &manifest["non_unicode_cli_arguments"];
    assert_eq!(scope["issue"].as_u64(), Some(100));
    assert_eq!(
        scope["fixture"].as_str(),
        Some("tests/fixtures/non_unicode_cli_arguments.json")
    );
    assert_eq!(scope["regression"].as_str(), Some("tests/cli_arguments.rs"));
    assert_eq!(
        scope["counts_as_compatibility_match"].as_bool(),
        Some(false)
    );
    assert_eq!(scope["complete_os_path_support"].as_bool(), Some(false));
    assert_eq!(scope["measured_platforms"].as_sequence().unwrap().len(), 1);
    assert_eq!(scope["measured_platforms"][0].as_str(), Some("windows"));
    assert_eq!(
        scope["unmeasured_reference_platforms"]
            .as_sequence()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        scope["unmeasured_reference_platforms"][0].as_str(),
        Some("unix")
    );
    serde_json::from_str(include_str!("fixtures/non_unicode_cli_arguments.json")).unwrap()
}

#[test]
fn non_unicode_manifest_stays_within_the_measured_argument_boundary() {
    use std::collections::HashSet;

    let fixture = recorded_non_unicode_fixture();
    assert_eq!(fixture["issue"], 100);
    assert_eq!(fixture["measured_platform"], "windows");
    assert_eq!(
        fixture["unmeasured_reference_platforms"],
        serde_json::json!(["unix"])
    );
    assert_eq!(fixture["reference"]["rubocop"], "1.89.0");
    assert_eq!(fixture["reference"]["ruby"], "4.0.7");
    assert_eq!(fixture["reference"]["parser"], "3.3.12.0");
    let mut expected = HashSet::new();
    for spelling in ["ordinary", "extended"] {
        for present in [false, true] {
            for mode in ["inspect", "safe", "unsafe"] {
                expected.insert(format!("path/{spelling}/{present}/{mode}"));
            }
        }
    }
    for mode in ["inspect", "safe", "unsafe"] {
        expected.insert(format!("argv0/ordinary/false/{mode}"));
    }
    let mut actual = HashSet::new();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 15);
    for case in cases {
        let location = case["location"].as_str().unwrap();
        let spelling = case["spelling"].as_str().unwrap();
        let present = case["replacement_present"].as_bool().unwrap();
        let mode = case["mode"].as_str().unwrap();
        assert!(actual.insert(format!("{location}/{spelling}/{present}/{mode}")));
        let filename = if location == "path" {
            vec![114, 97, 119, 45, 0xd800, 46, 114, 98]
        } else {
            vec![114, 97, 119, 45, 0xd800, 46, 101, 120, 101]
        };
        assert_eq!(
            case["argument_filename_utf16_code_units"],
            serde_json::json!(filename)
        );
        assert_eq!(case["raw_before_hex"], "783d310d0a");
        let before = if present {
            serde_json::json!("793d320d0a")
        } else {
            serde_json::Value::Null
        };
        assert_eq!(case["replacement_before_hex"], before);
        assert_eq!(case["sonicop"]["exit"], 2);
        assert_eq!(case["sonicop"]["stdout"], "");
        assert_eq!(case["sonicop"]["raw_after_hex"], case["raw_before_hex"]);
        assert_eq!(case["sonicop"]["replacement_after_hex"], before);
        let index = if location == "argv0" {
            0
        } else if mode == "inspect" {
            9
        } else {
            10
        };
        assert_eq!(
            case["sonicop"]["stderr"],
            format!("Error: command-line argument {index} is not valid Unicode\n")
        );
        let reference_exit = if location == "path" && !present {
            2
        } else if mode == "inspect" {
            1
        } else {
            0
        };
        assert_eq!(case["reference"]["exit"], reference_exit);
        let raw_after = if location == "argv0" && mode != "inspect" {
            "78203d20310d0a"
        } else {
            "783d310d0a"
        };
        assert_eq!(case["reference"]["raw_after_hex"], raw_after);
        let replacement_after = if present && mode != "inspect" {
            serde_json::json!("79203d20320d0a")
        } else {
            before
        };
        assert_eq!(
            case["reference"]["replacement_after_hex"],
            replacement_after
        );
    }
    assert_eq!(actual, expected);
}

#[cfg(windows)]
#[test]
fn recorded_non_unicode_rejections_preserve_every_source_byte() {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;

    let fixture = recorded_non_unicode_fixture();
    let decode = |hex: &str| {
        (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect::<Vec<_>>()
    };
    for case in fixture["cases"].as_array().unwrap() {
        let directory =
            tempfile::tempdir_in(Path::new(env!("CARGO_BIN_EXE_sonicop")).parent().unwrap())
                .unwrap();
        let root = directory.path();
        let units: Vec<_> = case["argument_filename_utf16_code_units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|unit| u16::try_from(unit.as_u64().unwrap()).unwrap())
            .collect();
        let filename = OsString::from_wide(&units);
        let argv_zero = case["location"] == "argv0";
        let raw = root.join(if argv_zero {
            OsString::from("valid.rb")
        } else {
            filename.clone()
        });
        let replacement = root.join("raw-\u{fffd}.rb");
        fs::write(&raw, decode(case["raw_before_hex"].as_str().unwrap())).unwrap();
        if let Some(hex) = case["replacement_before_hex"].as_str() {
            fs::write(&replacement, decode(hex)).unwrap();
        }
        fs::write(
            root.join("config.yml"),
            b"AllCops:\n  NewCops: disable\n  TargetRubyVersion: 3.3\n",
        )
        .unwrap();
        let mut invocation = if argv_zero {
            let alias = root.join(filename);
            fs::hard_link(env!("CARGO_BIN_EXE_sonicop"), &alias).unwrap();
            Command::new(alias)
        } else {
            command(root)
        };
        invocation
            .current_dir(root)
            .env_remove("RUBOCOP_OPTS")
            .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
            .args([
                "--config",
                "config.yml",
                "--cache",
                "false",
                "--only",
                "Layout/SpaceAroundOperators",
                "--format",
                "json",
            ]);
        match case["mode"].as_str().unwrap() {
            "safe" => {
                invocation.arg("-a");
            }
            "unsafe" => {
                invocation.arg("-A");
            }
            "inspect" => {}
            other => panic!("unknown recorded mode: {other}"),
        }
        let argument = if case["spelling"] == "extended" {
            fs::canonicalize(&raw).unwrap()
        } else {
            let mut ordinary: Vec<_> = raw.as_os_str().encode_wide().collect();
            if ordinary.starts_with(&[92, 92, 63, 92]) {
                ordinary.drain(..4);
            }
            PathBuf::from(OsString::from_wide(&ordinary))
        };
        let output = invocation.arg(argument).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(case["sonicop"]["exit"].as_i64().unwrap() as i32)
        );
        assert_eq!(
            output.stdout,
            case["sonicop"]["stdout"].as_str().unwrap().as_bytes()
        );
        // Windows のテキスト行末の別差を、この拒否契約へ混ぜない。
        assert_eq!(
            String::from_utf8(output.stderr)
                .unwrap()
                .trim_end_matches(['\r', '\n']),
            case["sonicop"]["stderr"]
                .as_str()
                .unwrap()
                .trim_end_matches(['\r', '\n'])
        );
        assert_eq!(
            fs::read(&raw).unwrap(),
            decode(case["sonicop"]["raw_after_hex"].as_str().unwrap())
        );
        if let Some(hex) = case["sonicop"]["replacement_after_hex"].as_str() {
            assert_eq!(fs::read(&replacement).unwrap(), decode(hex));
        } else {
            assert!(!replacement.exists());
        }
    }
}

#[test]
fn non_unicode_paths_fail_without_touching_either_name() {
    let directory = tempfile::tempdir().unwrap();
    let raw = directory.path().join(non_unicode_name());
    let replacement = directory.path().join("raw-\u{fffd}.rb");
    let valid = directory.path().join("valid.rb");
    // 本家 Windows Ruby は U+FFFD の別名を読む。こちらはその別ファイルにも書かない。
    for extended in 0..if cfg!(windows) { 2 } else { 1 } {
        for mode in [None, Some("-a"), Some("-A")] {
            let raw_exists = match fs::write(&raw, b"x=1\r\n") {
                Ok(()) => true,
                // macOS のテスト volume は非 UTF-8 名を EILSEQ で拒む。
                // その場合も引数自体と、既存の有効な別名を検査する。
                #[cfg(target_os = "macos")]
                Err(error) if error.raw_os_error() == Some(92) => false,
                Err(error) => panic!("failed to create non-Unicode fixture: {error}"),
            };
            fs::write(&replacement, b"y=2\r\n").unwrap();
            fs::write(&valid, b"z=3\r\n").unwrap();
            let argument = if extended != 0 {
                fs::canonicalize(&raw).unwrap()
            } else {
                raw.clone()
            };
            let mut invocation = command(directory.path());
            invocation.args([
                "--force-default-config",
                "--cache",
                "false",
                "--only",
                "Layout/SpaceAroundOperators",
                "--format",
                "json",
            ]);
            if let Some(mode) = mode {
                invocation.arg(mode);
            }
            let output = invocation.arg(&valid).arg(argument).output().unwrap();
            assert_rejected(&output, 9 + usize::from(mode.is_some()));
            if raw_exists {
                assert_eq!(fs::read(&raw).unwrap(), b"x=1\r\n");
            } else {
                assert!(!raw.exists());
            }
            assert_eq!(fs::read(&replacement).unwrap(), b"y=2\r\n");
            assert_eq!(fs::read(&valid).unwrap(), b"z=3\r\n");
        }
    }
}

#[test]
fn non_unicode_argv_zero_is_an_argument_error() {
    #[cfg(windows)]
    let directory =
        tempfile::tempdir_in(Path::new(env!("CARGO_BIN_EXE_sonicop")).parent().unwrap()).unwrap();
    #[cfg(unix)]
    let directory = tempfile::tempdir().unwrap();

    #[cfg(windows)]
    let mut invocation = {
        // 同じ volume の別名で起動し、argv[0] にも実際の非 Unicode 名を渡す。
        let mut alias = directory.path().join(non_unicode_name());
        alias.set_extension("exe");
        fs::hard_link(env!("CARGO_BIN_EXE_sonicop"), &alias).unwrap();
        let mut command = Command::new(alias);
        command.current_dir(directory.path());
        command
    };
    #[cfg(unix)]
    let mut invocation = {
        use std::os::unix::process::CommandExt;
        let mut command = command(directory.path());
        command.arg0(non_unicode_name());
        command
    };
    let path = directory.path().join("valid.rb");
    fs::write(&path, b"x=1\r\n").unwrap();
    let output = invocation.arg("-A").arg(&path).output().unwrap();
    assert_rejected(&output, 0);
    assert_eq!(fs::read(path).unwrap(), b"x=1\r\n");
}

#[test]
fn unicode_arguments_keep_dotfile_environment_command_line_order() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = if cfg!(windows) {
        b"x=1\r\n".as_slice()
    } else {
        b"x=1\n".as_slice()
    };
    fs::write(root.join("対象 日本😀.rb"), source).unwrap();
    // env の --out は dotfile の formatter へ、CLI の --out は env の formatter へ結び付く。
    fs::write(
        root.join(".rubocop"),
        "--force-default-config --cache false --only Layout/SpaceAroundOperators --format json\n",
    )
    .unwrap();
    let output = command(root)
        .env(
            "RUBOCOP_OPTS",
            "--out 'environment 日本.json' --format simple",
        )
        .args(["--out", "command 日本.txt", "対象 日本😀.rb"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("environment 日本.json")).unwrap()).unwrap();
    assert_eq!(json["summary"]["offense_count"], 1);
    assert_eq!(json["files"][0]["path"], "対象 日本😀.rb");
    let simple = fs::read_to_string(root.join("command 日本.txt")).unwrap();
    assert!(simple.contains("Layout/SpaceAroundOperators"));
    assert_eq!(fs::read(root.join("対象 日本😀.rb")).unwrap(), source);
}

#[test]
fn non_unicode_environment_options_keep_the_existing_error_contract() {
    let directory = tempfile::tempdir().unwrap();
    let output = command(directory.path())
        .env("RUBOCOP_OPTS", non_unicode_name())
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "Error: RUBOCOP_OPTS is not valid UTF-8"
    );
}
