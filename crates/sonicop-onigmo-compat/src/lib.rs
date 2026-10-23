//! 本家 Ruby の Onigmo を、Ruby VM と OS 共有ライブラリなしで利用する。
use std::sync::{OnceLock, mpsc};
#[derive(Clone, Copy, Debug)]
#[repr(i32)]
pub enum Encoding {
    Utf8 = 0,
    Binary = 1,
    UsAscii = 2,
    EucJp = 3,
    Windows31J = 4,
}
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
}
pub fn validate(pattern: &[u8], encoding: Encoding, extended: bool) -> Result<(), Vec<u8>> {
    validate_with(pattern, encoding, extended, false)
}

pub fn check(pattern: &[u8], encoding: Encoding, extended: bool) -> Result<(), Vec<u8>> {
    validate_with(pattern, encoding, extended, true)
}

struct Job {
    pattern: Vec<u8>,
    encoding: Encoding,
    extended: bool,
    ruby: bool,
    reply: mpsc::SyncSender<Result<(), Vec<u8>>>,
}

fn validate_with(
    pattern: &[u8],
    encoding: Encoding,
    extended: bool,
    ruby: bool,
) -> Result<(), Vec<u8>> {
    static ENGINE: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Job>(1);
        // Onigmo の compiler/destructor は再帰する。rayon の 2MiB stack を使わず、
        // 上流の parse depth 4096 を保ったまま専用 stack 上で直列化する。
        std::thread::Builder::new()
            .name("sonicop-onigmo".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                // process-global な初期化・警告設定・byte buffer arena も同じ thread だけが触る。
                unsafe { sonicop_onigmo_init() };
                while let Ok(job) = receiver.recv() {
                    let result = native(&job.pattern, job.encoding, job.extended, job.ruby);
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
            ruby,
            reply,
        })
        .expect("regexp compiler worker stopped");
    result
        .recv()
        .expect("regexp compiler worker did not return")
}

fn native(pattern: &[u8], encoding: Encoding, extended: bool, ruby: bool) -> Result<(), Vec<u8>> {
    let mut error = vec![0u8; pattern.len().saturating_mul(6).saturating_add(2048)];
    // 入出力は呼出し中に生存し、C 側は指定した長さ以内だけを参照する。
    let function = if ruby {
        sonicop_onigmo_check
    } else {
        sonicop_onigmo_validate
    };
    // 長さ 0 でも C の範囲演算へ実在する配列の pointer を渡す。
    static EMPTY: [u8; 1] = [0];
    let pointer = if pattern.is_empty() {
        EMPTY.as_ptr()
    } else {
        pattern.as_ptr()
    };
    let result = unsafe {
        function(
            pointer,
            pattern.len(),
            encoding as i32,
            i32::from(extended),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        let length = error
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(error.len());
        Err(error[..length].to_vec())
    }
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
}
