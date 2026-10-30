//! Windows本家の動的な実パスを、専用tempdir内の同じ構造へ束縛する。

use std::collections::BTreeMap;
use std::path::Path;

pub fn setup(work: &Path) -> BTreeMap<String, String> {
    let long = work.join("a deliberately long real cache directory name");
    std::fs::create_dir_all(long.join("0/rubocop_cache")).unwrap();
    std::fs::create_dir_all(work.join("nested realpath parent/child")).unwrap();
    std::fs::create_dir_all(work.join("nested realpath parent/rubocop_cache")).unwrap();
    std::fs::create_dir_all(work.join("rubocop_cache")).unwrap();
    let work = windows_name(work, false);
    let long = windows_name(&long, false);
    let short = windows_name(Path::new(&long), true);
    assert_ne!(short, long, "実8.3名が必要。未測定を成功として数えない");
    for (name, target) in [
        ("junction alias", long.clone()),
        (
            "nested junction",
            format!("{work}\\nested realpath parent\\child"),
        ),
    ] {
        let link = Path::new(&work).join(name);
        let target = Path::new(&target);
        assert!(link.starts_with(&work) && target.starts_with(&work));
        assert!(!link.exists());
        let done = std::process::Command::new("cmd.exe")
            .args(["/d", "/v:off", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            done.status.success(),
            "{}",
            String::from_utf8_lossy(&done.stderr)
        );
    }
    let mut bindings = BTreeMap::from([
        ("WORK".to_owned(), work),
        ("LONG".to_owned(), long.clone()),
        ("SHORT".to_owned(), short),
        ("UPPER".to_owned(), long.to_uppercase()),
        (
            "LOWER_LONG".to_owned(),
            format!("{}{}", long[..1].to_lowercase(), &long[1..]),
        ),
    ]);
    for (key, value) in bindings.clone() {
        bindings.insert(format!("{key}_FWD"), value.replace('\\', "/"));
    }
    bindings
}

pub fn bind(template: &str, bindings: &BTreeMap<String, String>) -> String {
    bindings
        .iter()
        .fold(template.to_owned(), |text, (key, value)| {
            text.replace(&format!("<{key}>"), value)
        })
}

pub fn root(kind: &str, bindings: &BTreeMap<String, String>) -> String {
    match kind {
        "long" => bindings["LONG"].clone(),
        "short" => bindings["SHORT"].clone(),
        "upper" => bindings["UPPER"].clone(),
        "junction" => format!("{}\\junction alias", bindings["WORK"]),
        "dot" => format!(
            "{}\\..\\a deliberately long real cache directory name",
            bindings["LONG"]
        ),
        "tmp" => "tmp".to_owned(),
        _ => panic!("unknown reference root: {kind}"),
    }
}

fn windows_name(path: &Path, short: bool) -> String {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLongPathNameW(input: *const u16, output: *mut u16, length: u32) -> u32;
        fn GetShortPathNameW(input: *const u16, output: *mut u16, length: u32) -> u32;
    }
    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut output = vec![0; 32768];
    // 出力容量とNUL終端は固定し、APIが返す実長だけを読む。
    let size = unsafe {
        if short {
            GetShortPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32)
        } else {
            GetLongPathNameW(input.as_ptr(), output.as_mut_ptr(), output.len() as u32)
        }
    } as usize;
    assert!(size > 0 && size < output.len());
    String::from_utf16(&output[..size]).unwrap()
}
