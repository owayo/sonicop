//! Regexp match の AST 分類を、二つの cop の本家 CLI 全診断・物理 byte で検査する。

mod support;

use std::fs;

use support::project::{command, project_without_pinned_ruby};

fn decode_hex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    (0..hex.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
        .collect()
}

#[test]
fn match_dispatch_matches_upstream_fields_and_correction_bytes() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/match_with_lvasgn.json")).unwrap();
    let groups = fixture["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 36);
    for group in groups {
        let label = format!(
            "{} / {} / {} / {} / {}",
            group["version"], group["engine"], group["ending"], group["cop"], group["mode"]
        );
        let cases = group["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 20, "{label}");
        let directory =
            project_without_pinned_ruby(&[(".rubocop.yml", group["yaml"].as_str().unwrap())]);
        for case in cases {
            fs::write(
                directory.path().join(case["filename"].as_str().unwrap()),
                case["source"].as_str().unwrap().as_bytes(),
            )
            .unwrap();
        }
        let mut run = command(directory.path());
        run.args([
            "--config",
            ".rubocop.yml",
            "--cache",
            "false",
            "--only",
            group["cop"].as_str().unwrap(),
            "-f",
            "json",
        ]);
        if let Some(mode) = group["mode"].as_str() {
            run.arg(mode);
        }
        run.args(cases.iter().map(|case| case["filename"].as_str().unwrap()));
        let output = run.output().unwrap();
        assert_eq!(
            output.status.code(),
            group["exit"].as_i64().map(|code| code as i32),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{label}: {:?}", output.stderr);
        let mut report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        report.as_object_mut().unwrap().remove("metadata");
        assert_eq!(report, group["report"], "{label}");
        for case in cases {
            let mut expected = decode_hex(case["after_hex"].as_str().unwrap());
            if !cfg!(windows) && group["ending"] == "lf" {
                // Windows 本家の native 書出しだけを POSIX の LF へ戻す。元 CRLF は保持する。
                assert!(!case["source"].as_str().unwrap().contains('\r'));
                expected = String::from_utf8(expected)
                    .unwrap()
                    .replace("\r\n", "\n")
                    .into_bytes();
            }
            assert_eq!(
                fs::read(directory.path().join(case["filename"].as_str().unwrap())).unwrap(),
                expected,
                "{label} / {}",
                case["name"]
            );
        }
    }
}

#[cfg(windows)]
fn repeated_source(part: &serde_json::Value) -> Vec<u8> {
    format!(
        "{}{}{}",
        part["prefix"].as_str().unwrap(),
        part["padding"]
            .as_str()
            .unwrap()
            .repeat(part["count"].as_u64().unwrap() as usize),
        part["suffix"].as_str().unwrap()
    )
    .into_bytes()
}

#[cfg(windows)]
#[test]
fn reparse_cap_matches_upstream_with_explicit_binding_and_crlf_divergences() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/match_with_lvasgn_cap.json")).unwrap();
    let runtime: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/match_with_lvasgn_runtime.json")).unwrap();
    let manifest: serde_json::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let safety = &manifest["match_dispatch_capture_divergences"];
    let crlf = &manifest["match_dispatch_crlf_controls"];
    assert_eq!(safety["withhold_correction"], true);
    assert_eq!(safety["candidate_preserves_source"], true);
    let cases = runtime["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 26);
    let original = cases
        .iter()
        .find(|case| case["id"] == "named-original")
        .unwrap();
    let corrected = cases
        .iter()
        .find(|case| case["id"] == "named-reference")
        .unwrap();
    // 新Runtimeの前後差と旧MRIの前後不変を分ける。候補の出力を正本には入れない。
    assert_ne!(
        original["observations"]["4.0.7"]["run"]["stdout_hex"],
        corrected["observations"]["4.0.7"]["run"]["stdout_hex"]
    );
    assert_eq!(
        original["observations"]["2.7.8"]["run"]["stdout_hex"],
        corrected["observations"]["2.7.8"]["run"]["stdout_hex"]
    );
    let groups = fixture["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 18);
    let mut safety_count = 0;
    let mut crlf_count = 0;
    for group in groups {
        let target = format!(
            "{}/{}",
            group["version"].as_str().unwrap(),
            group["engine"].as_str().unwrap()
        );
        let mode = group["mode"].as_str().unwrap_or("inspect");
        let label = format!("{target} / {} / {mode}", group["ending"]);
        let cases = group["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 13);
        let directory =
            project_without_pinned_ruby(&[(".rubocop.yml", group["yaml"].as_str().unwrap())]);
        for case in cases {
            fs::write(
                directory.path().join(case["filename"].as_str().unwrap()),
                repeated_source(&case["source"]),
            )
            .unwrap();
        }
        let mut expected = group["report"].clone();
        let mut expected_bytes = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            let mut bytes = repeated_source(&case["after"]);
            let permitted = |entry: &serde_json::Value| {
                entry["cases"].as_array().unwrap().contains(&case["name"])
                    && entry["targets"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|value| value.as_str() == Some(&target))
                    && entry["modes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|value| value.as_str() == Some(mode))
                    && entry["endings"]
                        .as_array()
                        .unwrap()
                        .contains(&group["ending"])
            };
            if permitted(safety) {
                safety_count += 1;
                let offenses = expected["files"][index]["offenses"].as_array_mut().unwrap();
                assert_eq!(offenses.len(), 1, "{label}");
                assert_eq!(offenses[0]["corrected"], true);
                assert_eq!(offenses[0]["correctable"], true);
                offenses[0]["corrected"] = false.into();
                bytes = repeated_source(&case["source"]);
            } else if permitted(crlf) {
                crlf_count += 1;
                let lf = groups
                    .iter()
                    .find(|other| {
                        other["ending"] == "lf"
                            && other["version"] == group["version"]
                            && other["engine"] == group["engine"]
                            && other["mode"] == group["mode"]
                    })
                    .unwrap();
                assert!(
                    expected["files"][index]["offenses"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
                assert_eq!(
                    lf["report"]["files"][index]["offenses"]
                        .as_array()
                        .unwrap()
                        .len(),
                    1
                );
                expected["files"][index] = lf["report"]["files"][index].clone();
                if mode != "inspect" {
                    // 生のCRLF原本は保持し、期待だけを同じ形の実LF本家補正から導く。
                    bytes = repeated_source(&lf["cases"][index]["after"]);
                }
            }
            expected_bytes.push(bytes);
        }
        expected["summary"]["offense_count"] = expected["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["offenses"].as_array().unwrap().len())
            .sum::<usize>()
            .into();
        let expected_exit = i32::from(expected["files"].as_array().unwrap().iter().any(|file| {
            file["offenses"]
                .as_array()
                .unwrap()
                .iter()
                .any(|offense| offense["corrected"] == false)
        }));
        let mut run = command(directory.path());
        run.args([
            "--config",
            ".rubocop.yml",
            "--cache",
            "false",
            "--only",
            "Style/RedundantParentheses",
            "-f",
            "json",
        ]);
        if mode != "inspect" {
            run.arg(mode);
        }
        run.args(cases.iter().map(|case| case["filename"].as_str().unwrap()));
        let output = run.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "{label}: {:?}",
            output.stderr
        );
        assert!(output.stderr.is_empty(), "{label}: {:?}", output.stderr);
        let mut actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        actual.as_object_mut().unwrap().remove("metadata");
        assert_eq!(actual, expected, "{label}");
        for (case, bytes) in cases.iter().zip(expected_bytes) {
            assert_eq!(
                fs::read(directory.path().join(case["filename"].as_str().unwrap())).unwrap(),
                bytes,
                "{label} / {}",
                case["name"]
            );
        }
    }
    assert_eq!(
        safety_count,
        safety["expected_correction_conditions"].as_u64().unwrap()
    );
    assert_eq!(crlf_count, crlf["expected_conditions"].as_u64().unwrap());
}
