//! 実RuboCopの一覧bytesとmode境界を照合し、原本/属性/stdin payloadを保持する。

use std::fs;
use std::process::{Command, Stdio};

use serde_json::Value;

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn transport(raw: Vec<u8>) -> Vec<u8> {
    if cfg!(windows) {
        raw
    } else {
        // 参照のIO transportだけをhostへ投影し、Ruby原本とpayload期待は別に扱う。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

#[test]
fn disk_listing_and_unchanged_modes_match_the_real_cli() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/disk_listing.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(fixture["primary"]["ruby"], "4.0.7");
    assert_eq!(fixture["primary"]["gems"]["rubocop"], "1.89.0");
    assert_eq!(
        cases
            .iter()
            .map(|case| case["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "disk-one",
            "explicit-reverse",
            "unicode",
            "directory",
            "short-flag",
            "format-out",
            "mixed-formats-out",
            "stderr-option",
            "redirect-files",
            "readonly-correction-mode",
            "empty-directory",
            "invalid-cache",
            "json-control",
            "stdin-directory-list",
            "stdin-json-correction",
            "existing-out",
            "stdin-directory-stderr",
            "rejected-disk-stderr",
            "rejected-directory-stderr"
        ]
    );
    for case in cases {
        assert_eq!(case["coverage"], "full_bytes");
        let root = tempfile::tempdir().unwrap();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = root.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text.as_str().unwrap().as_bytes()).unwrap();
        }
        if let Some(directories) = case["directories"].as_array() {
            for name in directories {
                fs::create_dir_all(root.path().join(name.as_str().unwrap())).unwrap();
            }
        }
        let mut restored = Vec::new();
        if let Some(readonly) = case["readonly"].as_array() {
            for name in readonly {
                let path = root.path().join(name.as_str().unwrap());
                let original = fs::metadata(&path).unwrap().permissions();
                let mut permissions = original.clone();
                permissions.set_readonly(true);
                fs::set_permissions(&path, permissions).unwrap();
                restored.push((path, original));
            }
        }
        let before: Vec<_> = case["files"]
            .as_object()
            .unwrap()
            .keys()
            .map(|name| {
                let path = root.path().join(name);
                let metadata = fs::metadata(&path).unwrap();
                (
                    name.clone(),
                    fs::read(path).unwrap(),
                    metadata.permissions(),
                    metadata.modified().unwrap(),
                )
            })
            .collect();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(root.path())
            .env("CLICOLOR_FORCE", "0")
            .env("TERM", "dumb")
            .env("PATH", "");
        for key in [
            "NO_COLOR",
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(key);
        }
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        );
        command.stdin(Stdio::piped());
        if case["redirect"] == true {
            command.stdout(fs::File::create(root.path().join("captured.stdout")).unwrap());
            command.stderr(fs::File::create(root.path().join("captured.stderr")).unwrap());
        } else {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        let mut child = command.spawn().unwrap();
        {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&bytes(case["stdin_hex"].as_str().unwrap_or("")))
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let after: Vec<_> = before
            .iter()
            .map(|(name, _, _, _)| {
                let path = root.path().join(name);
                let metadata = fs::metadata(&path).unwrap();
                (
                    name.clone(),
                    fs::read(path).unwrap(),
                    metadata.permissions(),
                    metadata.modified().unwrap(),
                )
            })
            .collect();
        // assertion失敗時も専用tempdirを片付けられるよう、観測後に元属性だけへ戻す。
        for (path, permissions) in restored {
            fs::set_permissions(path, permissions).unwrap();
        }
        let id = case["id"].as_str().unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            case["exit"].as_i64(),
            "{id}"
        );
        let (stdout, stderr) = if case["redirect"] == true {
            (
                fs::read(root.path().join("captured.stdout")).unwrap(),
                fs::read(root.path().join("captured.stderr")).unwrap(),
            )
        } else {
            (output.stdout, output.stderr)
        };
        assert_eq!(
            stderr,
            transport(bytes(case["stderr_hex"].as_str().unwrap())),
            "{id}"
        );
        if case.get("report").is_some() {
            let mut stream = serde_json::Deserializer::from_slice(&stdout).into_iter::<Value>();
            let mut report = stream.next().unwrap().unwrap();
            let end = stream.byte_offset();
            assert_eq!(report["metadata"]["ruby_engine"], "sonicop", "{id}");
            assert_eq!(report["metadata"]["rubocop_version"], "1.89.0", "{id}");
            report.as_object_mut().unwrap().remove("metadata");
            assert_eq!(report, case["report"], "{id}");
            assert_eq!(
                &stdout[end..],
                transport(bytes(case["suffix_hex"].as_str().unwrap())),
                "{id}"
            );
        } else {
            assert_eq!(
                stdout,
                transport(bytes(case["stdout_hex"].as_str().unwrap())),
                "{id}"
            );
        }
        for (old, new) in before.iter().zip(after.iter()) {
            assert_eq!(old, new, "{id}/{}", old.0);
        }
        for (name, value) in case["outputs"].as_object().unwrap() {
            let path = root.path().join(name);
            if let Some(hex) = value.as_str() {
                assert_eq!(fs::read(path).unwrap(), bytes(hex), "{id}/{name}");
            } else {
                assert!(
                    matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                    "{id}/{name}"
                );
            }
        }
    }
}
