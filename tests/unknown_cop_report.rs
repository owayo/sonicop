//! 失敗時にも本家がfinishedへ渡す空reportと、同じstream内の警告順を検査する。

mod support;

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use support::project::command;
#[cfg(not(target_os = "macos"))]
use support::project::project_without_pinned_ruby;

fn bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn project_stream(raw: &[u8], cwd: &Path, recorded: bool, windows: bool) -> Value {
    let separator = b"====================";
    let boundary = raw
        .windows(separator.len())
        .enumerate()
        .find(|(index, token)| *token == separator && (*index == 0 || raw[index - 1] == b'\n'))
        .map(|(index, _)| {
            let end = index + separator.len();
            if raw[end..].starts_with(b"\r\n") {
                end + 2
            } else {
                assert!(raw[end..].starts_with(b"\n"));
                end + 1
            }
        });
    let (raw, payload) =
        boundary.map_or((raw, None), |end| (&raw[..end], Some(raw[end..].to_vec())));
    let payload = payload.map(|payload| {
        if recorded && !windows {
            // この未補正echoのWindows記録だけ、text modeが各LFへ足したCRを1個戻す。
            // 候補の本文は変換しないため、原本CRLFの消失を比較で見逃さない。
            let mut original = Vec::with_capacity(payload.len());
            for byte in payload {
                if byte == b'\n' && original.last() == Some(&b'\r') {
                    original.pop();
                }
                original.push(byte);
            }
            original
        } else {
            payload
        }
    });
    let cwd = cwd.to_string_lossy().replace('\\', "/");
    let text = String::from_utf8(raw.to_vec()).unwrap();
    let text = if windows {
        text
    } else {
        text.replace("\r\n", "\n")
    }
    .replace(&cwd, "${CWD}");
    let Some(start) = text.find("{\"metadata\"") else {
        return json!({"raw": text, "stdin_payload": payload});
    };
    let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
    let mut report = stream.next().unwrap().unwrap();
    let end = start + stream.byte_offset();
    assert_eq!(report["metadata"]["rubocop_version"], "1.89.0");
    // Ruby実行環境のmetadata以外は、空reportも前後の文字列も全項目を比較する。
    report.as_object_mut().unwrap().remove("metadata");
    json!({"prefix": &text[..start], "report": report, "suffix": &text[end..], "stdin_payload": payload})
}

fn check_cases(fixture: Value, expected_count: usize) {
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), expected_count);
    for case in cases {
        // macOSのcwdは/private/varになる。既知の一時rootと子のcwdの表記を先に揃える。
        // 失敗streamに含まれる絶対パス自体を広く正規化せず、記録の比較条件を保つ。
        #[cfg(target_os = "macos")]
        let directory = tempfile::Builder::new()
            .tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        #[cfg(not(target_os = "macos"))]
        let directory = project_without_pinned_ruby(&[]);
        fs::create_dir(directory.path().join("empty")).unwrap();
        if let Some(paths) = case["directories"].as_array() {
            for path in paths {
                fs::create_dir_all(directory.path().join(path.as_str().unwrap())).unwrap();
            }
        }
        for (path, source) in case["inputs"].as_object().unwrap() {
            fs::create_dir_all(directory.path().join(path).parent().unwrap()).unwrap();
            fs::write(directory.path().join(path), bytes(source.as_str().unwrap())).unwrap();
        }
        let mut run = command(directory.path());
        for name in [
            "RUBYOPT",
            "RUBYLIB",
            "RUBOCOP_OPTS",
            "RUBOCOP_TARGET_RUBY_VERSION",
        ] {
            run.env_remove(name);
        }
        run.args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap()),
        );
        run.write_stdin(bytes(case["stdin_hex"].as_str().unwrap()));
        let output = run.output().unwrap();
        assert_eq!(
            output.status.code(),
            case["reference"]["exit"].as_i64().map(|code| code as i32),
            "{}: {:?}",
            case["id"],
            output.stderr
        );
        for (name, actual) in [
            ("stdout_hex", &output.stdout),
            ("stderr_hex", &output.stderr),
        ] {
            assert_eq!(
                project_stream(actual, directory.path(), false, cfg!(windows)),
                project_stream(
                    &bytes(case["reference"][name].as_str().unwrap()),
                    directory.path(),
                    true,
                    cfg!(windows)
                ),
                "{} / {name}",
                case["id"]
            );
        }
        for (path, expected) in case["reference"]["outputs"].as_object().unwrap() {
            assert_eq!(
                project_stream(
                    &fs::read(directory.path().join(path)).unwrap(),
                    directory.path(),
                    false,
                    cfg!(windows)
                ),
                project_stream(
                    &bytes(expected.as_str().unwrap()),
                    directory.path(),
                    true,
                    cfg!(windows)
                ),
                "{} / {path}",
                case["id"]
            );
        }
        for (path, source) in case["inputs"].as_object().unwrap() {
            assert_eq!(
                fs::read(directory.path().join(path)).unwrap(),
                bytes(source.as_str().unwrap()),
                "{} / {path}",
                case["id"]
            );
        }
    }
}

#[test]
fn rejected_selections_preserve_empty_finished_reports_and_first_input_priority() {
    check_cases(
        serde_json::from_str(include_str!("fixtures/unknown_cop_report.json")).unwrap(),
        50,
    );
}

#[test]
fn rejected_directory_stdin_uses_discovered_targets_and_preserves_zero_and_listing() {
    check_cases(
        serde_json::from_str(include_str!("fixtures/unknown_cop_directory_report.json")).unwrap(),
        6,
    );
}

#[test]
fn posix_reference_projection_rejects_lost_original_stdin_carriage_returns() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/unknown_cop_directory_report.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "empty-safe-progress")
        .unwrap();
    let reference = bytes(case["reference"]["stdout_hex"].as_str().unwrap());
    let recorded = String::from_utf8(reference.clone()).unwrap();
    let (header, _) = recorded.split_once("====================\r\n").unwrap();
    let header = format!("{}====================\n", header.replace("\r\n", "\n"));
    let original = bytes(case["stdin_hex"].as_str().unwrap());
    assert_eq!(original, b"x=1\r\n");
    let mut preserved = header.as_bytes().to_vec();
    preserved.extend_from_slice(&original);
    let mut damaged = header.into_bytes();
    damaged.extend_from_slice(b"x=1\n");
    let cwd = Path::new("unused-project-root");
    let expected = project_stream(&reference, cwd, true, false);
    assert_eq!(project_stream(&preserved, cwd, false, false), expected);
    assert_ne!(project_stream(&damaged, cwd, false, false), expected);
}
