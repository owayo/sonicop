//! I/Oの実先頭行とMissing/通常reportを分け、原本・既存report・directory属性を保つ。

#![cfg(windows)]

use std::fs;
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn bytes(value: &Value) -> Vec<u8> {
    value
        .as_str()
        .unwrap()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn long_name(path: &Path) -> String {
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    let input: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut output = vec![0; 32_768];
    let length =
        unsafe { GetLongPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
    assert!(length > 0 && length < output.len() as u32);
    String::from_utf16(&output[..length as usize]).unwrap()
}

#[test]
fn ten_actual_io_conditions_keep_their_kind_headline_reports_and_sources() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/config_io_error_headers.json")).unwrap();
    let rows = fixture["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 10);
    let mut observations = 0;
    for row in rows {
        let directory = tempfile::tempdir().unwrap();
        let cwd = long_name(directory.path());
        let bind = |text: &str| {
            text.replace("<CWD_FWD>", &cwd.replace('\\', "/"))
                .replace("<CWD>", &cwd)
        };
        let cwd = Path::new(&cwd);
        let mut originals = Vec::new();
        for (name, body) in row["files"].as_object().unwrap() {
            let path = cwd.join(name);
            let body = bind(body.as_str().unwrap()).into_bytes();
            fs::write(&path, &body).unwrap();
            let info = fs::metadata(&path).unwrap();
            originals.push((path, body, info.modified().unwrap(), info.permissions()));
        }
        let mut directories = Vec::new();
        for name in row["directories"].as_array().unwrap() {
            let path = cwd.join(name.as_str().unwrap());
            fs::create_dir(&path).unwrap();
            let info = fs::metadata(&path).unwrap();
            directories.push((path, info.modified().unwrap(), info.permissions()));
        }
        let guard = row["locked_file"].as_str().map(|name| {
            fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(cwd.join(name))
                .unwrap()
        });
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(cwd)
            .env("PATH", "")
            .env("TERM", "dumb")
            .env("XDG_CACHE_HOME", cwd.join("xdg-cache"))
            .env("XDG_CONFIG_HOME", cwd.join("empty-config"));
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
        for (name, value) in row["environment"].as_object().unwrap() {
            command.env(name, value.as_str().unwrap());
        }
        for arg in row["args"].as_array().unwrap() {
            command.arg(bind(arg.as_str().unwrap()));
        }
        let output = command.output().unwrap();
        drop(guard);
        let id = row["id"].as_str().unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            row["exit"].as_i64(),
            "{id}: {output:?}"
        );
        assert_eq!(output.stdout, bytes(&row["stdout_hex"]), "{id}");
        // Ruby内部13frameは参照へ残す。Nativeの比較は実測した先頭行の全bytes。
        let expected = if row["comparison"] == "system_headline" {
            &row["first_line_template"]
        } else {
            &row["stderr_template"]
        };
        assert_eq!(
            output.stderr,
            bind(expected.as_str().unwrap()).as_bytes(),
            "{id}"
        );
        for (path, body, mtime, permissions) in originals {
            assert_eq!(fs::read(&path).unwrap(), body, "{id}");
            let after = fs::metadata(path).unwrap();
            assert_eq!(after.modified().unwrap(), mtime, "{id}");
            assert_eq!(after.permissions(), permissions, "{id}");
            observations += 1;
        }
        for (path, mtime, permissions) in directories {
            let after = fs::metadata(path).unwrap();
            assert_eq!(after.modified().unwrap(), mtime, "{id}");
            assert_eq!(after.permissions(), permissions, "{id}");
        }
        let out = fs::read(cwd.join("out.json")).unwrap();
        if row["comparison"] == "report" {
            let mut actual: Value = serde_json::from_slice(&out).unwrap();
            let mut expected: Value = serde_json::from_slice(&bytes(&row["out_hex"])).unwrap();
            for key in fixture["metadata_identity_keys"].as_array().unwrap() {
                let key = key.as_str().unwrap();
                actual["metadata"].as_object_mut().unwrap().remove(key);
                expected["metadata"].as_object_mut().unwrap().remove(key);
            }
            assert_eq!(actual, expected, "{id}");
        } else {
            assert_eq!(out, bytes(&row["out_hex"]), "{id}");
        }
    }
    assert_eq!(observations, 49);
}
