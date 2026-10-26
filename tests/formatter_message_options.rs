//! 共通 message 注釈を、固定版 RuboCop の実 CLI 記録と全項目で照合する。

use std::fs;
use std::path::Path;

use assert_cmd::cargo::cargo_bin_cmd;
use serde_json::Value;
use tempfile::tempdir;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/formatter_messages.json"))
        .expect("本家の message 記録が JSON でない")
}

fn cache_controls() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/formatter_message_cache_controls.json"
    ))
    .expect("本家の追加 cache 対照が JSON でない")
}

fn assert_case(root: &Path, source: &[u8], case: &Value) {
    let name = case["name"].as_str().unwrap();
    let config = case["config"].as_str().unwrap();
    if root.join("case.rb").exists() {
        // mtime を動かすと cache が無効になるため、再実行では一切書き直さない。
        assert_eq!(
            fs::read(root.join("config.yml")).unwrap(),
            config.as_bytes()
        );
        assert_eq!(fs::read(root.join("case.rb")).unwrap(), source);
    } else {
        fs::write(root.join("config.yml"), config).unwrap();
        fs::write(root.join("case.rb"), source).unwrap();
    }
    for index in 0..2 {
        let path = root.join(format!("output{index}.txt"));
        if path.exists() {
            fs::remove_file(path).unwrap();
        }
    }
    let args = case["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect::<Vec<_>>();
    let output = cargo_bin_cmd!("sonicop")
        .current_dir(root)
        .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        case["exit"].as_i64().map(|v| v as i32),
        "{name}"
    );
    // debug のログは既存の別出力経路にあり、ここでは formatter の実ファイルだけを比較する。
    if name != "json-debug-off" {
        assert!(output.stderr.is_empty(), "{name}: {:?}", output.stderr);
    }
    assert_eq!(fs::read(root.join("case.rb")).unwrap(), source, "{name}");
    for (index, expected) in case["outputs"].as_array().unwrap().iter().enumerate() {
        let bytes = fs::read(root.join(format!("output{index}.txt"))).unwrap();
        if expected["format"] == "json" {
            let mut report: Value = serde_json::from_slice(&bytes).unwrap();
            // 実装名と実行環境を除き、message 以外も本家の記録と照合する。
            report.as_object_mut().unwrap().remove("metadata");
            assert_eq!(report, expected["report"], "{name} output {index}");
        } else {
            let text = String::from_utf8(bytes).unwrap().replace("\r\n", "\n");
            assert_eq!(
                text,
                expected["text"].as_str().unwrap(),
                "{name} output {index}"
            );
        }
    }
}

fn source(document: &Value) -> Vec<u8> {
    let (pairs, remainder) = document["source_hex"]
        .as_str()
        .unwrap()
        .as_bytes()
        .as_chunks::<2>();
    assert!(remainder.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn explicit_configured_and_mixed_formatter_messages_match_upstream() {
    let document = fixture();
    let source = source(&document);
    let cases = document["cases"].as_array().unwrap();
    assert_eq!(
        cases.iter().filter(|case| case["cached"] == false).count(),
        26
    );
    // 元の共有 cache 9 件は上書きしない。うち 2 条件の通常診断差は #110 で追跡する。
    assert_eq!(
        cases.iter().filter(|case| case["cached"] == true).count(),
        9
    );
    for case in cases.iter().filter(|case| case["cached"] == false) {
        let project = tempdir().unwrap();
        assert_case(project.path(), &source, case);
    }
}

#[test]
fn cached_raw_messages_survive_annotation_and_formatter_switches() {
    let document = cache_controls();
    let source = source(&document);
    let project = tempdir().unwrap();
    let cases = document["cases"].as_array().unwrap();
    assert_eq!(
        cases.iter().filter(|case| case["cached"] == true).count(),
        9
    );
    // 同じ path・config・本文と cache root のまま、on/off・順序・リンクを切り替える。
    for case in cases.iter().filter(|case| case["cached"] == true) {
        assert_case(project.path(), &source, case);
    }
    assert!(project.path().join("cache").exists());
}
