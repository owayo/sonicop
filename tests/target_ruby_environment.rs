//! ENVの実getter値と、parser拒否後のfinished reportを早期終了・encoding診断と分ける。
#![cfg(windows)]

use std::{fs, path::Path, process::Command};

use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/target_ruby_environment.json")).unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap();
    assert_eq!(text.len() % 2, 0);
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn long_name(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    let input: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut output = vec![0; 32_768];
    let length =
        unsafe { GetLongPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
    assert!(length > 0 && length < output.len() as u32);
    String::from_utf16(&output[..length as usize]).unwrap()
}

fn bind(text: &str, root: &Path) -> String {
    text.replace("<CWD_FWD>", &root.to_string_lossy().replace('\\', "/"))
        .replace("<CWD>", &root.to_string_lossy())
}

fn environment(command: &mut Command, root: &Path, row: &Value) {
    command
        .current_dir(root)
        .env("PATH", "")
        .env("TERM", "dumb")
        .env("XDG_CACHE_HOME", root.join("xdg"))
        .env("XDG_CONFIG_HOME", root.join("empty-config"));
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
        "NO_COLOR",
        "RUBOCOP_CACHE_ROOT",
        "SONICOP_RUNTIME_RUBY",
    ] {
        command.env_remove(key);
    }
    for (key, value) in row["environment"].as_object().unwrap() {
        command.env(key, value.as_str().unwrap());
    }
}

fn prepare(
    root: &Path,
    row: &Value,
) -> Vec<(
    std::path::PathBuf,
    Vec<u8>,
    std::time::SystemTime,
    fs::Permissions,
)> {
    row["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, text)| {
            let path = root.join(name);
            let body = text.as_str().unwrap().as_bytes().to_vec();
            fs::write(&path, &body).unwrap();
            let info = fs::metadata(&path).unwrap();
            (path, body, info.modified().unwrap(), info.permissions())
        })
        .collect()
}

#[test]
fn twenty_six_actual_environment_conditions_keep_phase_reports_and_originals() {
    use std::io::Write;
    use std::process::Stdio;
    let data = fixture();
    let rows = data["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 26);
    let mut preserved = 0;
    for row in rows {
        let directory = tempfile::tempdir().unwrap();
        let root = long_name(directory.path());
        let root = Path::new(&root);
        let originals = prepare(root, row);
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        environment(&mut command, root, row);
        command.args(
            row["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap()),
        );
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&bytes(&row["stdin_hex"]))
            .unwrap();
        let actual = child.wait_with_output().unwrap();
        let id = row["id"].as_str().unwrap();
        assert_eq!(
            actual.status.code().map(i64::from),
            row["exit"].as_i64(),
            "{id}: {actual:?}"
        );
        let expected_stdout = if row["comparison"] == "version_identity" {
            assert_eq!(bytes(&row["stdout_hex"]), b"1.89.0\r\n");
            format!("{}\r\n", env!("CARGO_PKG_VERSION")).into_bytes()
        } else {
            bytes(&row["stdout_hex"])
        };
        assert_eq!(actual.stdout, expected_stdout, "{id}");
        let stderr = if row["comparison"] == "parser_headline" {
            &row["first_line_template"]
        } else {
            &row["stderr_template"]
        };
        assert_eq!(
            actual.stderr,
            bind(stderr.as_str().unwrap(), root).as_bytes(),
            "{id}"
        );
        let output = fs::read(root.join("report.json")).unwrap();
        let expected = bytes(&row["report_hex"]);
        if row["report_preserved"] == true {
            assert_eq!(output, expected, "{id}");
        } else {
            let mut output: Value = serde_json::from_slice(&output).unwrap();
            let mut expected: Value = serde_json::from_slice(&expected).unwrap();
            for key in data["metadata_identity_keys"].as_array().unwrap() {
                let key = key.as_str().unwrap();
                output["metadata"].as_object_mut().unwrap().remove(key);
                expected["metadata"].as_object_mut().unwrap().remove(key);
            }
            assert_eq!(output, expected, "{id}");
        }
        for (path, body, mtime, mode) in originals {
            if path.file_name().unwrap() != "report.json" || row["report_preserved"] == true {
                assert_eq!(fs::read(&path).unwrap(), body, "{id}");
                let info = fs::metadata(path).unwrap();
                assert_eq!(info.modified().unwrap(), mtime, "{id}");
                assert_eq!(info.permissions(), mode, "{id}");
                preserved += 1;
            }
        }
    }
    assert_eq!(preserved, 81);
}

#[test]
fn quiet_api_getters_are_checked_in_isolated_processes() {
    for (index, row) in fixture()["cases"].as_array().unwrap().iter().enumerate() {
        let directory = tempfile::tempdir().unwrap();
        let root = long_name(directory.path());
        let root = Path::new(&root);
        let originals = prepare(root, row);
        let mut command = Command::new(std::env::current_exe().unwrap());
        environment(&mut command, root, row);
        let actual = command
            .args(["--exact", "quiet_api_child", "--nocapture"])
            .env("SONICOP_TEST_TARGET_ENV_ROW", index.to_string())
            .output()
            .unwrap();
        assert!(actual.status.success(), "{}: {actual:?}", row["id"]);
        for (path, body, mtime, mode) in originals {
            assert_eq!(fs::read(&path).unwrap(), body);
            let info = fs::metadata(path).unwrap();
            assert_eq!(info.modified().unwrap(), mtime);
            assert_eq!(info.permissions(), mode);
        }
    }
}

#[test]
fn quiet_api_child() {
    let Ok(index) = std::env::var("SONICOP_TEST_TARGET_ENV_ROW") else {
        return;
    };
    let data = fixture();
    let row = &data["cases"][index.parse::<usize>().unwrap()];
    let cwd = std::env::current_dir().unwrap();
    let config = Path::new(row["args"][1].as_str().unwrap());
    let loaded = sonicop::config::Config::load(Some(config), &cwd);
    if row["api"]["kind"] == "value" {
        assert_eq!(
            loaded.unwrap().target_ruby_version().to_string(),
            row["api"]["target_inspect"].as_str().unwrap()
        );
    } else {
        // Rustのloadは探索と既定値mergeも行う。MissingのAPI本文はCLI互換へ数えない。
        assert!(loaded.is_err());
    }
}
