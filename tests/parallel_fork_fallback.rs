//! Windows fork通知の回数・stream順と、開始前/単一対象の陰性を実本家へ照合する。

use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

use serde_json::Value;

const NOTICE: &[u8] = b"parallel: Process.fork is not supported by this Ruby\r\n";

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap().as_bytes();
    let (pairs, rest) = text.as_chunks::<2>();
    assert!(rest.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn host(raw: Vec<u8>) -> Vec<u8> {
    if cfg!(windows) {
        raw
    } else {
        // Windows参照の非通知条件だけをhostの物理report改行へ合わせる。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn without_metadata(data: &[u8], engine: &str) -> Vec<u8> {
    let marker = b"\"metadata\":{";
    let Some(start) = data.windows(marker.len()).position(|part| part == marker) else {
        return data.to_vec();
    };
    let field = b"\"files\":";
    let end = start
        + data[start..]
            .windows(field.len())
            .position(|part| part == field)
            .unwrap();
    let mut metadata = vec![b'{'];
    metadata.extend_from_slice(data[start..end].strip_suffix(b",").unwrap());
    metadata.push(b'}');
    let metadata: Value = serde_json::from_slice(&metadata).unwrap();
    assert_eq!(metadata["metadata"]["ruby_engine"], engine);
    assert_eq!(metadata["metadata"]["rubocop_version"], "1.89.0");
    let mut result = data[..start].to_vec();
    result.extend_from_slice(&data[end..]);
    result
}

#[test]
fn windows_fork_notice_follows_started_once_and_preserves_negative_phases() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/parallel_fork_fallback.json")).unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(fixture["runtime"]["fork_supported"], false);
    assert_eq!(fixture["coverage"]["subset"], false);
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 25);
    assert_eq!(NOTICE.len(), 54);
    let mut checked = 0;
    for case in fixture["cases"].as_array().unwrap() {
        // 実forkが使える他OSを、Windowsの通知あり参照値へ読み替えない。
        if !cfg!(windows) && case["expect_notice"] == true {
            continue;
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut before = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text.as_str().unwrap().as_bytes()).unwrap();
            before.push((
                path.clone(),
                fs::read(&path).unwrap(),
                fs::metadata(&path).unwrap().modified().unwrap(),
                fs::metadata(&path).unwrap().permissions(),
            ));
        }
        for name in ["empty", "outdir"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.current_dir(root);
        command.env("PATH", "").env("TERM", "dumb");
        for key in [
            "NO_COLOR",
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(key);
        }
        command
            .env("CLICOLOR_FORCE", "0")
            .env("XDG_CACHE_HOME", root.join("cache"));
        for (key, value) in case["env"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        );
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if case["stdin_hex"].is_string() {
            command.stdin(Stdio::piped());
        }
        let mut process = command.spawn().unwrap();
        if case["stdin_hex"].is_string() {
            process
                .stdin
                .take()
                .unwrap()
                .write_all(&bytes(&case["stdin_hex"]))
                .unwrap();
        }
        let actual = process.wait_with_output().unwrap();
        let expected = &case["expected"];
        assert_eq!(
            actual.status.code(),
            Some(expected["exit"].as_i64().unwrap() as i32),
            "{}: {actual:?}",
            case["id"]
        );
        let count = actual
            .stderr
            .windows(NOTICE.len())
            .filter(|part| *part == NOTICE)
            .count();
        assert_eq!(
            count,
            usize::from(case["expect_notice"].as_bool().unwrap()),
            "{}",
            case["id"]
        );
        assert!(
            !actual
                .stdout
                .windows(NOTICE.len())
                .any(|part| part == NOTICE)
        );
        assert_eq!(
            without_metadata(&actual.stdout, "sonicop"),
            without_metadata(&host(bytes(&expected["stdout_hex"])), "ruby"),
            "{} stdout",
            case["id"]
        );
        if case["kind"] == "io-phase" {
            // Rubyのbacktrace全体は既存I/O差。open失敗が通知より前であることだけを検査する。
            assert!(expected["stderr_hex"].is_null());
            assert!(!actual.stderr.is_empty());
            assert!(
                actual
                    .stderr
                    .windows(b"outdir".len())
                    .any(|part| part == b"outdir")
            );
            assert_eq!(count, 0);
        } else {
            assert_eq!(
                without_metadata(&actual.stderr, "sonicop"),
                without_metadata(&host(bytes(&expected["stderr_hex"])), "ruby"),
                "{} stderr",
                case["id"]
            );
        }
        for (name, value) in expected["outputs_hex"].as_object().unwrap() {
            assert_eq!(
                without_metadata(&fs::read(root.join(name)).unwrap(), "sonicop"),
                without_metadata(&host(bytes(value)), "ruby"),
                "{} output {name}",
                case["id"]
            );
        }
        for name in case["outputs"].as_array().unwrap() {
            let name = name.as_str().unwrap();
            if expected["outputs_hex"].get(name).is_none() {
                assert!(fs::symlink_metadata(root.join(name)).is_err());
            }
        }
        assert!(fs::symlink_metadata(root.join("report.json")).is_err());
        for (path, data, modified, permissions) in before {
            assert_eq!(fs::metadata(&path).unwrap().permissions(), permissions);
            assert_eq!(
                fs::read(&path).unwrap(),
                data,
                "{} source {}",
                case["id"],
                path.display()
            );
            assert_eq!(
                fs::metadata(&path).unwrap().modified().unwrap(),
                modified,
                "{} mtime {}",
                case["id"],
                path.display()
            );
        }
        checked += 1;
    }
    if cfg!(windows) {
        assert_eq!(checked, 25);
    } else {
        assert_eq!(checked, 15);
    }
}
