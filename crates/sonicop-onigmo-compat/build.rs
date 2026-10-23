use std::{env, fs, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let bits: usize = env::var("CARGO_CFG_TARGET_POINTER_WIDTH")
        .unwrap()
        .parse()
        .unwrap();
    let windows = env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows";
    // host の sizeof や Ruby を使わず、生成する target の ABI で設定する。
    let long = if windows { 4 } else { bits / 8 };
    let mut config = format!(
        "#define PACKAGE \"sonicop-onigmo-compat\"\n#include <stdint.h>\n#define STDC_HEADERS 1\n#define UNREACHABLE_RETURN(value) return (value)\n#ifdef __GNUC__\n#define RB_GNUC_EXTENSION __extension__\n#define RB_GNUC_EXTENSION_BLOCK(x) __extension__ ({{ x; }})\n#else\n#define RB_GNUC_EXTENSION\n#define RB_GNUC_EXTENSION_BLOCK(x) (x)\n#endif\n#define HAVE_STDINT_H 1\n#define HAVE_INTTYPES_H 1\n#define HAVE_STRING_H 1\n#define HAVE_STDLIB_H 1\n#define HAVE_SYS_TYPES_H 1\n#define SIZEOF_INT 4\n#define SIZEOF_SHORT 2\n#define SIZEOF_LONG {long}\n#define SIZEOF_LONG_LONG 8\n#define SIZEOF_VOIDP {}\n#define SIZEOF_SIZE_T {}\n",
        bits / 8,
        bits / 8
    );
    if env::var("CARGO_CFG_TARGET_ENDIAN").as_deref() == Ok("big") {
        config.push_str("#define WORDS_BIGENDIAN 1\n");
    }
    fs::write(output.join("config.h"), config).unwrap();
    // 原文の scanner を保ち、Ruby VM へのアクセスだけを byte buffer adapter に分ける。
    let source = fs::read_to_string(root.join("vendor/re.c"))
        .unwrap()
        .replace("\r\n", "\n");
    let begin = source.find("static int\nread_escaped_byte(").unwrap();
    let end = source[begin..]
        .find("\nVALUE\nrb_reg_check_preprocess(")
        .unwrap()
        + begin;
    fs::write(output.join("ruby_preprocess.inc"), &source[begin..end]).unwrap();
    let begin = source.find("static void\nrb_reg_expr_str(").unwrap();
    let end = source[begin..]
        .find("\nstatic VALUE\nrb_reg_desc(")
        .unwrap()
        + begin;
    let display = &source[begin..end];
    let signature = "rb_reg_expr_str(VALUE str, const char *s, long len,";
    assert_eq!(
        display.matches(signature).count(),
        1,
        "Ruby display boundary changed"
    );
    // Ruby String の long 長を Rust の slice 長へ合わせ、Windows LLP64 でも切り詰めない。
    let display = display.replace(
        signature,
        "rb_reg_expr_str(VALUE str, const char *s, size_t len,",
    );
    fs::write(output.join("ruby_regexp_display.inc"), display).unwrap();
    let string = fs::read_to_string(root.join("vendor/string.c"))
        .unwrap()
        .replace("\r\n", "\n");
    let begin = string.find("int\nrb_str_buf_cat_escaped_char(").unwrap();
    let end = string[begin..].find("\n}\n").unwrap() + begin + 3;
    fs::write(
        output.join("ruby_escaped_char.inc"),
        format!("static {}", &string[begin..end]),
    )
    .unwrap();
    let mut build = cc::Build::new();
    build
        .include(&output)
        .include(&root)
        .include(root.join("standalone"))
        .include(root.join("vendor"))
        .include(root.join("vendor/enc/unicode/17.0.0"))
        .define("ONIG_EXTERN", "extern")
        .warnings(false);
    if !windows {
        build
            .define("HAVE_ALLOCA_H", "1")
            .define("HAVE_UNISTD_H", "1");
    }
    for (index, name) in [
        "regcomp.c",
        "regparse.c",
        "regerror.c",
        "regenc.c",
        "regexec.c",
        "regsyntax.c",
        "st.c",
        "enc/ascii.c",
        "enc/us_ascii.c",
        "enc/utf_8.c",
        "enc/euc_jp.c",
        "enc/windows_31j.c",
        "enc/unicode.c",
    ]
    .iter()
    .enumerate()
    {
        let source_path = root.join("vendor").join(name);
        let source_path = if *name == "regparse.c" {
            let mut source = fs::read_to_string(&source_path)
                .unwrap()
                .replace("\r\n", "\n");
            // RUBY の VM 用 header/GC は使わず、文法を変える二つの条件だけ本家へ合わせる。
            // Ruby は binary の非 ASCII 名や空白を含む名前も受理し、WORD には限定しない。
            for (from, to) in [
                (
                    "# ifdef RUBY\n#  define ONIGENC_IS_CODE_NAME(enc, c)  TRUE",
                    "# if 1\n#  define ONIGENC_IS_CODE_NAME(enc, c)  TRUE",
                ),
                (
                    "#ifndef RUBY\n      ONIG_SYN_OP2_ESC_U_HEX4 |\n#endif",
                    "#if 0\n      ONIG_SYN_OP2_ESC_U_HEX4 |\n#endif",
                ),
            ] {
                assert_eq!(
                    source.matches(from).count(),
                    1,
                    "Ruby grammar boundary changed"
                );
                source = source.replace(from, to);
            }
            let adapted = output.join("ruby_regparse.c");
            fs::write(&adapted, source).unwrap();
            adapted
        } else {
            source_path
        };
        let source = source_path.to_string_lossy().replace('\\', "/");
        let wrapper = output.join(format!("onigmo_{index}.c"));
        fs::write(
            &wrapper,
            format!("#include \"rename.h\"\n#include \"{source}\"\n"),
        )
        .unwrap();
        build.file(wrapper);
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.flag("/utf-8");
    }
    build.file(root.join("shim.c")).compile("sonicop_onigmo");
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed=standalone");
    println!("cargo:rerun-if-changed=shim.c");
    println!("cargo:rerun-if-changed=rename.h");
}
