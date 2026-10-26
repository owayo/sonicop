//! 複数ファイルの索引を必要とする cop を、本家 CLI が記録した全項目で照合する。

mod support;

use std::fs;

use serde_json::Value;
use support::project::command;

fn check_recorded_cases(cop: &str, count: usize) {
    let mut cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/project_index_cases.json")).unwrap();
    cases.extend(
        serde_json::from_str::<Vec<Value>>(include_str!("fixtures/path_cop_cases.json")).unwrap(),
    );
    let mut checked = 0;
    let mut reported = 0;
    for case in cases.iter().filter(|case| case["cop"] == cop) {
        let directory = tempfile::tempdir().unwrap();
        let files = case["files"].as_object().unwrap();
        for (name, source) in files {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source.as_str().unwrap()).unwrap();
        }
        fs::write(
            directory.path().join(".rubocop.yml"),
            serde_json::to_vec(&case["config"]).unwrap(),
        )
        .unwrap();
        let mut expected = case["expected"].clone();
        // 本家のメッセージに含まれる絶対パスだけを実行先に合わせる。
        let canonical = directory
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        // Windows の canonicalize が付ける拡張長接頭辞は、索引の表示パスに含まれない。
        let root = canonical.strip_prefix("//?/").unwrap_or(&canonical);
        // GemFilename の File.expand_path は Windows の短縮名を長い名へ変えない。
        // 索引由来の他 cop と分け、この記録だけ child の cwd に渡す表記を使う。
        #[cfg(windows)]
        let supplied_root = directory.path().to_string_lossy().replace('\\', "/");
        #[cfg(windows)]
        let root = if cop == "Bundler/GemFilename" {
            supplied_root.as_str()
        } else {
            root
        };
        for file in expected.as_array_mut().unwrap() {
            for offense in file["offenses"].as_array_mut().unwrap() {
                // 本家の Windows CLI も message の絶対パスを / で返す。
                let message = offense["message"].as_str().unwrap().to_owned();
                offense["message"] = message.replace("{root}", root).into();
            }
        }
        let expected = expected.as_array().unwrap();
        let offense_count: usize = expected
            .iter()
            .map(|file| file["offenses"].as_array().unwrap().len())
            .sum();
        let targets: Vec<_> = case["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| target.as_str().unwrap())
            .collect();
        // Ruby の実行ファイルがなくても索引と診断が完結することを確かめる。
        let mut process = command(directory.path());
        process
            .env("PATH", "")
            .args(["--cache", "false", "--only", cop, "--format", "json"]);
        if let Some(source) = case["stdin"].as_str() {
            process.args(["--stdin", targets[0]]).write_stdin(source);
        } else {
            process.args(&targets);
        }
        let output = process
            .assert()
            .code(if offense_count == 0 { 0 } else { 1 })
            .get_output()
            .stdout
            .clone();
        let report: Value = serde_json::from_slice(&output).unwrap();
        let origin = case["origin"].as_str().unwrap();
        assert_eq!(
            report["files"].as_array().unwrap(),
            expected,
            "{cop}: {origin}"
        );
        assert_eq!(report["summary"]["target_file_count"], targets.len());
        assert_eq!(report["summary"]["inspected_file_count"], targets.len());
        for (name, source) in files {
            assert_eq!(
                fs::read(directory.path().join(name)).unwrap(),
                source.as_str().unwrap().as_bytes(),
                "検査で入力を書き換えた: {name}"
            );
        }
        checked += 1;
        reported += offense_count;
    }
    assert_eq!(checked, count);
    assert!(reported > 0, "沈黙だけでは cop の実装を検証できない");
}

#[test]
fn deprecated_references_match_upstream_with_a_project_index() {
    check_recorded_cases("Lint/DeprecatedReference", 32);
}

#[test]
fn name_typos_match_upstream_with_a_project_index() {
    check_recorded_cases("Lint/NameTypo", 42);
}

#[test]
fn unused_private_methods_match_upstream_with_a_project_index() {
    check_recorded_cases("Lint/UnusedPrivateMethod", 25);
}

#[test]
fn filename_dependent_cops_match_recorded_upstream() {
    check_recorded_cases("Bundler/GemFilename", 1);
    check_recorded_cases("Lint/RequireRelativeSelfPath", 1);
}

