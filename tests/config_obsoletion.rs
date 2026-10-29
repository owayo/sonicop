//! fatal renameの順と初期phaseを、実CLIのraw・原本・outで照合する。

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use std::time::SystemTime;

use serde_json::Value;

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|part| u8::from_str_radix(std::str::from_utf8(part).unwrap(), 16).unwrap())
        .collect()
}

fn stderr_bytes(hex: &str) -> Vec<u8> {
    let raw = bytes(hex);
    if cfg!(windows) {
        raw
    } else {
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn inspect(case: &Value) {
    let root = tempfile::tempdir().unwrap();
    let mut before = BTreeMap::<String, (Vec<u8>, SystemTime)>::new();
    for (path, text) in case["files"].as_object().unwrap() {
        let data = text.as_str().unwrap().as_bytes();
        fs::create_dir_all(root.path().join(path).parent().unwrap()).unwrap();
        fs::write(root.path().join(path), data).unwrap();
        before.insert(
            path.clone(),
            (
                data.to_vec(),
                fs::metadata(root.path().join(path))
                    .unwrap()
                    .modified()
                    .unwrap(),
            ),
        );
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command.current_dir(root.path());
    for key in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        command.env_remove(key);
    }
    for (key, value) in case["env"].as_object().unwrap() {
        command.env(key, value.as_str().unwrap());
    }
    command.args(
        case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap()),
    );
    let result = command.output().unwrap();
    let id = case["id"].as_str().unwrap();
    assert_eq!(
        root.path().join("report.json").exists(),
        case["out_exists"].as_bool().unwrap(),
        "{id}"
    );
    assert_eq!(
        result.status.code().map(i64::from),
        case["exit"].as_i64(),
        "{id}"
    );
    #[cfg(windows)]
    let display_root = root.path().to_owned();
    #[cfg(not(windows))]
    let display_root = fs::canonicalize(root.path()).unwrap();
    let expected = String::from_utf8(stderr_bytes(case["stderr_hex"].as_str().unwrap())).unwrap();
    assert_eq!(
        result.stderr,
        expected
            .replace("<ROOT>", &display_root.to_string_lossy().replace('\\', "/"))
            .as_bytes(),
        "{id}"
    );
    if case["exit"] == 2 {
        let expected = bytes(case["stdout_hex"].as_str().unwrap());
        assert!(expected.is_empty(), "{id}");
        assert_eq!(result.stdout, expected, "{id}");
    } else {
        let mut report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["metadata"]["rubocop_version"], "1.89.0");
        assert_eq!(report["metadata"]["ruby_engine"], "sonicop");
        report.as_object_mut().unwrap().remove("metadata");
        let mut expected: Value =
            serde_json::from_slice(&bytes(case["stdout_hex"].as_str().unwrap())).unwrap();
        expected.as_object_mut().unwrap().remove("metadata");
        assert_eq!(report, expected, "{id}");
    }
    for (path, (data, modified)) in before {
        assert_eq!(
            fs::read(root.path().join(&path)).unwrap(),
            data,
            "{id}/{path}"
        );
        assert_eq!(
            fs::metadata(root.path().join(&path))
                .unwrap()
                .modified()
                .unwrap(),
            modified,
            "{id}/{path}"
        );
    }
}

#[test]
fn fatal_renames_match_the_measured_initial_cli_phases() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/config_obsoletion.json")).unwrap();
    assert_eq!(fixture["schema"], 1);
    let cases = fixture["cases"].as_array().unwrap();
    let ids: Vec<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "full-disabled",
            "full-ignore",
            "bare-disabled",
            "moved",
            "two-reversed",
            "parent",
            "parent-overridden",
            "full-null",
            "bare-null",
            "early-missing-target",
            "version",
            "verbose-version",
            "force-default",
            "out-sentinel",
            "color-flag",
            "force-color",
            "force-no-color",
            "namespace-first",
            "parameter-suppressed",
            "canonical-negative",
            "full-enabled",
            "old-scalar"
        ]
    );
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        22
    );
    for case in cases {
        inspect(case);
    }
}

#[test]
fn fatal_renames_precede_unknowns_and_preserve_completed_warnings() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/config_obsoletion_order.json")).unwrap();
    assert_eq!(fixture["schema"], 1);
    let cases = fixture["cases"].as_array().unwrap();
    let ids: Vec<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "fatal-before-unknown-force-0",
            "fatal-before-unknown-force-1",
            "completed-warning-before-fatal-and-missing-force-0",
            "completed-warning-before-fatal-and-missing-force-1"
        ]
    );
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        4
    );
    for case in cases {
        inspect(case);
    }
}
