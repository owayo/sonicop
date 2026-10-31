//! 本家 Ruby の Onigmo を、Ruby VM と OS 共有ライブラリなしで利用する。
use std::{
    collections::HashMap,
    ffi::c_void,
    ptr::NonNull,
    sync::{OnceLock, mpsc},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Encoding {
    Utf8 = 0,
    Binary = 1,
    UsAscii = 2,
    EucJp = 3,
    Windows31J = 4,
}

/// Regexp#names の byte と符号化。ASCII 名の Symbol は呼出し側で US-ASCII として比較する。
#[derive(Debug, PartialEq, Eq)]
pub struct CompiledRegexp {
    pub encoding: Encoding,
    pub names: Vec<Vec<u8>>,
}
type NameCallback = unsafe extern "C" fn(*const u8, usize, i32, *mut c_void);
unsafe extern "C" {
    fn sonicop_onigmo_init();
    fn sonicop_onigmo_check(
        pattern: *const u8,
        length: usize,
        encoding: i32,
        extended: i32,
        error: *mut u8,
        capacity: usize,
    ) -> i32;
    fn sonicop_onigmo_validate(
        pattern: *const u8,
        length: usize,
        encoding: i32,
        extended: i32,
        error: *mut u8,
        capacity: usize,
    ) -> i32;
    fn sonicop_onigmo_compile(
        pattern: *const u8,
        length: usize,
        encoding: i32,
        extended: i32,
        error: *mut u8,
        capacity: usize,
        compiled_encoding: *mut i32,
        callback: Option<NameCallback>,
        context: *mut c_void,
    ) -> i32;
    fn sonicop_onigmo_match_compile(
        pattern: *const u8,
        length: usize,
        error: *mut u8,
        capacity: usize,
    ) -> *mut c_void;
    fn sonicop_onigmo_match(
        compiled: *mut c_void,
        text: *const u8,
        length: usize,
        error: *mut u8,
        capacity: usize,
    ) -> i32;
    fn sonicop_onigmo_match_free(compiled: *mut c_void);
}
pub fn validate(pattern: &[u8], encoding: Encoding, extended: bool) -> Result<(), Vec<u8>> {
    request(
        pattern,
        encoding,
        extended,
        Operation::Compile(CompileOperation::Validate),
    )
    .map(|_| ())
}
pub fn check(pattern: &[u8], encoding: Encoding, extended: bool) -> Result<(), Vec<u8>> {
    request(
        pattern,
        encoding,
        extended,
        Operation::Compile(CompileOperation::Check),
    )
    .map(|_| ())
}
/// 前処理後にコンパイルした名前を返す。Unicode escape による UTF-8 固定も Onigmo に任せる。
pub fn compile(
    pattern: &[u8],
    encoding: Encoding,
    extended: bool,
) -> Result<CompiledRegexp, Vec<u8>> {
    request(
        pattern,
        encoding,
        extended,
        Operation::Compile(CompileOperation::Names),
    )
    .map(|reply| {
        let Reply::Compiled(compiled) = reply else {
            unreachable!("compile request returns compiled names")
        };
        compiled
    })
}

/// UTF-8本文への一致。不正な本文はRubyのArgumentErrorに対応するNone、regexpの拒否はErr。
/// 設定由来のpatternは専用workerでコンパイルし、プロセス中に再利用する。
pub fn matches_utf8(pattern: &[u8], text: &[u8]) -> Result<Option<bool>, Vec<u8>> {
    request(
        pattern,
        Encoding::Utf8,
        false,
        Operation::Match(text.to_vec()),
    )
    .map(|reply| {
        let Reply::Matched(matched) = reply else {
            unreachable!("match request returns a match")
        };
        matched
    })
}

#[derive(Clone, Copy)]
enum CompileOperation {
    Validate,
    Check,
    Names,
}
enum Operation {
    Compile(CompileOperation),
    Match(Vec<u8>),
}
enum Reply {
    Compiled(CompiledRegexp),
    Matched(Option<bool>),
}
struct Job {
    pattern: Vec<u8>,
    encoding: Encoding,
    extended: bool,
    operation: Operation,
    reply: mpsc::SyncSender<Result<Reply, Vec<u8>>>,
}
fn request(
    pattern: &[u8],
    encoding: Encoding,
    extended: bool,
    operation: Operation,
) -> Result<Reply, Vec<u8>> {
    static ENGINE: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Job>(1);
        // Onigmo の compiler/destructor は再帰する。rayon の 2MiB stack を使わず、
        // 上流の parse depth 4096 を保ったまま専用 stack 上で直列化する。
        std::thread::Builder::new()
            .name("sonicop-onigmo".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                // process-global な初期化・警告設定・byte buffer arena は名前取得も同じ thread だけが触る。
                unsafe { sonicop_onigmo_init() };
                // C handleとそのdestructorも同じ大きなstackのworkerだけが所有する。
                let mut matchers: HashMap<Vec<u8>, Result<NativeMatcher, Vec<u8>>> = HashMap::new();
                while let Ok(job) = receiver.recv() {
                    let result = match job.operation {
                        Operation::Compile(operation) => {
                            native(&job.pattern, job.encoding, job.extended, operation)
                                .map(Reply::Compiled)
                        }
                        Operation::Match(text) => {
                            let compiled = matchers
                                .entry(job.pattern.clone())
                                .or_insert_with(|| NativeMatcher::new(&job.pattern));
                            compiled
                                .as_ref()
                                .map_err(Clone::clone)
                                .and_then(|compiled| {
                                    if std::str::from_utf8(&text).is_err() {
                                        Ok(None)
                                    } else {
                                        compiled.matches(&text).map(Some)
                                    }
                                })
                                .map(Reply::Matched)
                        }
                    };
                    let _ = job.reply.send(result);
                }
            })
            .expect("cannot start regexp compiler worker");
        sender
    });
    let (reply, result) = mpsc::sync_channel(1);
    engine
        .send(Job {
            pattern: pattern.to_vec(),
            encoding,
            extended,
            operation,
            reply,
        })
        .expect("regexp compiler worker stopped");
    result
        .recv()
        .expect("regexp compiler worker did not return")
}

