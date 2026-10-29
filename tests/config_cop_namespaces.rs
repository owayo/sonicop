//! 設定値の正規化を全JSON/実bytesで検査し、既存unknown残差を成功へ混ぜない。

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn fixture() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/config_cop_namespaces.json")).unwrap();
    assert_eq!(fixture["schema"], 1);
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 42);
    let ids: std::collections::HashSet<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 42);
    let order: Vec<_> = cases
        .iter()
        .map(|case| case["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        order,
        [
            "full",
            "short",
            "short_false",
            "wrong_namespace",
            "short_snake",
            "qualified_snake",
            "unknown",
            "unknown_bare",
            "obsolete",
            "ambiguous_multiple",
            "ambiguous_wrong",
            "ambiguous_self",
            "ambiguous_full",
            "ruby_extension",
            "short_parameter",
            "wrong_parameter",
            "initial_color",
            "explicit_color",
            "collision_short_first",
            "collision_full_first",
            "collision_short_no_enabled",
            "collision_full_no_enabled",
            "order_existing_target",
            "order_new_target",
            "two_short",
            "parent_short_child_full",
            "parent_full_child_short",
            "parent_short_child_short",
            "parent_wrong_child_full",
            "diamond",
            "parent_parameter_child_short",
            "short_safe",
            "short_unsafe",
            "full_safe",
            "short_stdin",
            "discovered_1",
            "discovered_2",
            "department_disabled_short_enabled",
            "department_disabled_short_inherited",
            "parent_department_disabled_child_short",
            "inherited_namespace_prefix_before_ambiguity",
            "parent_exclude_namespace_reload"
        ]
    );
    let expected = [
        ("json", 34),
        ("ambiguity_bytes", 4),
        ("rejection_envelope", 3),
        ("known_residual", 1),
    ];
    for (coverage, count) in expected {
        assert_eq!(
            cases
                .iter()
                .filter(|case| case["coverage"] == coverage)
                .count(),
            count
        );
    }
    fixture
}

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn native_text(raw: &[u8]) -> Vec<u8> {
    if cfg!(windows) {
        raw.to_vec()
    } else {
        // 参照のpipe text modeだけをhostへ合わせる。JSON診断や未補正原本へ適用しない。
        String::from_utf8(raw.to_vec())
            .unwrap()
            .replace("\r\n", "\n")
            .into_bytes()
    }
}

fn report(raw: &[u8]) -> Value {
    let mut value: Value = serde_json::from_slice(raw).unwrap();
    let identity = value.as_object_mut().unwrap().remove("metadata").unwrap();
    assert_eq!(identity["rubocop_version"], "1.89.0");
    assert_eq!(identity["ruby_engine"], "sonicop");
    value
}

