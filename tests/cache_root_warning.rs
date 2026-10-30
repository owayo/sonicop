//! CacheConfigの早期lookupと、force-default適用後の警告を実本家のrawで照合する。

#[cfg(windows)]
#[path = "support/cache_root_paths.rs"]
mod cache_root_paths;

#[cfg(windows)]
fn decode(value: &serde_json::Value) -> Vec<u8> {
    let value = value.as_str().unwrap().as_bytes();
    let (value, remainder) = value.as_chunks::<2>();
    assert!(remainder.is_empty());
    value
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[cfg(windows)]
#[test]
fn aliases_initial_color_and_early_exits_match_thirteen_more_real_cli_runs() {
    use serde_json::Value;
    use std::{fs, process::Command};
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/cache_root_warning.json")).unwrap();
    let folder = tempfile::tempdir().unwrap();
    let bindings = cache_root_paths::setup(folder.path());
    let cache = std::path::Path::new(&bindings["LONG"]).join("0/rubocop_cache");
    let cache_before = fs::metadata(&cache).unwrap();
    assert_eq!(fixture["additional_cases"].as_array().unwrap().len(), 13);
    for case in fixture["additional_cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let cwd = std::path::Path::new(&cache_root_paths::bind("<WORK>", &bindings)).join(id);
        fs::create_dir(&cwd).unwrap();
        let mut configured =
            cache_root_paths::root(case["configured_kind"].as_str().unwrap(), &bindings);
        let desired = cache_root_paths::root(case["desired_kind"].as_str().unwrap(), &bindings);
        if case["append_uid"].as_bool().unwrap() {
            configured.push_str("/0");
        }
        let config = case["config_template"]
            .as_str()
            .unwrap()
            .replace("<CONFIG_ROOT>", &configured.replace('\'', "''"));
        let input = decode(&case["input_hex"]);
        let gemfile = decode(&case["gemfile_hex"]);
        let sources = [
            (cwd.join("input.rb"), input),
            (cwd.join(".rubocop.yml"), config.into_bytes()),
            (cwd.join("Gemfile"), gemfile),
        ];
        let before: Vec<_> = sources
            .iter()
            .map(|(path, bytes)| {
                fs::write(path, bytes).unwrap();
                fs::metadata(path).unwrap()
            })
            .collect();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(&cwd)
            .env("PATH", "")
            .env("TERM", "dumb")
            .env("CLICOLOR_FORCE", "0")
            .env("XDG_CACHE_HOME", &desired)
            .env("XDG_CONFIG_HOME", folder.path().join("empty-config"));
        for key in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
            "RUBOCOP_CACHE_ROOT",
            "NO_COLOR",
        ] {
            command.env_remove(key);
        }
        for (key, value) in case["env"].as_object().unwrap() {
            command.env(key, value.as_str().unwrap());
        }
        for arg in case["args"].as_array().unwrap() {
            command.arg(arg.as_str().unwrap());
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            case["exit"].as_i64(),
            "{id}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let early = format!("{configured}/rubocop_cache");
        let desired = format!("{desired}/0/rubocop_cache");
        let stderr = case["stderr_template"]
            .as_str()
            .unwrap()
            .replace("<EARLY>", &early)
            .replace("<DESIRED>", &desired);
        assert_eq!(output.stderr, stderr.as_bytes(), "{id}");
        match case["kind"].as_str().unwrap() {
            "report" => {
                let mut found: Value = serde_json::from_slice(&output.stdout).unwrap();
                let mut expected: Value =
                    serde_json::from_slice(&decode(&case["stdout_hex"])).unwrap();
                assert_eq!(found["metadata"]["ruby_engine"], "sonicop");
                assert_eq!(expected["metadata"]["ruby_engine"], "ruby");
                assert_eq!(found["metadata"]["rubocop_version"], "1.89.0");
                for key in fixture["metadata_identity_keys"].as_array().unwrap() {
                    let key = key.as_str().unwrap();
                    found["metadata"].as_object_mut().unwrap().remove(key);
                    expected["metadata"].as_object_mut().unwrap().remove(key);
                }
                assert_eq!(found, expected, "{id}");
            }
            "version" => assert_eq!(
                output.stdout,
                format!("{}\r\n", sonicop::VERSION).as_bytes(),
                "{id}"
            ),
            "verbose-version" => {
                // 製品固有の版本文を本家のRuby/parser identityへ偽装しない。
                assert!(
                    String::from_utf8_lossy(&output.stdout)
                        .starts_with(&format!("sonicop {} (RuboCop 1.89 CLI,", sonicop::VERSION)),
                    "{id}"
                );
                assert!(output.stdout.ends_with(b"\r\n"));
            }
            "text" => assert_eq!(output.stdout, decode(&case["stdout_hex"]), "{id}"),
            kind => panic!("unexpected reference scope: {kind}"),
        }
        for ((path, bytes), before) in sources.iter().zip(before) {
            let after = fs::metadata(path).unwrap();
            assert_eq!(fs::read(path).unwrap(), *bytes, "{id}: {path:?}");
            assert_eq!(
                before.modified().unwrap(),
                after.modified().unwrap(),
                "{id}"
            );
            assert_eq!(before.permissions(), after.permissions(), "{id}");
        }
        assert!(!cwd.join("tmp").exists(), "{id}");
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0, "{id}");
        assert_eq!(
            fs::metadata(&cache).unwrap().modified().unwrap(),
            cache_before.modified().unwrap(),
            "{id}"
        );
        assert_eq!(
            fs::metadata(&cache).unwrap().permissions(),
            cache_before.permissions(),
            "{id}"
        );
    }
}