struct NativeMatcher(NonNull<c_void>);

impl NativeMatcher {
    fn new(pattern: &[u8]) -> Result<Self, Vec<u8>> {
        let mut error = vec![0; pattern.len().saturating_mul(6).saturating_add(2048)];
        static EMPTY: [u8; 1] = [0];
        let pointer = if pattern.is_empty() {
            EMPTY.as_ptr()
        } else {
            pattern.as_ptr()
        };
        // Cは入力を呼出し中だけ読み、独立したbytecodeを返す。handleはworker内に留める。
        let compiled = unsafe {
            sonicop_onigmo_match_compile(pointer, pattern.len(), error.as_mut_ptr(), error.len())
        };
        NonNull::new(compiled)
            .map(Self)
            .ok_or_else(|| error_message(error))
    }

    fn matches(&self, text: &[u8]) -> Result<bool, Vec<u8>> {
        let mut error = vec![0; 2048];
        static EMPTY: [u8; 1] = [0];
        let pointer = if text.is_empty() {
            EMPTY.as_ptr()
        } else {
            text.as_ptr()
        };
        // UTF-8検証済み本文と、同じworkerで生存するhandleを同期呼出し中だけ渡す。
        match unsafe {
            sonicop_onigmo_match(
                self.0.as_ptr(),
                pointer,
                text.len(),
                error.as_mut_ptr(),
                error.len(),
            )
        } {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(error_message(error)),
        }
    }
}

impl Drop for NativeMatcher {
    fn drop(&mut self) {
        // compilerと同じworkerのstackで解放し、呼出し側の小さなstackへ移さない。
        unsafe { sonicop_onigmo_match_free(self.0.as_ptr()) };
    }
}