fn run(case: &Value) -> (tempfile::TempDir, Output) {
    let directory = tempfile::tempdir().unwrap();
    for (name, source) in case["files"].as_object().unwrap() {
        let path = directory.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source.as_str().unwrap()).unwrap();
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
    command.current_dir(directory.path());
    for name in [
        "RUBYOPT",
        "RUBYLIB",
        "RUBOCOP_OPTS",
        "RUBOCOP_TARGET_RUBY_VERSION",
    ] {
        command.env_remove(name);
    }
    for (key, value) in case["env"].as_object().unwrap() {
        command.env(key, value.as_str().unwrap());
    }
    let args = case["args"].as_array().unwrap();
    command.args(args.iter().map(|arg| arg.as_str().unwrap()));
    let output = if args.iter().any(|arg| arg == "--stdin") {
        use std::io::Write;
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(case["files"]["a.rb"].as_str().unwrap().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    } else {
        command.output().unwrap()
    };
    assert_eq!(
        output.status.code(),
        Some(case["exit"].as_i64().unwrap() as i32),
        "{}: {:?}",
        case["id"],
        output
    );
    for (name, expected) in case["after_hex"].as_object().unwrap() {
        let raw = bytes(expected.as_str().unwrap());
        let original = case["files"][name].as_str().unwrap().as_bytes();
        let expected = if !cfg!(windows) && raw != original && name.ends_with(".rb") {
            // この有限入力の補正はpure LFコードだけ。Windows File.writeの新CRLFをhost LFへ戻す。
            assert!(!original.contains(&b'\r'));
            native_text(&raw)
        } else {
            raw
        };
        assert_eq!(
            fs::read(directory.path().join(name)).unwrap(),
            expected,
            "{}: {name}",
            case["id"]
        );
    }
    (directory, output)
}

fn ambiguity_bytes(raw: &[u8], directory: &Path) -> Vec<u8> {
    let text = String::from_utf8(raw.to_vec()).unwrap();
    // 相対configの出典は子のcwd表記なので、Windowsの8.3名を長名へ戻さない。
    #[cfg(not(target_os = "macos"))]
    let path = directory.join("config.yml");
    #[cfg(target_os = "macos")]
    let path = fs::canonicalize(directory.join("config.yml")).unwrap();
    let origin = path.to_string_lossy().replace('\\', "/");
    let origin = origin.strip_prefix("//?/").unwrap_or(&origin);
    let needle = format!(" used in {origin} needs department qualifier.");
    assert_eq!(text.matches(&needle).count(), 1, "{text}");
    text.replace(
        &needle,
        " used in <cwd>/config.yml needs department qualifier.",
    )
    .into_bytes()
}

#[test]
fn resolved_namespace_values_match_upstream_reports_and_every_source_byte() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["coverage"] == "json")
    {
        let (_directory, output) = run(case);
        assert_eq!(report(&output.stdout), case["report"], "{}", case["id"]);
        assert_eq!(
            output.stderr,
            native_text(&bytes(case["stderr_hex"].as_str().unwrap())),
            "{}",
            case["id"]
        );
        count += 1;
    }
    assert_eq!(count, 34);
}

#[test]
fn ambiguous_names_reject_after_the_measured_namespace_warning() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["coverage"] == "ambiguity_bytes")
    {
        let (directory, output) = run(case);
        assert!(output.stdout.is_empty(), "{}", case["id"]);
        // #150で末尾もnative化したため、元のstderr全bytesへ比較を昇格する。
        assert_eq!(
            ambiguity_bytes(&output.stderr, directory.path()),
            native_text(&bytes(case["stderr_hex"].as_str().unwrap())),
            "{}",
            case["id"]
        );
        count += 1;
    }
    assert_eq!(count, 4);
}

#[test]
fn existing_unknown_and_obsolete_rejections_keep_their_envelope() {
    let fixture = fixture();
    let mut rejected = 0;
    let mut linked = 0;
    for case in fixture["cases"].as_array().unwrap() {
        if case["coverage"] == "rejection_envelope" {
            let (_directory, output) = run(case);
            assert!(output.stdout.is_empty());
            assert!(!output.stderr.is_empty());
            rejected += 1;
        } else if case["coverage"] == "known_residual" {
            // 元のknown_residual/rawは維持し、#144で同じ本家記録の拒否全文へ接続する。
            assert_eq!(case["id"], "unknown_bare");
            let (_directory, output) = run(case);
            assert!(output.stdout.is_empty());
            assert_eq!(
                output.stderr,
                native_text(&bytes(case["stderr_hex"].as_str().unwrap()))
            );
            linked += 1;
        }
    }
    assert_eq!((rejected, linked), (3, 1));
}

#[cfg(windows)]
mod windows_origins {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

