//! base_configsはURL判定より先に、起動位置を基準とするDir.globを適用する。

use std::fs;
use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};

pub(super) fn expand(pattern: &str, cwd: &Path) -> Vec<String> {
    // PathUtil.glob?はescapeや閉じ括弧の有無を調べない。非globの欠落fileは後段で拒否する。
    if !pattern.contains(['*', '{', '[', '?']) {
        return vec![pattern.to_owned()];
    }
    let mut result = Vec::new();
    for branch in braces(pattern) {
        let mut paths = expand_branch(&branch, cwd);
        // braceごとの順と重複を保つ。Pathの構成要素順ではなく、Rubyの文字列順にする。
        paths.sort();
        result.extend(paths);
    }
    result
}

fn braces(pattern: &str) -> Vec<String> {
    let mut pending = vec![pattern.to_owned()];
    let mut result = Vec::new();
    while let Some(pattern) = pending.pop() {
        let mut escaped = false;
        let mut open = None;
        let mut depth = 0;
        let mut start = 0;
        let mut alternatives = Vec::new();
        let mut close = None;
        for (index, ch) in pattern.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '{' => {
                    if depth == 0 {
                        open = Some(index);
                        start = index + 1;
                    }
                    depth += 1;
                }
                ',' if depth == 1 => {
                    alternatives.push(&pattern[start..index]);
                    start = index + 1;
                }
                '}' if depth > 0 => {
                    depth -= 1;
                    if depth == 0 {
                        alternatives.push(&pattern[start..index]);
                        close = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        match (open, close) {
            (None, _) => result.push(pattern),
            (Some(open), Some(close)) => {
                for alternative in alternatives.into_iter().rev() {
                    pending.push(format!(
                        "{}{alternative}{}",
                        &pattern[..open],
                        &pattern[close + 1..]
                    ));
                }
            }
            // MRIのbrace展開は、class内の閉じていないbraceも該当なしにする。
            (Some(_), None) => {}
        }
    }
    result
}

struct Position {
    path: PathBuf,
    display: String,
    index: usize,
    separator: bool,
}

fn expand_branch(pattern: &str, cwd: &Path) -> Vec<String> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let leading = pattern.bytes().take_while(|byte| *byte == b'/').count();
    let drive = cfg!(windows)
        && pattern
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
        && pattern.as_bytes().get(1) == Some(&b':');
    let prefix_len = if drive {
        2 + usize::from(pattern.as_bytes().get(2) == Some(&b'/'))
    } else {
        leading
    };
    let prefix = &pattern[..prefix_len];
    let path = if prefix.is_empty() || (cfg!(windows) && leading == 1) {
        cwd.join(prefix)
    } else {
        PathBuf::from(prefix)
    };
    let raw: Vec<_> = pattern[prefix_len..].split('/').collect();
    let mut segments = Vec::new();
    for (index, segment) in raw.iter().enumerate() {
        // MRIは連続した再帰辺を一つにし、同じ階層分割による重複を作らない。
        // 末尾**は再帰辺ではなく通常のstarなので残す。
        if *segment == "**" && index + 1 < raw.len() && segments.last() == Some(&"**") {
            continue;
        }
        segments.push(*segment);
    }
    let mut pending = vec![Position {
        path,
        display: prefix.to_owned(),
        index: 0,
        separator: false,
    }];
    let mut result = Vec::new();
    while let Some(position) = pending.pop() {
        let segment = segments[position.index];
        let last = position.index + 1 == segments.len();
        let separator = if position.separator { "/" } else { "" };
        if segment == "**" && !last {
            if let Ok(entries) = fs::read_dir(&position.path) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    // **による自動降下はsymlinkを辿らない。明示した通常の辺とは分ける。
                    if !name.starts_with('.') && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        pending.push(Position {
                            path: entry.path(),
                            display: format!("{}{separator}{name}", position.display),
                            index: position.index,
                            separator: true,
                        });
                    }
                }
            }
            // **/は0階層も含み、次の辺の前に余分なslashを増やさない。
            let mut zero = position;
            zero.index += 1;
            pending.push(zero);
            continue;
        }
        if segment.is_empty() {
            if !last {
                pending.push(Position {
                    path: position.path,
                    display: format!("{}{separator}", position.display),
                    index: position.index + 1,
                    separator: true,
                });
            } else if !position.display.is_empty() && position.path.is_dir() {
                result.push(format!("{}{separator}", position.display));
            }
            continue;
        }
        let Some((matcher, magic, literal, class)) = matcher(segment) else {
            continue;
        };
        if !magic {
            let path = position.path.join(&literal);
            if fs::symlink_metadata(&path).is_err() {
                continue;
            }
            let display = format!("{}{separator}{literal}", position.display);
            if last {
                result.push(display);
            } else {
                pending.push(Position {
                    path,
                    display,
                    index: position.index + 1,
                    separator: true,
                });
            }
            continue;
        }
        let options = MatchOptions {
            // 本家のASCII照合はmacOSではvolumeの能力にも従う。classの実測は別に保つ。
            case_sensitive: case_sensitive(&position.path, class),
            require_literal_separator: false,
            require_literal_leading_dot: true,
        };
        let Ok(entries) = fs::read_dir(&position.path) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // glob crateは*.ymlの末尾のdotでも先頭dotを許すが、Dir.globは先頭のliteralだけを見る。
            if name.starts_with('.') && !segment.starts_with('.') && !segment.starts_with("\\.") {
                continue;
            }
            if !matcher.matches_with(&name, options) {
                continue;
            }
            let display = format!("{}{separator}{name}", position.display);
            if last {
                result.push(display);
            } else {
                pending.push(Position {
                    path: entry.path(),
                    display,
                    index: position.index + 1,
                    separator: true,
                });
            }
        }
    }
    result
}