#[test]
fn existing_cops_consult_the_same_index_as_upstream() {
    for (cop, count) in [
        ("Lint/ConstantReassignment", 7),
        ("Lint/ConstantResolution", 3),
        ("Lint/DuplicateMethods", 13),
        ("Lint/InheritException", 7),
        ("Lint/MissingSuper", 9),
        ("Naming/AccessorMethodName", 2),
        ("Naming/PredicatePrefix", 2),
        ("Style/ClassAndModuleChildren", 13),
        ("Style/Documentation", 6),
        ("Style/MissingRespondToMissing", 6),
        ("Style/RedundantConstantBase", 4),
        ("Style/StaticClass", 3),
    ] {
        check_recorded_cases(cop, count);
    }
}

#[test]
fn cached_verdicts_follow_references_changed_in_another_file() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/project_index_cases.json")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    for phase in [
        "cache-unreferenced",
        "cache-referenced",
        "cache-unreferenced",
    ] {
        let origin = format!("constructed/{phase}");
        let case = cases.iter().find(|case| case["origin"] == origin).unwrap();
        for (name, source) in case["files"].as_object().unwrap() {
            let path = directory.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            // 対象ファイルの stat は保ち、参照元の変更だけで前の判定が古くなる状況を作る。
            if !path.exists() || name != "lib/current.rb" {
                fs::write(path, source.as_str().unwrap()).unwrap();
            }
        }
        let config = directory.path().join(".rubocop.yml");
        if !config.exists() {
            fs::write(config, serde_json::to_vec(&case["config"]).unwrap()).unwrap();
        }
        let output = command(directory.path())
            .args([
                "--cache",
                "true",
                "--only",
                "Lint/UnusedPrivateMethod",
                "-f",
                "json",
                "lib/current.rb",
            ])
            .args([
                "--cache-root",
                directory.path().join("cache").to_str().unwrap(),
            ])
            .assert()
            .code(if phase == "cache-referenced" { 0 } else { 1 })
            .get_output()
            .stdout
            .clone();
        let report: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(report["files"], case["expected"], "{phase}");
    }
}

#[cfg(unix)]
#[test]
fn bundled_sources_are_consulted_only_when_enabled() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let project = directory.path().join("project");
    fs::create_dir(&project).unwrap();
    let external = directory.path().join("external.rb");
    fs::write(
        &external,
        "# @deprecated Prefer NewApi.\nclass OldApi; end\n",
    )
    .unwrap();
    fs::write(project.join("current.rb"), "OldApi\n").unwrap();
    let interpreter = directory.path().join("bundle-query");
    let marker = directory.path().join("queried");
    let report = serde_json::json!({"files": [external]}).to_string();
    // gem のファイルはプロジェクト外に置く。内部に置くと通常の索引だけでも見つかってしまう。
    // 応答は Ruby/Bundler との境界で代替し、通常の Rust テストには Ruby を要求しない。
    fs::write(
        &interpreter,
        "#!/bin/sh\nprintf invoked > \"$SONICOP_QUERY_MARKER\"\nprintf '\\000sonicop-bundle-index\\000%s' \"$SONICOP_QUERY_REPORT\"\n",
    )
    .unwrap();
    fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o700)).unwrap();
    for enabled in [false, true] {
        let config = serde_json::json!({
            "AllCops": {"UseProjectIndex": true, "ProjectIndexIncludesGems": enabled, "NewCops": "disable"},
            "Lint/DeprecatedReference": {"Enabled": true}
        });
        fs::write(project.join(".rubocop.yml"), config.to_string()).unwrap();
        let output = command(&project)
            .env("SONICOP_BUNDLER_RUBY", &interpreter)
            .env("SONICOP_QUERY_MARKER", &marker)
            .env("SONICOP_QUERY_REPORT", &report)
            .args([
                "--cache",
                "false",
                "--only",
                "Lint/DeprecatedReference",
                "-f",
                "json",
                "current.rb",
            ])
            .assert()
            .code(i32::from(enabled))
            .get_output()
            .stdout
            .clone();
        let report: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(report["summary"]["offense_count"], usize::from(enabled));
        assert_eq!(marker.exists(), enabled);
        if enabled {
            assert_eq!(
                report["files"][0]["offenses"][0]["message"],
                "Constant `OldApi` is deprecated: Prefer NewApi."
            );
        }
    }
}