fn error_message(mut error: Vec<u8>) -> Vec<u8> {
    let length = error
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(error.len());
    error.truncate(length);
    error
}
unsafe extern "C" fn collect_name(
    name: *const u8,
    length: usize,
    first_group: i32,
    context: *mut c_void,
) {
    // C は onig_foreach_name の同期呼出し中だけ、解放前の名前と生存中の Vec を渡す。
    let names = unsafe { &mut *context.cast::<Vec<(i32, Vec<u8>)>>() };
    let bytes = unsafe { std::slice::from_raw_parts(name, length) };
    names.push((first_group, bytes.to_vec()));
}
fn native(
    pattern: &[u8],
    encoding: Encoding,
    extended: bool,
    operation: CompileOperation,
) -> Result<CompiledRegexp, Vec<u8>> {
    let mut error = vec![0u8; pattern.len().saturating_mul(6).saturating_add(2048)];
    // 長さ 0 でも C の範囲演算へ実在する配列の pointer を渡す。
    static EMPTY: [u8; 1] = [0];
    let pointer = if pattern.is_empty() {
        EMPTY.as_ptr()
    } else {
        pattern.as_ptr()
    };
    let mut names: Vec<(i32, Vec<u8>)> = Vec::new();
    let mut compiled_encoding = encoding as i32;
    // 入出力は呼出し中に生存し、callback もこの worker 上で同期的に完了する。
    let result = unsafe {
        match operation {
            CompileOperation::Names => sonicop_onigmo_compile(
                pointer,
                pattern.len(),
                encoding as i32,
                i32::from(extended),
                error.as_mut_ptr(),
                error.len(),
                &mut compiled_encoding,
                Some(collect_name),
                (&mut names as *mut Vec<(i32, Vec<u8>)>).cast(),
            ),
            CompileOperation::Check => sonicop_onigmo_check(
                pointer,
                pattern.len(),
                encoding as i32,
                i32::from(extended),
                error.as_mut_ptr(),
                error.len(),
            ),
            CompileOperation::Validate => sonicop_onigmo_validate(
                pointer,
                pattern.len(),
                encoding as i32,
                i32::from(extended),
                error.as_mut_ptr(),
                error.len(),
            ),
        }
    };
    if result != 0 {
        let length = error
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(error.len());
        return Err(error[..length].to_vec());
    }
    let encoding = match compiled_encoding {
        0 => Encoding::Utf8,
        1 => Encoding::Binary,
        2 => Encoding::UsAscii,
        3 => Encoding::EucJp,
        4 => Encoding::Windows31J,
        _ => return Err(b"unsupported regexp encoding".to_vec()),
    };
    // Onigmo の名前表の列挙順には依存せず、最初の capture 番号で Regexp#names の順にする。
    names.sort_unstable_by_key(|(first_group, _)| *first_group);
    Ok(CompiledRegexp {
        encoding,
        names: names.into_iter().map(|(_, name)| name).collect(),
    })
}
#[cfg(test)]
mod tests {
    use super::{Encoding, check, validate};
    #[test]
    fn ruby_unicode_properties_and_patterns_compile_without_a_vm() {
        for source in [
            r"\p{Kawi}",
            r"\p{Sidetic}",
            r"\p{Tai_Yo}",
            r"(?<foo>a)(?<foo>b)",
            r"(?<=a|bc)d",
            r"(?a:a)",
            r"(?~a)",
        ] {
            assert_eq!(
                validate(source.as_bytes(), Encoding::Utf8, false),
                Ok(()),
                "{source}"
            );
        }
    }
    #[test]
    fn ruby_group_names_are_not_limited_to_word_characters() {
        for (source, encoding) in [
            (b"(?<\xc3\xa9>x)".as_slice(), Encoding::Binary),
            (b"(?<a-b>x)".as_slice(), Encoding::Utf8),
            (b"(?< >x)".as_slice(), Encoding::Utf8),
        ] {
            assert_eq!(check(source, encoding, false), Ok(()));
        }
    }
    #[test]
    fn ruby_compile_errors_use_the_native_error_messages() {
        for (source, message) in [
            ("(", "end pattern with unmatched parenthesis"),
            ("[z-a]", "empty range in char class"),
            (r"\1", "invalid backref number/name"),
            ("(?<=a*)b", "invalid pattern in look-behind"),
            ("a{2,1}", "upper is smaller than lower in repeat range"),
        ] {
            assert_eq!(
                validate(source.as_bytes(), Encoding::Utf8, false),
                Err(message.as_bytes().to_vec()),
                "{source}"
            );
        }
    }
    #[test]
    fn ruby_preprocessing_rejects_invalid_utf8_but_preserves_binary() {
        assert_eq!(
            super::check(br"\x80", Encoding::Utf8, false),
            Err(br"invalid multibyte escape: /\x80/".to_vec())
        );
        assert_eq!(super::check(br"\x80", Encoding::Binary, false), Ok(()));
        assert_eq!(super::check(br"\u{e9}", Encoding::Utf8, false), Ok(()));
        assert_eq!(
            super::check(br"\uD800", Encoding::Utf8, false),
            Err(br"invalid Unicode range: /\uD800/".to_vec())
        );
        assert_eq!(
            super::check(b"a/b(", Encoding::Utf8, true),
            Err(br"end pattern with unmatched parenthesis: /a\/b(/x".to_vec())
        );
    }
    #[test]
    fn ruby_valid_deep_groups_do_not_use_the_callers_small_stack() {
        let pattern = format!("{}a{}", "(".repeat(3000), ")".repeat(3000));
        let result = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || check(pattern.as_bytes(), Encoding::Utf8, false))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn compiled_names_keep_the_native_encoding_and_first_capture_order() {
        use super::compile;
        assert_eq!(
            compile(br"(?<z>x)(?<a>y)(?<z>z)", Encoding::Utf8, false).unwrap(),
            super::CompiledRegexp {
                encoding: Encoding::UsAscii,
                names: vec![b"z".to_vec(), b"a".to_vec()]
            }
        );
        assert_eq!(
            compile(b"(?<\xa4\xa2>x)(?<ascii>y)", Encoding::EucJp, false).unwrap(),
            super::CompiledRegexp {
                encoding: Encoding::EucJp,
                names: vec![vec![0xa4, 0xa2], b"ascii".to_vec()]
            }
        );
        for encoding in [Encoding::Binary, Encoding::EucJp, Encoding::Windows31J] {
            assert_eq!(
                compile(br"(?<\u3042>x)", encoding, false).unwrap(),
                super::CompiledRegexp {
                    encoding: Encoding::Utf8,
                    names: vec!["あ".as_bytes().to_vec()]
                }
            );
        }
        assert_eq!(
            compile(b"(?<ascii>\xa4\xa2)", Encoding::EucJp, false)
                .unwrap()
                .names,
            vec![b"ascii".to_vec()]
        );
        assert!(compile(b"(", Encoding::Utf8, false).is_err());
        assert!(
            compile(b"", Encoding::Utf8, false)
                .unwrap()
                .names
                .is_empty()
        );
    }
}
