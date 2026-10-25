//! `Array.new` のブロック付き展開で、本家が隣接要素を消す補正を抑止する。

mod support;

use std::fs;

use serde_json::Value;
use support::project::{command, project};

const COP: &str = "Lint/RedundantSplatExpansion";

fn decode_hex(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn block_splat_keeps_sibling_values_and_reference_diagnostics() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/splat_block_reference.json")).unwrap();
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let policy = &manifest["splat_block_value_preservation_divergences"];
    assert_eq!(policy["cop"].as_str(), Some(COP));
    assert_eq!(
        policy["reference_fixture"].as_str(),
        Some("tests/fixtures/splat_block_reference.json")
    );
    assert_eq!(
        policy["candidate_diagnostics"].as_str(),
        Some("reference_inspection_offenses")
    );
    let entries = policy["cases"].as_sequence().unwrap();
    assert_eq!(reference["cases"].as_array().unwrap().len(), 12);
    assert_eq!(reference["effect_cases"].as_array().unwrap().len(), 8);
    assert_eq!(entries.len(), 20);
    for case in reference["cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(reference["effect_cases"].as_array().unwrap())
    {
        let entry = entries
            .iter()
            .find(|entry| entry["id"].as_str() == case["id"].as_str())
            .unwrap();
        for key in ["original_sha256", "reference_sha256", "candidate_sha256"] {
            assert_eq!(entry[key].as_str(), case[key].as_str(), "{case}");
        }
        assert_eq!(entry["candidate_exit"].as_i64(), Some(1));
        assert_eq!(entry["candidate_corrected"].as_bool(), Some(false));
        assert_eq!(entry["reference_exit"].as_i64(), Some(0));
        for (key, fixture_key) in [
            (
                "reference_inspection_offense_count",
                "reference_inspection_offenses",
            ),
            ("reference_correction_offense_count", "reference_offenses"),
            ("candidate_offense_count", "candidate_offenses"),
        ] {
            assert_eq!(
                entry[key].as_u64(),
                Some(case[fixture_key].as_array().unwrap().len() as u64)
            );
        }
        assert_eq!(
            entry["original_runtime_stdout_sha256"].as_str(),
            case["original_runtime"]["stdout_sha256"].as_str()
        );
        assert_eq!(
            entry["reference_runtime_stdout_sha256"].as_str(),
            case["reference_runtime"]["stdout_sha256"].as_str()
        );
        assert_eq!(
            entry["candidate_runtime_stdout_sha256"].as_str(),
            case["candidate_runtime"]["stdout_sha256"].as_str()
        );
        let original = decode_hex(case["original_hex"].as_str().unwrap());
        let reference_bytes = decode_hex(case["reference_hex"].as_str().unwrap());
        assert_ne!(original, reference_bytes);
        assert_ne!(case["original_runtime"], case["reference_runtime"]);
        assert_eq!(case["original_runtime"], case["candidate_runtime"]);
        assert_eq!(case["original_hex"], case["candidate_hex"]);
        assert_eq!(case["original_sha256"], case["candidate_sha256"]);
        let directory = project(&[]);
        let input = directory.path().join("example.rb");
        fs::write(&input, &original).unwrap();
        let mode = if case["mode"] == "safe" { "-a" } else { "-A" };
        // 本家は最初の補正で後続の splat ごと消す。補正前の全診断を維持する。
        let expected = case["reference_inspection_offenses"].clone();
        assert!(
            expected
                .as_array()
                .unwrap()
                .iter()
                .all(|offense| offense["corrected"] == false)
        );
        assert_eq!(case["candidate_offenses"], expected);
        for run_mode in ["inspect", mode, mode] {
            let mut run = command(directory.path());
            run.args(["--cache", "false", "--only", COP, "-f", "json"]);
            if run_mode != "inspect" {
                run.arg(run_mode);
            }
            let output = run
                .arg("example.rb")
                .assert()
                .code(1)
                .get_output()
                .stdout
                .clone();
            let found: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(found["files"][0]["offenses"], expected, "{case}");
            assert_eq!(found["summary"]["target_file_count"], 1);
            assert_eq!(found["summary"]["inspected_file_count"], 1);
            assert_eq!(
                found["summary"]["offense_count"],
                expected.as_array().unwrap().len()
            );
            assert_eq!(fs::read(&input).unwrap(), original, "{case}");
        }
    }
}

#[test]
fn block_splat_still_corrects_without_discarding_other_values() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/splat_block_reference.json")).unwrap();
    let controls = reference["controls"].as_array().unwrap();
    assert_eq!(controls.len(), 16);
    for case in controls {
        let original = decode_hex(case["original_hex"].as_str().unwrap());
        let written = decode_hex(case["reference_hex"].as_str().unwrap());
        // 本体は本家の実バイトに合わせ、原本の改行形式を保つ既存方針を重ねる。
        let normalized = String::from_utf8(written).unwrap().replace("\r\n", "\n");
        let expected = if case["ending"] == "crlf" {
            normalized.replace('\n', "\r\n")
        } else {
            normalized
        };
        let directory = project(&[]);
        let input = directory.path().join("example.rb");
        fs::write(&input, &original).unwrap();
        let mode = if case["mode"] == "safe" { "-a" } else { "-A" };
        let output = command(directory.path())
            .args([
                "--cache",
                "false",
                "--only",
                COP,
                "-f",
                "json",
                mode,
                "example.rb",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let found: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(found["files"][0]["offenses"], case["reference_offenses"]);
        assert_eq!(fs::read(&input).unwrap(), expected.as_bytes());
        assert_eq!(case["original_runtime"], case["candidate_runtime"]);
    }
}

#[test]
fn block_splat_records_keep_effect_order_and_evaluation_counts() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/splat_block_reference.json")).unwrap();
    for case in reference["effect_cases"].as_array().unwrap() {
        let expected = match case["name"].as_str().unwrap() {
            "side_effects" => "[[99, 1, 1, 100], [:before, :block, :block, :after]]\n",
            "multiple_splats" => "[[1, 2], [:first, :second]]\n",
            other => panic!("未登録の副作用ケース: {other}"),
        };
        assert_eq!(case["original_runtime"]["exit"], 0);
        assert_eq!(case["candidate_runtime"], case["original_runtime"]);
        assert_ne!(case["reference_runtime"], case["original_runtime"]);
        assert_eq!(
            case["candidate_runtime"]["stdout"]
                .as_str()
                .unwrap()
                .replace("\r\n", "\n"),
            expected
        );
    }
    // 本家の二重の Array.new 補正は内部エラーになる。補正一致例へ数えず原本保持を調べる。
    let source = "events = []\nary = [*Array.new(1) { events << :first; 1 }, *Array.new(1) { events << :second; 2 }]\np [ary, events]\n";
    for ending in ["\n", "\r\n"] {
        for mode in ["-a", "-A"] {
            let original = source.replace('\n', ending);
            let directory = project(&[("example.rb", &original)]);
            let output = command(directory.path())
                .args([
                    "--cache",
                    "false",
                    "--only",
                    COP,
                    "-f",
                    "json",
                    mode,
                    "example.rb",
                ])
                .assert()
                .code(1)
                .get_output()
                .stdout
                .clone();
            let found: Value = serde_json::from_slice(&output).unwrap();
            let offenses = found["files"][0]["offenses"].as_array().unwrap();
            assert_eq!(offenses.len(), 2);
            assert!(offenses.iter().all(|offense| offense["corrected"] == false));
            assert_eq!(
                fs::read(directory.path().join("example.rb")).unwrap(),
                original.as_bytes()
            );
        }
    }
}
