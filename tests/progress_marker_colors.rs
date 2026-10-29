//! 本家の全streamと履歴severity色を照合し、物理原本の保持とは分けて検査する。
use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn decode_hex(value: &str) -> Vec<u8> {
    let (pairs, remainder) = value.as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn expected_text(hex: &str) -> Vec<u8> {
    let raw = decode_hex(hex);
    if cfg!(windows) {
        raw
    } else {
        // Windows実録のtext transportだけを移し、ANSIと物理source byteは保つ。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn read_physical(root: &Path, name: &str) -> Vec<u8> {
    std::fs::read(root.join(name)).unwrap()
}

#[test]
fn progress_markers_match_real_color_and_correction_states() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/progress_marker_colors.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(fixture["runtime"]["ruby"], "4.0.7");
    assert_eq!(fixture["runtime"]["gems"]["rubocop"], "1.89.0");
    assert_eq!(
        cases
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "info",
            "refactor",
            "convention",
            "warning",
            "error",
            "fatal",
            "clean",
            "corrected-C",
            "mixed-corrected-high",
            "mixed-files",
            "no-color",
            "forced",
            "forced-no-color",
            "stderr",
            "cache",
            "shared-simple",
            "separate-simple",
            "plain"
        ]
    );
    let mut executions = 0;
    for case in cases {
        let name = case["id"].as_str().unwrap();
        assert_eq!(case["coverage"], "full_bytes");
        assert_eq!(case["run_count"], if name == "cache" { 2 } else { 1 });
        assert_eq!(
            case["runs"].as_array().unwrap().len(),
            if name == "cache" { 2 } else { 1 }
        );
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut stable = Vec::new();
        let mut permissions = std::collections::BTreeMap::new();
        for (filename, value) in case["files"].as_object().unwrap() {
            std::fs::write(root.join(filename), value.as_str().unwrap()).unwrap();
            permissions.insert(
                filename.clone(),
                std::fs::metadata(root.join(filename))
                    .unwrap()
                    .permissions(),
            );
            if decode_hex(case["runs"][0]["after_hex"][filename].as_str().unwrap())
                == value.as_str().unwrap().as_bytes()
            {
                stable.push((
                    filename.clone(),
                    read_physical(root, filename),
                    std::fs::metadata(root.join(filename))
                        .unwrap()
                        .modified()
                        .unwrap(),
                ));
            }
        }
        for (index, run) in case["runs"].as_array().unwrap().iter().enumerate() {
            let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
            command
                .current_dir(root)
                .env("XDG_CACHE_HOME", root.join("cache"))
                .env("TERM", "dumb")
                .env("PATH", "");
            for key in [
                "RUBYOPT",
                "RUBYLIB",
                "RUBOCOP_OPTS",
                "RUBOCOP_TARGET_RUBY_VERSION",
                "NO_COLOR",
            ] {
                command.env_remove(key);
            }
            for (key, value) in case["env"].as_object().unwrap() {
                command.env(key, value.as_str().unwrap());
            }
            for argument in case["args"].as_array().unwrap() {
                command.arg(argument.as_str().unwrap());
            }
            let output = command.output().unwrap();
            assert_eq!(
                output.status.code(),
                run["exit"].as_i64().map(|n| n as i32),
                "{name} run{index}"
            );
            assert_eq!(
                output.stdout,
                expected_text(run["stdout_hex"].as_str().unwrap()),
                "{name} run{index} stdout"
            );
            assert_eq!(
                output.stderr,
                expected_text(run["stderr_hex"].as_str().unwrap()),
                "{name} run{index} stderr"
            );
            for (filename, bytes) in run["artifacts_hex"].as_object().unwrap() {
                assert_eq!(
                    read_physical(root, filename),
                    expected_text(bytes.as_str().unwrap()),
                    "{name} {filename}"
                );
            }
            if !run["artifacts_hex"]
                .as_object()
                .unwrap()
                .contains_key("progress.txt")
            {
                assert!(
                    matches!(std::fs::symlink_metadata(root.join("progress.txt")), Err(error) if error.kind()==std::io::ErrorKind::NotFound)
                );
            }
            for (filename, bytes) in run["after_hex"].as_object().unwrap() {
                // CRLF原本の保持方針は既存のもの。formatterのtext変換と混ぜない。
                assert_eq!(
                    read_physical(root, filename),
                    decode_hex(bytes.as_str().unwrap()),
                    "{name} physical {filename}"
                );
                assert_eq!(
                    std::fs::metadata(root.join(filename))
                        .unwrap()
                        .permissions(),
                    permissions[filename]
                );
            }
            for (filename, bytes, modified) in &stable {
                assert_eq!(read_physical(root, filename), *bytes, "{name} stable bytes");
                assert_eq!(
                    std::fs::metadata(root.join(filename))
                        .unwrap()
                        .modified()
                        .unwrap(),
                    *modified,
                    "{name} stable mtime"
                );
            }
            executions += 1;
        }
    }
    assert_eq!(executions, 19);
}
