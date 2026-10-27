//! cache/configの最後優先を、本家の診断とcache書込みで照合する。
use std::{fs, path::Path, process::Command};

use serde_json::Value;

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap().as_bytes();
    assert!(text.len().is_multiple_of(2));
    text.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn report(raw: &[u8]) -> Value {
    let mut report: Value = serde_json::from_slice(raw).unwrap();
    // 実装固有のmetadataだけを分け、診断・対象順・summaryは全項目を保つ。
    report.as_object_mut().unwrap().remove("metadata");
    report
}

fn cache_has_files(root: &Path) -> bool {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        if !directory.exists() {
            continue;
        }
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_file() {
                return true;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    false
}

fn check_case(fixture: &Value, case: &Value) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut files = vec![
        ("case.rb", bytes(&fixture["source_hex"])),
        ("a.yml", bytes(&fixture["config_a_hex"])),
        ("b.yml", bytes(&fixture["config_b_hex"])),
        (".rubocop.yml", bytes(&fixture["config_a_hex"])),
        ("invalid.yml", b"AllCops: [\n".to_vec()),
    ];
    if let Some(dotfile) = case["dotfile"].as_str() {
        files.push((".rubocop", dotfile.as_bytes().to_vec()));
    }
    for (name, raw) in &files {
        fs::write(root.join(name), raw).unwrap();
    }
    let states: Vec<_> = files
        .iter()
        .map(|(name, _)| {
            let metadata = fs::metadata(root.join(name)).unwrap();
            (metadata.modified().unwrap(), metadata.permissions())
        })
        .collect();
    let cache = root.join("cache-home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command
        .current_dir(root)
        .env("XDG_CACHE_HOME", &cache)
        .env("RUBOCOP_CACHE_ROOT", cache.join("reference"));
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        command.env_remove(key);
    }
    if let Some(options) = case["env_options"].as_str() {
        command.env("RUBOCOP_OPTS", options);
    }
    for argument in case["args"].as_array().unwrap() {
        command.arg(argument.as_str().unwrap());
    }
    let output = command.output().unwrap();
    let id = case["id"].as_str().unwrap();
    assert_eq!(
        output.status.code(),
        case["exit"].as_i64().map(|v| v as i32),
        "{id}: {:?}",
        output.stderr
    );
    if case["kind"] == "report" {
        assert!(output.stderr.is_empty(), "{id}: {:?}", output.stderr);
        if case["report"].is_null() {
            assert!(output.stdout.is_empty(), "{id}");
        } else {
            assert_eq!(report(&output.stdout), case["report"], "{id}");
        }
    } else {
        // 拒否の境界であり、実装固有のstderr本文の完全一致には数えない。
        assert!(output.stdout.is_empty(), "{id}");
        assert!(!output.stderr.is_empty(), "{id}");
    }
    for (name, expected) in case["outputs"].as_object().unwrap() {
        let path = root.join(name);
        if expected.is_null() {
            assert!(!path.exists(), "{id}/{name}");
        } else {
            assert_eq!(report(&fs::read(path).unwrap()), *expected, "{id}/{name}");
        }
    }
    assert_eq!(
        cache_has_files(&cache),
        case["cache_files"].as_bool().unwrap(),
        "{id}"
    );
    for ((name, raw), (modified, permissions)) in files.iter().zip(states) {
        let metadata = fs::metadata(root.join(name)).unwrap();
        assert_eq!(fs::read(root.join(name)).unwrap(), *raw, "{id}/{name}");
        assert_eq!(metadata.modified().unwrap(), modified, "{id}/{name}");
        assert_eq!(metadata.permissions(), permissions, "{id}/{name}");
    }
}

#[test]
fn repeated_cache_config_match_reports_output_slots_and_cache_writes() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/repeated_options.json")).unwrap();
    let cases: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "report")
        .collect();
    assert_eq!(cases.len(), 31);
    assert_eq!(cases.iter().filter(|c| c["cache_files"] == true).count(), 6);
    for case in cases {
        check_case(&fixture, case);
    }
}

#[test]
fn repeated_options_keep_missing_and_invalid_values_rejected() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/repeated_options.json")).unwrap();
    let cases: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "rejection")
        .collect();
    assert_eq!(cases.len(), 11);
    for case in cases {
        check_case(&fixture, case);
    }
}
