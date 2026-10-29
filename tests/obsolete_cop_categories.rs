//! 初期拒否と途中拒否の40CLIを、元の実参照で照合する。

use std::fs;
use std::process::Command;

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

fn native_text(raw: Vec<u8>) -> Vec<u8> {
    if cfg!(windows) {
        raw
    } else {
        // 参照のpipe transportだけをhostへ合わせ、原本byteには適用しない。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn inspect_case(case: &Value) {
    let directory = tempfile::tempdir().unwrap();
    let mut originals = Vec::new();
    for (name, text) in case["files"].as_object().unwrap() {
        let path = directory.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text.as_str().unwrap().as_bytes()).unwrap();
        originals.push((
            path.clone(),
            fs::read(&path).unwrap(),
            fs::metadata(&path).unwrap().modified().unwrap(),
        ));
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command.current_dir(directory.path());
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        command.env_remove(key);
    }
    command.env("CLICOLOR_FORCE", "0");
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
        output.status.code(),
        Some(case["exit"].as_i64().unwrap() as i32),
        "{}: {output:?}",
        case["id"]
    );
    assert_eq!(
        output.stderr,
        native_text(bytes(case["stderr_hex"].as_str().unwrap())),
        "{}",
        case["id"]
    );
    if case["phase"] == "late_rejection"
        || matches!(
            case["id"].as_str(),
            Some("canonical-negative" | "unaffected-department")
        )
    {
        let mut expected: Value =
            serde_json::from_slice(&bytes(case["stdout_hex"].as_str().unwrap())).unwrap();
        let mut actual: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(actual["metadata"]["ruby_engine"], "sonicop");
        assert_eq!(actual["metadata"]["rubocop_version"], "1.89.0");
        actual.as_object_mut().unwrap().remove("metadata");
        expected.as_object_mut().unwrap().remove("metadata");
        assert_eq!(actual, expected);
    } else {
        assert_eq!(
            output.stdout,
            bytes(case["stdout_hex"].as_str().unwrap()),
            "{}",
            case["id"]
        );
    }
    assert_eq!(
        directory.path().join("report.json").exists(),
        case["out_exists"].as_bool().unwrap(),
        "{}",
        case["id"]
    );
    for (path, original, modified) in originals {
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "{}: {}",
            case["id"],
            path.display()
        );
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            modified,
            "{}: {}",
            case["id"],
            path.display()
        );
    }
}

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/obsolete_cop_categories.json")).unwrap();
    assert_eq!(fixture["schema"], 1);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 40);
    let ids: Vec<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "catalogue-00",
            "catalogue-01",
            "catalogue-02",
            "catalogue-03",
            "catalogue-04",
            "catalogue-05",
            "catalogue-06",
            "catalogue-07",
            "catalogue-08",
            "catalogue-09",
            "catalogue-10",
            "catalogue-11",
            "catalogue-12",
            "catalogue-13",
            "catalogue-14",
            "catalogue-15",
            "removed-bare",
            "split-bare",
            "removed-null",
            "split-null",
            "extracted-null",
            "removed-ignore",
            "split-ignore",
            "extracted-ignore",
            "extracted-one",
            "extracted-many",
            "extracted-department",
            "rules-order",
            "namespace-first",
            "parameter-suppressed",
            "force-color",
            "color-flag",
            "force-no-color",
            "inherit-parent",
            "inherit-overridden",
            "nested-discovery",
            "inherited-namespace",
            "out-sentinel",
            "canonical-negative",
            "unaffected-department"
        ]
    );
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        40
    );
    assert_eq!(
        cases
            .iter()
            .filter(|case| case["coverage"] == "strict")
            .count(),
        40
    );
    assert_eq!(
        cases
            .iter()
            .filter(|case| case["coverage"] == "deferred_152")
            .count(),
        0
    );
    fixture
}

#[test]
fn initial_categories_match_upstream_before_output_and_source_changes() {
    let fixture = fixture();
    let cases: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["coverage"] == "strict")
        .collect();
    assert_eq!(cases.len(), 40);
    for case in cases {
        inspect_case(case);
    }
}