#[cfg(windows)]
#[test]
fn cache_root_warning_matches_six_recorded_windows_cli_runs() {
    use std::fs;
    use std::process::Command;

    use serde_json::Value;

    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/cache_root_warning.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 6);
    for case in fixture["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let xdg = folder.path().join("state/xdg-cache");
        let env_root = folder.path().join("state/environment-cache");
        let tmp = folder.path().join("state/tmp");
        fs::create_dir_all(&tmp).unwrap();
        let xdg = xdg.to_str().unwrap();
        let config = case["config"].as_str().unwrap().replace("<XDG>", xdg);
        let (input_pairs, remainder) = case["input_hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>();
        assert!(remainder.is_empty());
        let input: Vec<u8> = input_pairs
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        let source_path = folder.path().join("input.rb");
        let config_path = folder.path().join(".rubocop.yml");
        let gemfile_path = folder.path().join("Gemfile");
        fs::write(&source_path, &input).unwrap();
        fs::write(&config_path, &config).unwrap();
        fs::write(&gemfile_path, b"").unwrap();
        let source_before = fs::metadata(&source_path).unwrap();
        let config_before = fs::metadata(&config_path).unwrap();
        let gemfile_before = fs::metadata(&gemfile_path).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(folder.path())
            .env_remove("RUBYOPT")
            .env_remove("RUBYLIB")
            .env_remove("RUBOCOP_OPTS")
            .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
            .env_remove("RUBOCOP_CACHE_ROOT")
            .env_remove("NO_COLOR")
            .env("CLICOLOR_FORCE", "0")
            .env("PATH", "")
            .env("TERM", "dumb")
            .env("XDG_CACHE_HOME", xdg)
            .env("XDG_CONFIG_HOME", folder.path().join("state/config"))
            .env("TMP", &tmp)
            .env("TEMP", &tmp)
            .env("TMPDIR", &tmp);
        if case["environment_root"].is_string() {
            command.env("RUBOCOP_CACHE_ROOT", &env_root);
        }
        for arg in case["args"].as_array().unwrap() {
            command.arg(arg.as_str().unwrap());
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            case["exit"].as_i64(),
            "{id}"
        );
        let stderr = case["stderr"].as_str().unwrap().replace("<XDG>", xdg);
        // 本家の先頭警告だけでなく、続くpending通知の全byte/順序も固定する。
        assert_eq!(output.stderr, stderr.as_bytes(), "{id}");
        let mut found: Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut expected = case["report"].clone();
        assert_eq!(found["metadata"]["ruby_engine"], "sonicop");
        assert_eq!(expected["metadata"]["ruby_engine"], "ruby");
        assert_eq!(found["metadata"]["rubocop_version"], "1.89.0");
        // 実装固有の4identityだけを除外し、残りmetadata/summary/全診断は厳密に比較する。
        for key in fixture["metadata_identity_keys"].as_array().unwrap() {
            let key = key.as_str().unwrap();
            found["metadata"].as_object_mut().unwrap().remove(key);
            expected["metadata"].as_object_mut().unwrap().remove(key);
        }
        assert_eq!(found, expected, "{id}");
        for (path, before, bytes) in [
            (&source_path, source_before, input.as_slice()),
            (&config_path, config_before, config.as_bytes()),
            (&gemfile_path, gemfile_before, b"".as_slice()),
        ] {
            let after = fs::metadata(path).unwrap();
            assert_eq!(fs::read(path).unwrap(), bytes, "{id}");
            assert_eq!(
                before.modified().unwrap(),
                after.modified().unwrap(),
                "{id}"
            );
            assert_eq!(before.permissions(), after.permissions(), "{id}");
        }
        assert!(!env_root.exists(), "cache falseは保存先を作らない: {id}");
        assert!(!std::path::Path::new(xdg).exists(), "{id}");
        assert!(!folder.path().join("tmp").exists(), "{id}");
    }
}
