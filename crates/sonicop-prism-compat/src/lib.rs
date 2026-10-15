//! RuboCop の仕様版が使う Prism 1.8.1 の診断。Ruby プロセスは起動しない。

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ops::Range;

#[derive(Debug)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub kind: String,
    pub message: String,
}

type Callback = unsafe extern "C" fn(*mut c_void, usize, usize, *const c_char, *const c_char);

unsafe extern "C" {
    fn sonicop_prism_compat_parse(
        source: *const u8,
        length: usize,
        version: *const c_char,
        version_length: usize,
        binary: bool,
        callback: Callback,
        data: *mut c_void,
    ) -> c_int;
    fn sonicop_prism_compat_version() -> *const c_char;
}

unsafe extern "C" fn collect(
    data: *mut c_void,
    start: usize,
    end: usize,
    kind: *const c_char,
    message: *const c_char,
) {
    // C の呼び出し中だけ借りる。診断文字列は parser を解放する前に所有値へ複製する。
    let diagnostics = unsafe { &mut *data.cast::<Vec<Diagnostic>>() };
    diagnostics.push(Diagnostic {
        range: start..end,
        kind: unsafe { CStr::from_ptr(kind) }
            .to_string_lossy()
            .into_owned(),
        message: unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned(),
    });
}

pub fn parse(source: &[u8], version: &str, binary: bool) -> Result<Vec<Diagnostic>, &'static str> {
    let mut diagnostics = Vec::<Diagnostic>::new();
    // pm_options_version_set の latest 分岐は終端の NUL まで比較する。
    let version = CString::new(version).map_err(|_| "invalid Prism target Ruby version")?;
    // C 側は同期的で、source・version・diagnostics の参照を戻り後に保持しない。
    let result = unsafe {
        sonicop_prism_compat_parse(
            source.as_ptr(),
            source.len(),
            version.as_ptr(),
            version.as_bytes().len(),
            binary,
            collect,
            (&mut diagnostics as *mut Vec<Diagnostic>).cast(),
        )
    };
    if result == 0 {
        Ok(diagnostics)
    } else {
        Err("unsupported Prism target Ruby version")
    }
}

pub fn version() -> &'static str {
    // pm_version は、ライブラリに埋め込まれた静的な NUL 終端文字列を返す。
    unsafe { CStr::from_ptr(sonicop_prism_compat_version()) }
        .to_str()
        .expect("Prism version must be ASCII")
}
