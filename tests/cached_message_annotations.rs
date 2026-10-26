use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

fn decode(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
        .collect()
}

fn snapshot(paths: impl IntoIterator<Item = PathBuf>) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    paths
        .into_iter()
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            (path, (bytes, modified))
        })
        .collect()
}

fn cache_snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    snapshot(
        fs::read_dir(root.join("cache"))
            .into_iter()
            .flatten()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file()),
    )
}

#[test]
fn cached_annotation_histories_match_recorded_rubocop() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/cached_message_annotations_windows.json"
    ))
    .unwrap();
    for sequence in fixture["sequences"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(
            root.join("config.yml"),
            decode(fixture["configuration_hex"].as_str().unwrap()),
        )
        .unwrap();
        let sources = if sequence["name"] == "mixed-histories" {
            vec![root.join("first.rb"), root.join("second.rb")]
        } else {
            vec![root.join("case.rb")]
        };
        for source in &sources {
            fs::write(
                source,
                decode(fixture["initial_source_hex"].as_str().unwrap()),
            )
            .unwrap();
        }
        for (index, step) in sequence["steps"].as_array().unwrap().iter().enumerate() {
            let label = format!("{} / {index}", sequence["name"]);
            if let Some(action) = step["action"].as_str() {
                let path = root.join("case.rb");
                let modified = fs::metadata(&path).unwrap().modified().unwrap();
                let modified = match action {
                    "touch" => modified + Duration::from_secs(2),
                    "body-same-stat" => {
                        fs::write(&path, b"y=2\n").unwrap();
                        modified
                    }
                    other => panic!("unknown recorded action: {other}"),
                };
                fs::OpenOptions::new()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(modified))
                    .unwrap();
            }
            let before = snapshot(sources.iter().cloned().chain([root.join("config.yml")]));
            let cache_before = cache_snapshot(root);
            let arguments: Vec<_> = step["arguments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|argument| argument.as_str().unwrap())
                .collect();
            let output = Command::new(env!("CARGO_BIN_EXE_sonicop"))
                .current_dir(root)
                .env_remove("RUBOCOP_OPTS")
                .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
                .args(arguments)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(step["exit"].as_i64().unwrap() as i32),
                "{label}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            // debug の走査ログは本家 stdout / native stderr の既存差。注釈本体を下で照合する。
            if !step["flags"]
                .as_array()
                .unwrap()
                .iter()
                .any(|flag| flag == "--debug")
            {
                assert_eq!(
                    String::from_utf8(output.stderr).unwrap(),
                    step["stderr"].as_str().unwrap(),
                    "{label}"
                );
            }
            for (output_index, expected) in step["outputs"].as_array().unwrap().iter().enumerate() {
                let body = if expected["channel"] == "stdout" {
                    output.stdout.clone()
                } else {
                    fs::read(root.join(format!("report-{output_index}.out"))).unwrap()
                };
                if !expected["report"].is_null() {
                    let actual: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(actual["files"], expected["report"]["files"], "{label}");
                    assert_eq!(actual["summary"], expected["report"]["summary"], "{label}");
                } else {
                    // Windows のテキスト出力改行は別検証。ここでは注釈と並びを比べる。
                    let actual = String::from_utf8(body).unwrap().replace("\r\n", "\n");
                    let expected =
                        String::from_utf8(decode(expected["text_hex"].as_str().unwrap()))
                            .unwrap()
                            .replace("\r\n", "\n");
                    assert_eq!(actual, expected, "{label}");
                }
            }
            assert_eq!(
                snapshot(sources.iter().cloned().chain([root.join("config.yml")])),
                before,
                "{label}"
            );
            let cache_after = cache_snapshot(root);
            if step["action"] == "touch" {
                // 本文が同じなら名前空間を増やさず、native の stat 更新だけを許す。
                assert_eq!(
                    cache_before.keys().collect::<Vec<_>>(),
                    cache_after.keys().collect::<Vec<_>>(),
                    "{label}"
                );
            } else {
                assert_eq!(
                    cache_before == cache_after,
                    step["cache_unchanged"].as_bool().unwrap(),
                    "{label}"
                );
            }
            for path in cache_after.keys().filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "index")
            }) {
                let document: serde_json::Value =
                    serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                for entry in document["entries"].as_object().unwrap().values() {
                    for offense in entry["report"]["offenses"].as_array().unwrap() {
                        // API と保存済み raw message に表示注釈を混ぜない。
                        assert_eq!(
                            offense["message"], "Surrounding space missing for operator `=`.",
                            "{label}"
                        );
                    }
                }
            }
        }
    }
}
