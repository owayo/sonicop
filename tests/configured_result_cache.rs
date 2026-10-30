//! for_pwdのUseCacheとCLI明示値を、実reportとディスク上の結果cacheで照合する。
//! 入れ子でもcop選択を明示し、native改行のOS差をcache参照へ混ぜない。

use std::{fs, process::Command};

use serde_json::Value;

fn bytes(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap().as_bytes();
    let (pairs, rest) = text.as_chunks::<2>();
    assert!(rest.is_empty());
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn host(raw: Vec<u8>) -> Vec<u8> {
    if cfg!(windows) {
        raw
    } else {
        // 本家Windowsのreport輸送だけを投影し、Ruby/config原本を変換しない。
        String::from_utf8(raw)
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn report(raw: &[u8], engine: &str) -> Value {
    assert_eq!(raw.last(), Some(&b'}'), "compact JSONに終端改行を足さない");
    let mut value: Value = serde_json::from_slice(raw).unwrap();
    assert_eq!(value["metadata"]["ruby_engine"], engine);
    assert_eq!(value["metadata"]["rubocop_version"], "1.89.0");
    if engine == "sonicop" {
        assert_eq!(
            value["metadata"]["sonicop_version"],
            env!("CARGO_PKG_VERSION")
        );
    } else {
        assert_eq!(value["metadata"]["ruby_version"], "4.0.7");
    }
    value.as_object_mut().unwrap().remove("metadata");
    value
}

#[test]
fn configured_cache_obeys_root_truthiness_and_explicit_cli_overrides() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/configured_result_cache.json")).unwrap();
    assert_eq!(fixture["schema"], 1);
    assert_eq!(fixture["coverage"]["subset"], false);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 21);
    let (mut enabled, mut disabled, mut original_observations) = (0, 0, 0);
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut originals = Vec::new();
        for (name, text) in case["files"].as_object().unwrap() {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text.as_str().unwrap().as_bytes()).unwrap();
            let info = fs::metadata(&path).unwrap();
            originals.push((path.clone(), fs::read(&path).unwrap(), info));
        }
        assert!(!root.join("cache").exists());
        let results = case["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        for (repeat, expected) in results.iter().enumerate() {
            let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
            command
                .current_dir(root)
                .env("TERM", "dumb")
                .env("CLICOLOR_FORCE", "0")
                .env("XDG_CACHE_HOME", root.join("cache"))
                .env("PATH", "");
            for key in [
                "RUBYOPT",
                "RUBYLIB",
                "RUBOCOP_OPTS",
                "RUBOCOP_TARGET_RUBY_VERSION",
                "RUBOCOP_CACHE_ROOT",
                "NO_COLOR",
                "SONICOP_PROFILE",
                "SONICOP_RUNTIME_RUBY",
            ] {
                command.env_remove(key);
            }
            command.args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap()),
            );
            let output = command.output().unwrap();
            let id = case["id"].as_str().unwrap();
            assert_eq!(
                output.status.code().map(i64::from),
                expected["exit"].as_i64(),
                "{id}/{repeat}"
            );
            assert_eq!(
                output.stderr,
                host(bytes(&expected["stderr_hex"])),
                "{id}/{repeat}"
            );
            let wanted = host(bytes(&expected["stdout_hex"]));
            let start = wanted.iter().position(|byte| *byte == b'{').unwrap();
            assert!(output.stdout.starts_with(&wanted[..start]), "{id}/{repeat}");
            assert_eq!(
                report(&output.stdout[start..], "sonicop"),
                report(&wanted[start..], "ruby"),
                "{id}/{repeat} report"
            );
            let cache_exists = expected["cache_exists"].as_bool().unwrap();
            assert_eq!(
                fs::symlink_metadata(root.join("cache")).is_ok(),
                cache_exists,
                "{id}/{repeat}"
            );
            assert!(
                fs::symlink_metadata(root.join("report.json")).is_err(),
                "{id}/{repeat}"
            );
            if cache_exists {
                enabled += 1;
                // 保存形式は両engineで異なる。Nativeの実indexに対象の結果があることを別に確かめる。
                let indexes: Vec<_> = fs::read_dir(root.join("cache/sonicop"))
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == "index")
                    })
                    .collect();
                assert_eq!(indexes.len(), 1, "{id}/{repeat}");
                let index: Value = serde_json::from_slice(&fs::read(&indexes[0]).unwrap()).unwrap();
                assert_eq!(
                    index["entries"].as_object().unwrap().len(),
                    1,
                    "{id}/{repeat}"
                );
            } else {
                disabled += 1;
            }
            for (path, raw, before) in &originals {
                assert_eq!(&fs::read(path).unwrap(), raw, "{id}/{repeat} source");
                let after = fs::metadata(path).unwrap();
                assert_eq!(
                    after.modified().unwrap(),
                    before.modified().unwrap(),
                    "{id}/{repeat} mtime"
                );
                assert_eq!(
                    after.permissions(),
                    before.permissions(),
                    "{id}/{repeat} mode"
                );
                original_observations += 1;
            }
        }
    }
    assert_eq!((enabled, disabled, original_observations), (26, 16, 184));
}
