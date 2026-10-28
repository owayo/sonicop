//! 実 RuboCop 1.89.0 CLI の記録を、診断全項目・終了値・ディスクの補正バイトで照合する。
//! 再記録は `python3 scripts/record_regexp_semantics.py`。期待値を候補から採らない。

mod support;
use support::project::{recorded_disk_bytes, without_cache_parallel_notice};

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Case {
    id: String,
    path: String,
    source: String,
    offenses: Vec<Value>,
    after: String,
}

#[derive(Deserialize)]
struct Group {
    id: String,
    target: String,
    ending: String,
    config: String,
    only: String,
    mode: String,
    exit: i32,
    files: Vec<Case>,
    delegated_to_regexp_validation: Vec<String>,
}

#[derive(Deserialize)]
struct BindingDivergence {
    case: String,
    targets: Vec<String>,
    reference: String,
    reason: String,
}

#[test]
fn static_regexps_follow_the_selected_parser_and_version() {
    let groups: Vec<Group> =
        serde_json::from_str(include_str!("fixtures/regexp_semantics.json")).unwrap();
    assert!(!groups.is_empty());
    assert_eq!(
        groups.iter().map(|group| group.files.len()).sum::<usize>(),
        3768
    );
    assert_eq!(
        groups
            .iter()
            .map(|group| group.delegated_to_regexp_validation.len())
            .sum::<usize>(),
        48
    );
    assert!(
        groups
            .iter()
            .flat_map(|group| &group.delegated_to_regexp_validation)
            .all(|case| matches!(
                case.as_str(),
                "encoding_meta_hex_False" | "encoding_meta_octal_False"
            ))
    );
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let divergences: Vec<BindingDivergence> =
        serde_yaml_ng::from_value(manifest["regexp_binding_divergences"].clone()).unwrap();
    assert!(
        divergences
            .iter()
            .all(|entry| !entry.reason.trim().is_empty())
    );
    let fold_guard = &manifest["regexp_capture_fold_guard_divergences"];
    let guard_cases = fold_guard["cases"].as_sequence().unwrap();
    let guard_targets = fold_guard["targets"].as_sequence().unwrap();
    let guarded = |group: &Group, case: &Case| {
        group.only == fold_guard["only"].as_str().unwrap()
            && guard_targets
                .iter()
                .any(|target| target.as_str() == Some(group.target.as_str()))
            && fold_guard["modes"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|mode| mode.as_str() == Some(group.mode.as_str()))
            && fold_guard["endings"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|ending| ending.as_str() == Some(group.ending.as_str()))
            && guard_cases
                .iter()
                .any(|id| id.as_str() == Some(case.id.as_str()))
    };
    let mut guarded_known = BTreeSet::new();
    let mut known = BTreeSet::new();
    let mut failures = Vec::new();
    for group in &groups {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(".rubocop.yml"), &group.config).unwrap();
        for case in &group.files {
            fs::write(directory.path().join(&case.path), case.source.as_bytes()).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command.current_dir(directory.path()).args([
            "--cache",
            "false",
            "--no-parallel",
            "--format",
            "json",
            "--only",
            &group.only,
        ]);
        if group.mode != "inspect" {
            command.arg(&group.mode);
        }
        command.args(group.files.iter().map(|case| &case.path));
        let output = command.output().unwrap();
        let found: Value = serde_json::from_slice(without_cache_parallel_notice(&output.stdout))
            .unwrap_or_else(|error| {
                panic!(
                    "{}: {error}: {}",
                    group.id,
                    String::from_utf8_lossy(&output.stderr)
                )
            });
        for field in ["inspected_file_count", "target_file_count"] {
            assert_eq!(
                found["summary"][field],
                group.files.len(),
                "{} {field}",
                group.id
            );
        }
        let files: HashMap<_, _> = found["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| (file["path"].as_str().unwrap(), &file["offenses"]))
            .collect();
        assert_eq!(files.len(), group.files.len(), "{}", group.id);
        let expected_exit = if group.files.iter().any(|case| guarded(group, case)) {
            fold_guard["candidate_exit"].as_i64().unwrap() as i32
        } else {
            group.exit
        };
        if output.status.code() != Some(expected_exit) {
            failures.push(format!(
                "{}: exit {:?} / {}",
                group.id,
                output.status.code(),
                expected_exit
            ));
        }
        for case in &group.files {
            let intentional = divergences
                .iter()
                .find(|entry| entry.case == case.id && entry.targets.contains(&group.target));
            let expected = if guarded(group, case) {
                // 旧記録の本家 inspect 診断と原本を使う。候補の未補正出力から期待値を作らない。
                let original = groups
                    .iter()
                    .find(|reference| {
                        reference.only == group.only
                            && reference.target == group.target
                            && reference.ending == group.ending
                            && reference.mode == "inspect"
                    })
                    .unwrap()
                    .files
                    .iter()
                    .find(|original| original.id == case.id)
                    .unwrap();
                assert_eq!(case.source, original.source);
                assert_eq!(original.after, original.source);
                assert_ne!(case.after, original.source);
                assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
                    "Autocorrection was not written to {} because parentheses would change a regexp capture variable into a method call.",
                    case.path,
                )));
                assert!(guarded_known.insert((
                    case.id.clone(),
                    group.target.clone(),
                    group.ending.clone(),
                    group.mode.clone()
                )));
                original
            } else if let Some(intentional) = intentional {
                // 同じ入力の実 Prism 出力を使う。候補の出力を「物理 Ruby の期待値」にしてはならない。
                let reference = groups
                    .iter()
                    .find(|reference| {
                        reference.only == group.only
                            && reference.target == intentional.reference
                            && reference.ending == group.ending
                            && reference.mode == group.mode
                    })
                    .unwrap();
                let physical = reference
                    .files
                    .iter()
                    .find(|file| file.id == case.id)
                    .unwrap();
                assert_eq!(case.source, physical.source);
                assert!(
                    case.offenses != physical.offenses || case.after != physical.after,
                    "{}/{}: 既知差分が解消している",
                    group.id,
                    case.id
                );
                assert!(known.insert((
                    case.id.clone(),
                    group.target.clone(),
                    group.ending.clone(),
                    group.mode.clone()
                )));
                physical
            } else {
                case
            };
            let mut offenses: Vec<Value> = serde_json::from_value(
                (**files
                    .get(case.path.as_str())
                    .unwrap_or_else(|| panic!("{}: missing {}", group.id, case.path)))
                .clone(),
            )
            .unwrap();
            offenses.sort_by_cached_key(Value::to_string);
            let mut expected_offenses = expected.offenses.clone();
            expected_offenses.sort_by_cached_key(Value::to_string);
            let after = fs::read(directory.path().join(&case.path)).unwrap();
            let rewritten = !guarded(group, case)
                && group.mode != "inspect"
                && expected_offenses
                    .iter()
                    .any(|offense| offense["corrected"] == true);
            let expected_after = recorded_disk_bytes(
                case.source.as_bytes(),
                expected.after.as_bytes(),
                rewritten,
                0,
            );
            if offenses != expected_offenses || after != expected_after {
                failures.push(format!(
                    "{}/{}\noffenses: {} / {}\nafter: {:?} / {:?}",
                    group.id,
                    case.id,
                    serde_json::to_string(&offenses).unwrap(),
                    serde_json::to_string(&expected_offenses).unwrap(),
                    String::from_utf8_lossy(&after),
                    String::from_utf8_lossy(&expected_after)
                ));
            }
        }
    }
    // 旧 #31 の72条件と別の24拒否を全IDs/版/改行/補正modeで取りこぼさず検査する。
    assert_eq!(guarded_known.len(), 24);
    for id in guard_cases {
        for target in guard_targets {
            for ending in ["lf", "crlf"] {
                for mode in ["-a", "-A"] {
                    assert!(guarded_known.contains(&(
                        id.as_str().unwrap().to_owned(),
                        target.as_str().unwrap().to_owned(),
                        ending.to_owned(),
                        mode.to_owned()
                    )));
                }
            }
        }
    }
    for entry in &divergences {
        for target in &entry.targets {
            for ending in ["lf", "crlf"] {
                for mode in ["inspect", "-a", "-A"] {
                    assert!(
                        known.contains(&(
                            entry.case.clone(),
                            target.clone(),
                            ending.to_owned(),
                            mode.to_owned()
                        )),
                        "未検証の既知差分: {}/{target}/{ending}/{mode}",
                        entry.case
                    );
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn invalid_character_boundary_is_a_separately_recorded_existing_difference() {
    let groups: Vec<Group> =
        serde_json::from_str(include_str!("fixtures/regexp_character_boundary.json")).unwrap();
    let manifest: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(include_str!("conformance/known_divergences.yml")).unwrap();
    let entries = manifest["regexp_character_boundary_divergences"]
        .as_sequence()
        .unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["case"].as_str(), Some("truncated_hex_escape"));
    assert!(
        entry["reason"]
            .as_str()
            .unwrap()
            .contains("物理lexerで無効なcharacter")
    );
    assert_eq!(entry["candidate_preserves_source"].as_bool(), Some(true));
    assert_eq!(entry["candidate_offenses"].as_u64(), Some(0));
    assert_eq!(groups.len(), 4);
    let mut checked = BTreeSet::new();
    for group in groups {
        assert_eq!(group.target, entry["target"].as_str().unwrap());
        assert_eq!(group.only, entry["only"].as_str().unwrap());
        assert_eq!(group.exit, 0);
        assert!(group.delegated_to_regexp_validation.is_empty());
        assert_eq!(group.files.len(), 1);
        let case = &group.files[0];
        assert_eq!(case.id, "truncated_hex_escape");
        assert_eq!(
            case.source,
            entry["original"]
                .as_str()
                .unwrap()
                .replace('\n', if group.ending == "crlf" { "\r\n" } else { "\n" })
        );
        // 本家が受理して実際に書いた bytes も保存する。既存 CRLF 差をここでは正規化しない。
        assert_eq!(case.offenses.len(), 1);
        assert_eq!(case.offenses[0]["cop_name"], "Layout/SpaceAroundOperators");
        assert_eq!(case.offenses[0]["corrected"], true);
        assert!(case.after.ends_with("p y + 1\n"));
        assert_ne!(case.after, case.source);
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(".rubocop.yml"), &group.config).unwrap();
        fs::write(directory.path().join(&case.path), case.source.as_bytes()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_sonicop"))
            .current_dir(directory.path())
            .args([
                "--cache",
                "false",
                "--no-parallel",
                "--format",
                "json",
                "--only",
                &group.only,
                &group.mode,
                &case.path,
            ])
            .output()
            .unwrap();
        let report: Value =
            serde_json::from_slice(without_cache_parallel_notice(&output.stdout)).unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            entry["candidate_exit"].as_i64()
        );
        assert_eq!(
            report["summary"],
            serde_json::json!({"offense_count": 0, "target_file_count": 1, "inspected_file_count": 1})
        );
        assert_eq!(
            report["files"],
            serde_json::json!([{"path":case.path,"offenses":[]} ])
        );
        assert_eq!(
            fs::read(directory.path().join(&case.path)).unwrap(),
            case.source.as_bytes()
        );
        assert!(checked.insert((group.ending, group.mode)));
    }
    assert_eq!(
        checked,
        BTreeSet::from([
            ("lf".to_owned(), "-a".to_owned()),
            ("lf".to_owned(), "-A".to_owned()),
            ("crlf".to_owned(), "-a".to_owned()),
            ("crlf".to_owned(), "-A".to_owned())
        ])
    );
}