fn case_sensitive(path: &Path, class: bool) -> bool {
    if class {
        return true;
    }
    #[cfg(windows)]
    {
        let _ = path;
        false
    }
    #[cfg(target_os = "macos")]
    {
        !volume_folds_case(path)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = path;
        true
    }
}

#[cfg(target_os = "macos")]
fn volume_folds_case(path: &Path) -> bool {
    use std::os::fd::AsRawFd;

    #[repr(C)]
    struct Volume {
        length: u32,
        capabilities: libc::vol_capabilities_attr_t,
    }

    let Ok(directory) = fs::File::open(path) else {
        return false;
    };
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_CAPABILITIES,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut output = Volume {
        length: 0,
        capabilities: libc::vol_capabilities_attr_t {
            capabilities: [0; 4],
            valid: [0; 4],
        },
    };
    // MRIのis_case_sensitiveと同じ能力照会。C配置の初期化済み領域と、その全長を渡す。
    let status = unsafe {
        libc::fgetattrlist(
            directory.as_raw_fd(),
            (&mut attributes as *mut libc::attrlist).cast(),
            (&mut output as *mut Volume).cast(),
            std::mem::size_of::<Volume>(),
            libc::FSOPT_NOFOLLOW,
        )
    };
    let index = libc::VOL_CAPABILITIES_FORMAT;
    let mask = libc::VOL_CAP_FMT_CASE_SENSITIVE;
    status == 0
        && output.capabilities.valid[index] & mask != 0
        && output.capabilities.capabilities[index] & mask == 0
}

fn matcher(segment: &str) -> Option<(Pattern, bool, String, bool)> {
    let mut pattern = String::new();
    let mut literal = String::new();
    let mut chars = segment.chars().peekable();
    let mut magic = false;
    let mut class = false;
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let escaped = chars.next()?;
            pattern.push_str(&Pattern::escape(&escaped.to_string()));
            literal.push(escaped);
        } else if ch == '*' {
            magic = true;
            pattern.push('*');
            while chars.peek() == Some(&'*') {
                chars.next();
            }
        } else {
            magic |= matches!(ch, '?' | '[');
            class |= ch == '[';
            pattern.push(ch);
            literal.push(ch);
            if ch == '[' && chars.peek() == Some(&'^') {
                pattern.push('!');
                chars.next();
            }
        }
    }
    Some((Pattern::new(&pattern).ok()?, magic, literal, class))
}

