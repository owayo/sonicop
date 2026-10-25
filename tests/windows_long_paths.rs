#![cfg(windows)]

// 長パス置換 (#98) と Windows の LF 書出し (#95) を分け、原本は明示した CRLF バイトにする。
use std::ffi::OsString;
use std::fs;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

fn ordinary_path(path: &Path) -> PathBuf {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if units.starts_with(&[92, 92, 63, 92, 85, 78, 67, 92]) {
        let mut ordinary = vec![92, 92];
        ordinary.extend_from_slice(&units[8..]);
        OsString::from_wide(&ordinary).into()
    } else if units.starts_with(&[92, 92, 63, 92]) {
        OsString::from_wide(&units[4..]).into()
    } else {
        path.to_owned()
    }
}

fn padded_file(root: &Path, length: usize, filename: &str, unicode: bool) -> PathBuf {
    let root = ordinary_path(root);
    let filename_length = filename.encode_utf16().count();
    let minimum = root.as_os_str().encode_wide().count() + filename_length + 3;
    let length = length.max(minimum);
    let mut directory = root;
    let mut remaining = length - directory.as_os_str().encode_wide().count() - filename_length - 1;
    // 各構成要素は NTFS の上限内に保ち、MAX_PATH だけを越える入力を作る。
    while remaining > 201 {
        directory.push("p".repeat(100));
        remaining -= 101;
    }
    directory.push(if unicode {
        "日".repeat(remaining - 1)
    } else {
        "p".repeat(remaining - 1)
    });
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(filename);
    assert_eq!(path.as_os_str().encode_wide().count(), length);
    path
}

fn correct(root: &Path, path: &Path, mode: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sonicop"))
        .current_dir(root)
        .args([
            "--force-default-config",
            "--cache",
            "false",
            "--only",
            "Layout/SpaceAroundOperators",
            mode,
            "--format",
            "json",
        ])
        .arg(path)
        .output()
        .unwrap()
}

fn assert_corrected(output: &Output, path: &Path) {
    assert!(
        output.status.success(),
        "{}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["summary"]["target_file_count"], 1);
    assert_eq!(report["summary"]["inspected_file_count"], 1);
    assert_eq!(report["files"].as_array().unwrap().len(), 1);
    assert_eq!(fs::read(path).unwrap(), b"x = 1\r\n");
}

#[test]
fn autocorrection_supports_long_windows_paths() {
    let directory = tempfile::tempdir().unwrap();
    let root = ordinary_path(directory.path());
    for mode in ["-a", "-A"] {
        for (length, filename, unicode, relative, extended) in [
            // 原本は MAX_PATH 未満でも、一時ファイル名だけが上限を越える。
            (257, "case.rb", false, false, false),
            (260, "case.rb", false, false, false),
            (304, "case.rb", false, true, false),
            (304, "case.rb", false, false, true),
            (304, "例😀.rb", true, false, false),
        ] {
            let path = padded_file(&root, length, filename, unicode);
            fs::write(&path, b"x=1\r\n").unwrap();
            let argument = if relative {
                path.strip_prefix(&root).unwrap().to_owned()
            } else if extended {
                fs::canonicalize(&path).unwrap()
            } else {
                path.clone()
            };
            let output = correct(&root, &argument, mode);
            assert_corrected(&output, &path);
            let output = correct(&root, &argument, mode);
            assert_corrected(&output, &path);
        }
    }
}

#[test]
fn long_path_corrections_keep_access_link_and_failure_guarantees() {
    let directory = tempfile::tempdir().unwrap();
    let root = ordinary_path(directory.path());
    let path = padded_file(&root, 304, "readonly.rb", false);
    fs::write(&path, b"x=1\r\n").unwrap();
    let permissions = fs::metadata(&path).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&path, readonly).unwrap();
    let output = correct(&root, &path, "-a");
    fs::set_permissions(&path, permissions).unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read(&path).unwrap(), b"x=1\r\n");

    let path = padded_file(&root, 304, "linked.rb", true);
    let alias = root.join("alias.rb");
    fs::write(&path, b"x=1\r\n").unwrap();
    fs::hard_link(&path, &alias).unwrap();
    let output = correct(&root, &path, "-a");
    assert_corrected(&output, &path);
    assert!(same_file::is_same_file(&path, &alias).unwrap());
    assert_eq!(fs::read(&alias).unwrap(), b"x = 1\r\n");

    let path = padded_file(&root, 304, "locked.rb", false);
    fs::write(&path, b"x=1\r\n").unwrap();
    // 読み書きだけを共有し、置換を拒むハンドルで persist 失敗後の原本と一時名を調べる。
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .unwrap();
    let before = fs::read_dir(path.parent().unwrap()).unwrap().count();
    let output = correct(&root, &path, "-a");
    drop(held);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read(&path).unwrap(), b"x=1\r\n");
    assert_eq!(
        fs::read_dir(path.parent().unwrap()).unwrap().count(),
        before
    );
}
