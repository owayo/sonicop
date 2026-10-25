//! 実 RuboCop CLI の記録と、静的正規表現の診断・補正後 byte を照合する。
mod support;

use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};
use support::project::{command, project_with_ruby, recorded_disk_bytes};

fn decode(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn static_regexp_reports_and_correction_bytes_match_the_real_cli() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/regexp_validation.json")).unwrap();
    assert_eq!(fixture["rubocop_version"], "1.89.0");
    let cases = fixture["cases"].as_object().unwrap();
    assert!(!cases.is_empty());
    let names: Vec<_> = cases.keys().map(String::as_str).collect();
    for group in fixture["groups"].as_array().unwrap() {
        let target = group["target"].as_str().unwrap();
        let engine = group["engine"].as_str().unwrap_or("default");
        let directory = project_with_ruby(&[], target);
        fs::write(
            directory.path().join(".rubocop.yml"),
            group["config"].as_str().unwrap(),
        )
        .unwrap();
        for run in group["runs"].as_array().unwrap() {
            let mode = run["mode"].as_str().unwrap_or("inspect");
            for (name, hex) in cases {
                fs::write(directory.path().join(name), decode(hex.as_str().unwrap())).unwrap();
            }
            let mut invocation = command(directory.path());
            // source gem を含む配布物は、Ruby と外部の regexp compiler が無い環境でも完結する。
            invocation.env("PATH", "").args([
                "--cache",
                "false",
                "--only",
                fixture["only"].as_str().unwrap(),
                "--format",
                "json",
            ]);
            if mode != "inspect" {
                invocation.arg(mode);
            }
            let output = invocation.args(&names).output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(run["exit"].as_i64().unwrap() as i32),
                "{target} {engine} {mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                report["summary"], run["summary"],
                "{target} {engine} {mode}"
            );
            assert_eq!(
                report["summary"]["inspected_file_count"],
                cases.len(),
                "incomplete run"
            );
            let actual: BTreeMap<_, _> = report["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|file| {
                    let name = Path::new(file["path"].as_str().unwrap())
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap();
                    (name, &file["offenses"])
                })
                .collect();
            assert_eq!(actual.len(), cases.len());
            for expected in run["files"].as_array().unwrap() {
                let name = expected["path"].as_str().unwrap();
                assert_eq!(
                    actual.get(name),
                    Some(&&expected["offenses"]),
                    "{target} {engine} {mode} {name}"
                );
                let original = decode(cases[name].as_str().unwrap());
                let mut written = decode(run["written"][name].as_str().unwrap());
                if original.windows(2).any(|pair| pair == b"\r\n") {
                    // 本家記録は保持し、既登録の CRLF 保存差だけを書き戻し byte の照合で戻す。
                    let mut preserved = Vec::with_capacity(written.len());
                    for (offset, &byte) in written.iter().enumerate() {
                        if byte == b'\n' && (offset == 0 || written[offset - 1] != b'\r') {
                            preserved.push(b'\r');
                        }
                        preserved.push(byte);
                    }
                    written = preserved;
                }
                let rewritten = mode != "inspect"
                    && expected["offenses"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|offense| offense["corrected"] == true);
                let written = recorded_disk_bytes(&original, &written, rewritten, 0);
                assert_eq!(
                    fs::read(directory.path().join(name)).unwrap(),
                    written,
                    "{target} {engine} {mode} {name} written bytes"
                );
            }
        }
    }
}

#[test]
fn vendored_ruby_sources_match_the_fixed_provenance() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/sonicop-onigmo-compat");
    let inventory: Value =
        serde_json::from_slice(&fs::read(root.join("PROVENANCE.json")).unwrap()).unwrap();
    let entries = inventory.as_array().unwrap();
    assert_eq!(entries.len(), 27);
    for entry in entries {
        let name = entry["path"].as_str().unwrap();
        let content = fs::read(root.join("vendor").join(name)).unwrap();
        assert_eq!(
            blake3::hash(&content).to_hex().as_str(),
            entry["blake3"].as_str().unwrap(),
            "vendor source changed: {name}"
        );
    }
}

#[test]
fn upstream_regexp_internal_exceptions_remain_explicit_divergences() {
    let manifest: Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let cases = manifest["regexp_internal_error_divergences"]
        .as_array()
        .unwrap();
    assert_eq!(cases.len(), 6);
    let controls = manifest["regexp_internal_error_controls"]
        .as_array()
        .unwrap();
    assert_eq!(controls.len(), 2);
    for case in cases.iter().chain(controls) {
        let name = case["id"].as_str().unwrap();
        if case["upstream"]["exit"] == 2 {
            assert_eq!(case["upstream"]["summary"]["inspected_file_count"], 0);
        } else {
            assert_eq!(case["upstream"]["files"], case["sonicop"]["files"]);
            assert_eq!(case["upstream"]["summary"], case["sonicop"]["summary"]);
        }
        for ending in ["\n", "\r\n"] {
            let source = case["source"].as_str().unwrap().replace('\n', ending);
            let directory = project_with_ruby(
                &[("example.rb", &source)],
                case["target_ruby"].as_str().unwrap(),
            );
            fs::write(
                directory.path().join(".rubocop.yml"),
                format!(
                    "AllCops:\n  TargetRubyVersion: {}\n  ParserEngine: {}\n  NewCops: disable\n",
                    case["target_ruby"].as_str().unwrap(),
                    case["parser_engine"].as_str().unwrap()
                ),
            )
            .unwrap();
            for mode in [None, Some("-a"), Some("-A")] {
                let mut invocation = command(directory.path());
                invocation.env("PATH", "").args([
                    "--cache",
                    "false",
                    "--only",
                    "Lint/Syntax",
                    "--format",
                    "json",
                ]);
                if let Some(mode) = mode {
                    invocation.arg(mode);
                }
                let output = invocation.arg("example.rb").output().unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(case["sonicop"]["exit"].as_i64().unwrap() as i32),
                    "{name} {mode:?}"
                );
                let report: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["files"], case["sonicop"]["files"], "{name} {mode:?}");
                assert_eq!(
                    report["summary"], case["sonicop"]["summary"],
                    "{name} {mode:?}"
                );
                assert_eq!(
                    fs::read(directory.path().join("example.rb")).unwrap(),
                    source.as_bytes(),
                    "{name} {mode:?}"
                );
                assert!(output.stderr.is_empty(), "{name} {mode:?}");
            }
        }
    }
}
