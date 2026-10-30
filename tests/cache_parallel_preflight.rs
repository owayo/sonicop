//! 明示parallelと設定cacheの初期検証を、本家の全raw・完了JSONへ照合する。

use std::{collections::BTreeMap, fs, process::Command};

use serde_json::Value;

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
        // Windows参照のreport輸送だけを投影し、Ruby/config原本には適用しない。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn report(raw: &[u8], engine: &str) -> Value {
    assert_eq!(raw.last(), Some(&b'}'), "compact JSONに終端改行を足さない");
    let mut value: Value = serde_json::from_slice(raw).unwrap();
    assert_eq!(value["metadata"]["ruby_engine"], engine);
    assert_eq!(value["metadata"]["rubocop_version"], "1.89.0");
    if engine == "ruby" {
        assert_eq!(value["metadata"]["ruby_version"], "4.0.7");
    }
    value.as_object_mut().unwrap().remove("metadata");
    value
}

#[test]
fn configured_cache_parallel_validation_preserves_early_modes_and_pending_notice_order() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/cache_parallel_preflight.json")).unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(fixture["coverage"]["subset"], false);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 34);
    let mut kinds = BTreeMap::new();
    let mut input_count = 0;
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut originals = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = root.join(name);
            fs::write(&path, text.as_str().unwrap().as_bytes()).unwrap();
            let info = fs::metadata(&path).unwrap();
            originals.push((
                path.clone(),
                fs::read(&path).unwrap(),
                info.modified().unwrap(),
                info.permissions(),
            ));
            input_count += 1;
        }
        for name in ["empty", "outdir"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(root)
            .env("TERM", "dumb")
            .env("CLICOLOR_FORCE", "0")
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("PATH", "");
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "NO_COLOR",
            "SONICOP_RUNTIME_RUBY",
        ] {
            command.env_remove(key);
        }
        for (name, value) in case["env"].as_object().unwrap() {
            command.env(name, value.as_str().unwrap());
        }
        command.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        );
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            case["exit"].as_i64(),
            "{}: {output:?}",
            case["id"]
        );
        let kind = case["kind"].as_str().unwrap();
        *kinds.entry(kind).or_insert(0) += 1;
        let mut stderr = bytes(&case["stderr_hex"]);
        if !cfg!(windows) && kind == "pending-report" {
            // 158のWindows専用fallbackだけを除き、pending本文の全bytesは検査する。
            let notice = b"parallel: Process.fork is not supported by this Ruby\r\n";
            assert!(stderr.ends_with(notice));
            stderr.truncate(stderr.len() - notice.len());
        }
        assert_eq!(output.stderr, host(stderr), "{} stderr", case["id"]);
        let expected = host(bytes(&case["stdout_hex"]));
        match kind {
            "report" | "notice-report" | "pending-report" => {
                let start = expected.iter().position(|byte| *byte == b'{').unwrap();
                assert!(
                    output.stdout.starts_with(&expected[..start]),
                    "{}",
                    case["id"]
                );
                assert_eq!(
                    report(&output.stdout[start..], "sonicop"),
                    report(&expected[start..], "ruby"),
                    "{} report",
                    case["id"]
                );
            }
            // 参照のidentityを書き換えず、native自身の版と物理改行を別に検査する。
            "version" => assert_eq!(
                output.stdout,
                host(format!("{}\r\n", env!("CARGO_PKG_VERSION")).into_bytes())
            ),
            "verbose-version" => {
                let text = String::from_utf8(output.stdout).unwrap();
                assert!(text.starts_with(&format!("sonicop {} ", env!("CARGO_PKG_VERSION"))));
                assert!(text.ends_with(if cfg!(windows) { "\r\n" } else { "\n" }));
                assert_eq!(text.matches('\n').count(), 1);
                assert!(!text.contains("\r\r\n"));
            }
            _ => assert_eq!(output.stdout, expected, "{} stdout", case["id"]),
        }
        assert_eq!(
            fs::symlink_metadata(root.join("report.json")).is_ok(),
            case["out_exists"].as_bool().unwrap(),
            "{} output state",
            case["id"]
        );
        for (path, raw, modified, permissions) in originals {
            assert_eq!(fs::read(&path).unwrap(), raw, "{} source", case["id"]);
            let info = fs::metadata(path).unwrap();
            assert_eq!(info.modified().unwrap(), modified, "{} mtime", case["id"]);
            assert_eq!(info.permissions(), permissions, "{} mode", case["id"]);
        }
    }
    assert_eq!(input_count, 137);
    assert_eq!(kinds["rejection"], 13);
    assert_eq!(kinds["invalid-cache"], 3);
    assert_eq!(kinds["unknown-config"], 1);
    assert_eq!(kinds["report"], 9);
    assert_eq!(kinds["notice-report"], 2);
    assert_eq!(kinds["pending-report"], 1);
    assert_eq!(kinds["text"], 3);
    assert_eq!(kinds["version"] + kinds["verbose-version"], 2);
}
