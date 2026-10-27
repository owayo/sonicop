//! 読み取れる本文の境界と、whitequark が EOF の先へ返す報告範囲を分ける。

use std::fs;
use std::process::Command;

use serde_json::Value;

#[test]
fn syntax_eof_ranges_match_recorded_reference_without_ruby() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/whitequark_eof_ranges.json")).unwrap();
    let sources = fixture["sources"].as_array().unwrap();
    let groups = fixture["groups"].as_array().unwrap();
    assert_eq!(sources.len(), 6);
    assert_eq!(groups.len(), 3);

    for group in groups {
        let folder = tempfile::tempdir().unwrap();
        fs::create_dir(folder.path().join("cases")).unwrap();
        fs::write(
            folder.path().join("config.yml"),
            group["yaml"].as_str().unwrap(),
        )
        .unwrap();
        let mut originals = Vec::new();
        for source in sources {
            let hex = source["source_hex"].as_str().unwrap().as_bytes();
            let (pairs, remainder) = hex.as_chunks::<2>();
            assert!(remainder.is_empty());
            let bytes: Vec<u8> = pairs
                .iter()
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            let path = folder.path().join(source["path"].as_str().unwrap());
            fs::write(&path, &bytes).unwrap();
            let metadata = fs::metadata(&path).unwrap();
            originals.push((path, bytes, metadata));
        }

        let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
        command
            .current_dir(folder.path())
            .env("PATH", "")
            .env_remove("RUBYOPT")
            .env_remove("RUBYLIB")
            .env_remove("RUBOCOP_OPTS")
            .env_remove("RUBOCOP_TARGET_RUBY_VERSION")
            .env("XDG_CACHE_HOME", folder.path().join("cache"))
            .args([
                "--config",
                "config.yml",
                "--cache",
                "false",
                "--only",
                "Lint/Syntax",
                "--format",
                "json",
            ]);
        for source in sources {
            command.arg(source["path"].as_str().unwrap());
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code().map(i64::from), group["exit"].as_i64());
        let mut found: Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut expected = group["report"].clone();
        // 期待の全診断・順序・件数を保ち、実装固有の runtime identity だけを除く。
        for key in fixture["metadata_identity_keys"].as_array().unwrap() {
            let key = key.as_str().unwrap();
            found["metadata"].as_object_mut().unwrap().remove(key);
            expected["metadata"].as_object_mut().unwrap().remove(key);
        }
        assert_eq!(found, expected, "{} {}", group["target"], group["engine"]);
        for (path, bytes, before) in originals {
            let after = fs::metadata(&path).unwrap();
            assert_eq!(fs::read(path).unwrap(), bytes);
            assert_eq!(before.modified().unwrap(), after.modified().unwrap());
            assert_eq!(before.permissions(), after.permissions());
        }
    }
}
