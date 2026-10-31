//! 本家のversion輸送を照合し、本文はSonicop自身のidentityとして保持する。

use std::{collections::BTreeMap, fs, process::Command};

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

#[test]
fn version_aliases_keep_native_identity_and_recorded_transport_and_early_exit() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/version_textmode.json")).unwrap();
    assert_eq!(fixture["runtime"]["ruby"], "4.0.7");
    assert_eq!(fixture["runtime"]["gems"]["rubocop"], "1.89.0");
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 20);
    assert_eq!(
        cases
            .iter()
            .filter(|case| case["kind"] == "rejection" && case["residual_issue"] == 137)
            .count(),
        4
    );
    let eol = if cfg!(windows) { "\r\n" } else { "\n" };
    // 本家Windowsの輸送16条件と、native自身のidentityを別々に確認する。
    for case in cases.iter().filter(|case| case["kind"] != "rejection") {
        let folder = tempfile::tempdir().unwrap();
        let files = case["files"].as_object().unwrap();
        for (name, source) in files {
            fs::write(folder.path().join(name), source.as_str().unwrap()).unwrap();
        }
        let before: BTreeMap<_, _> = files
            .keys()
            .map(|name| {
                (name.clone(), {
                    let metadata = fs::metadata(folder.path().join(name)).unwrap();
                    (metadata.modified().unwrap(), metadata.permissions())
                })
            })
            .collect();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(folder.path())
            .env("PATH", "")
            .env("TERM", "dumb");
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            command.env_remove(key);
        }
        let actual = command
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap()),
            )
            .output()
            .unwrap();
        assert_eq!(
            actual.status.code().map(i64::from),
            case["exit"].as_i64(),
            "{}",
            case["id"]
        );
        assert_eq!(
            actual.stderr,
            bytes(case["stderr_hex"].as_str().unwrap()),
            "{}",
            case["id"]
        );
        let expected = match case["kind"].as_str().unwrap() {
            "version" => format!("{}{eol}", sonicop::VERSION),
            "verbose" => format!(
                "sonicop {} (RuboCop {} CLI, tree-sitter-ruby v26.10.107, analyzing as Ruby 2.7) [{} {}]{eol}",
                sonicop::VERSION,
                sonicop::RUBOCOP_COMPAT_VERSION,
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
            other => panic!("unexpected version fixture kind: {other}"),
        };
        // 版本文の実装系差は同一視せず、明示したnative identityと物理bytesを固定する。
        assert_eq!(actual.stdout, expected.as_bytes(), "{}", case["id"]);
        for (name, hex) in case["after_hex"].as_object().unwrap() {
            assert_eq!(
                fs::read(folder.path().join(name)).unwrap(),
                bytes(hex.as_str().unwrap()),
                "{}",
                case["id"]
            );
            assert_eq!(
                fs::metadata(folder.path().join(name))
                    .unwrap()
                    .modified()
                    .unwrap(),
                before[name].0,
                "{}",
                case["id"]
            );
            assert_eq!(
                fs::metadata(folder.path().join(name))
                    .unwrap()
                    .permissions(),
                before[name].1,
                "{}",
                case["id"]
            );
        }
        let out = folder.path().join("out.txt");
        assert_eq!(
            out.exists(),
            case["out_exists"].as_bool().unwrap(),
            "{}",
            case["id"]
        );
        if out.exists() {
            assert_eq!(
                fs::read(out).unwrap(),
                bytes(case["out_hex"].as_str().unwrap()),
                "{}",
                case["id"]
            );
        }
    }
}