#[cfg(test)]
mod tests {
    use super::expand;
    use std::{fs, path::Path, process::Command};

    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../../tests/fixtures/inherit_glob_api.json")).unwrap()
    }

    struct Original {
        path: std::path::PathBuf,
        bytes: Vec<u8>,
        mtime: std::time::SystemTime,
        permissions: fs::Permissions,
    }

    fn prepare(root: &Path, group: &serde_json::Value) -> Vec<Original> {
        group["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, body)| {
                let path = root.join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                let bytes = body.as_str().unwrap().as_bytes().to_vec();
                fs::write(&path, &bytes).unwrap();
                let info = fs::metadata(&path).unwrap();
                Original {
                    path,
                    bytes,
                    mtime: info.modified().unwrap(),
                    permissions: info.permissions(),
                }
            })
            .collect()
    }

    fn preserved(originals: Vec<Original>) {
        for Original {
            path,
            bytes,
            mtime,
            permissions,
        } in originals
        {
            assert_eq!(fs::read(&path).unwrap(), bytes);
            let info = fs::metadata(path).unwrap();
            assert_eq!(info.modified().unwrap(), mtime);
            assert_eq!(info.permissions(), permissions);
        }
    }

    #[test]
    #[cfg(windows)]
    fn forty_nine_actual_windows_api_conditions_preserve_glob_order_and_originals() {
        let mut checked = 0;
        for group in fixture()["groups"].as_array().unwrap() {
            let directory = tempfile::tempdir().unwrap();
            let originals = prepare(directory.path(), group);
            for row in group["rows"].as_array().unwrap() {
                let expected: Vec<String> =
                    serde_json::from_value(row["expanded"].clone()).unwrap();
                assert_eq!(
                    expand(row["pattern"].as_str().unwrap(), directory.path()),
                    expected,
                    "{}",
                    row["pattern"]
                );
                checked += 1;
            }
            preserved(originals);
        }
        assert_eq!(checked, 49);
    }

    #[test]
    fn compare_the_actual_rubocop_glob_api_on_the_current_platform() {
        if std::env::var("SONICOP_RUNTIME_TESTS").as_deref() != Ok("1") {
            return;
        }
        // CIの全OSは固定gemを用意する。Windowsの保存済みpipeを他OSの実走へ読み替えない。
        let ruby = std::env::var_os("SONICOP_RUNTIME_RUBY").unwrap_or_else(|| "ruby".into());
        let code = "gem 'rubocop', '= 1.89.0'; require 'rubocop'; require 'json'; print JSON.generate(JSON.parse(ARGV.fetch(0)).map {|p| RuboCop::PathUtil.glob?(p) ? Dir.glob(p) : [p]})";
        let mut mismatches = Vec::new();
        for group in fixture()["groups"].as_array().unwrap() {
            let directory = tempfile::tempdir().unwrap();
            let originals = prepare(directory.path(), group);
            let patterns: Vec<_> = group["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["pattern"].as_str().unwrap())
                .collect();
            let output = Command::new(&ruby)
                .current_dir(directory.path())
                .env_remove("RUBYOPT")
                .env_remove("RUBYLIB")
                .env_remove("RUBOCOP_OPTS")
                .args(["-e", code, &serde_json::to_string(&patterns).unwrap()])
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            let actual: Vec<Vec<String>> = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(actual.len(), patterns.len());
            for (pattern, expected) in patterns.into_iter().zip(actual) {
                let native = expand(pattern, directory.path());
                if native != expected {
                    mismatches.push((pattern.to_owned(), native, expected));
                }
            }
            preserved(originals);
        }
        assert!(mismatches.is_empty(), "{mismatches:#?}");
    }
}