    fn short_path(path: &Path) -> PathBuf {
        let input: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut output = vec![0; 32_768];
        // Win32の返す実aliasを使い、canonicalizeによる長名展開で期待を作らない。
        let length =
            unsafe { GetShortPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32) };
        assert!(length > 0 && length < output.len() as u32);
        PathBuf::from(OsString::from_wide(&output[..length as usize]))
    }

    fn expand(text: &str, long: &Path, short: &Path, short_file: &str) -> String {
        let hybrid = short.parent().unwrap().join(long.file_name().unwrap());
        text.replace("<long>", &long.to_string_lossy().replace('\\', "/"))
            .replace("<short>", &short.to_string_lossy().replace('\\', "/"))
            .replace("<hybrid>", &hybrid.to_string_lossy().replace('\\', "/"))
            .replace("<short_file>", short_file)
    }

    #[test]
    fn namespace_origins_preserve_the_measured_windows_path_spellings() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/config_cop_namespace_origins.json"))
                .unwrap();
        assert_eq!(fixture["schema"], 1);
        let groups = fixture["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["cases"].as_array().unwrap().len(), 14);
        assert_eq!(groups[1]["cases"].as_array().unwrap().len(), 10);
        let mut count = 0;
        let mut ids = std::collections::HashSet::new();
        for group in groups {
            let directory = tempfile::Builder::new()
                .prefix("sonicop-namespace-origin-")
                .tempdir()
                .unwrap();
            let root = directory.path().join("namespace-origin-fixture-directory");
            std::fs::create_dir(&root).unwrap();
            let long = std::fs::canonicalize(&root).unwrap();
            let long = long.to_string_lossy();
            let long = PathBuf::from(long.strip_prefix(r"\\?\").unwrap_or(&long));
            let short = short_path(&long);
            let hybrid = short.parent().unwrap().join(long.file_name().unwrap());
            if short == long || hybrid == long || hybrid == short {
                // 親と末尾の両aliasがないvolumeでは24条件を区別できず、互換成功とは数えない。
                eprintln!(
                    "未計測: long/short/hybridの3表記を作れないvolumeのため24 origin条件を省略する"
                );
                return;
            }
            for (name, text) in group["files"].as_object().unwrap() {
                let path = long.join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, expand(text.as_str().unwrap(), &long, &short, "")).unwrap();
            }
            let short_file = if group["id"] == "explicit" {
                short_path(&long.join(fixture["long_filename"].as_str().unwrap()))
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            } else {
                String::new()
            };
            for case in group["cases"].as_array().unwrap() {
                assert!(ids.insert(case["id"].as_str().unwrap()));
                let id = case["id"].as_str().unwrap();
                let cwd = expand(case["cwd"].as_str().unwrap(), &long, &short, &short_file);
                let args: Vec<_> = case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| expand(arg.as_str().unwrap(), &long, &short, &short_file))
                    .collect();
                let mut command = Command::new(env!("CARGO_BIN_EXE_sonicop"));
                command
                    .current_dir(cwd)
                    .env("CLICOLOR_FORCE", "0")
                    .args(args);
                for key in [
                    "RUBYOPT",
                    "RUBYLIB",
                    "RUBOCOP_OPTS",
                    "RUBOCOP_TARGET_RUBY_VERSION",
                ] {
                    command.env_remove(key);
                }
                let output = command.output().unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(case["exit"].as_i64().unwrap() as i32),
                    "{id}: {output:?}"
                );
                let expected_stderr = expand(
                    case["stderr_lf"].as_str().unwrap(),
                    &long,
                    &short,
                    &short_file,
                );
                // 元の2拒否も含め24件すべて、警告と最終errorのCRLFを比較する。
                assert_eq!(
                    output.stderr,
                    expected_stderr.replace('\n', "\r\n").as_bytes(),
                    "{id}"
                );
                if case["report"].is_null() {
                    assert!(output.stdout.is_empty(), "{id}");
                } else {
                    let expected: serde_json::Value = serde_json::from_str(&expand(
                        &case["report"].to_string(),
                        &long,
                        &short,
                        &short_file,
                    ))
                    .unwrap();
                    assert_eq!(super::report(&output.stdout), expected, "{id}");
                }
                for (name, text) in group["files"].as_object().unwrap() {
                    assert_eq!(
                        std::fs::read(long.join(name)).unwrap(),
                        expand(text.as_str().unwrap(), &long, &short, "").as_bytes(),
                        "{id}: {name}"
                    );
                }
                count += 1;
            }
        }
        assert_eq!(count, 24);
        assert_eq!(ids.len(), 24);
    }
}
