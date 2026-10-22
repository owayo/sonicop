#[path = "../crates/sonicop-prism-compat/src/build_support.rs"]
mod build_support;

const NODE_C: &str = include_str!("../crates/sonicop-prism-compat/vendor/src/node.c");
const DECLARATION: &str =
    "PRISM_EXPORTED_FUNCTION void\npm_node_destroy(pm_parser_t *parser, pm_node_t *node)";
const RENAMED: &str = "static void\nsonicop_node_destroy_one(pm_parser_t *parser, pm_node_t *node)";

fn newline_cases(source: &str) -> [String; 3] {
    [
        source.to_owned(),
        source.replace('\n', "\r\n"),
        source.replacen(DECLARATION, &DECLARATION.replace('\n', "\r\n"), 1),
    ]
}

fn assert_destructor_changed(source: &str) {
    let Err(error) = std::panic::catch_unwind(|| build_support::node_translation_unit(source))
    else {
        panic!("変更されたデストラクタを受理してはいけない");
    };
    let message = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(
        message.contains("Prism node destructor changed"),
        "{message}"
    );
}

#[test]
fn prism_translation_unit_preserves_lf_output_across_newlines() {
    let lf = NODE_C.replace("\r\n", "\n");
    assert_eq!(lf.matches(DECLARATION).count(), 2);
    // LF の既存生成処理を基準にし、改行形式ごとに同じ誤出力を返す退行も検出する。
    let expected = format!(
        "#include \"rename.h\"\n{}\n#include \"iterative_destroy.c\"\n",
        lf.replace(DECLARATION, RENAMED)
    );
    let cases = newline_cases(&lf);
    assert_eq!(cases[2].matches(DECLARATION).count(), 1);
    assert_eq!(
        cases[2].matches(&DECLARATION.replace('\n', "\r\n")).count(),
        1
    );
    for (index, source) in cases.iter().enumerate() {
        assert_eq!(
            build_support::node_translation_unit(source),
            expected,
            "改行ケース {index}"
        );
    }
}

#[test]
fn prism_destructor_changes_are_rejected_in_all_newline_styles() {
    let lf = NODE_C.replace("\r\n", "\n");
    let changed = DECLARATION.replace("pm_node_destroy", "pm_node_destroy_changed");
    let cases = [
        (0, lf.replace(DECLARATION, &changed)),
        (1, lf.replacen(DECLARATION, &changed, 1)),
        (3, format!("{lf}\n{DECLARATION};\n")),
    ];
    for (count, invalid) in cases {
        assert_eq!(invalid.matches(DECLARATION).count(), count);
        for source in newline_cases(&invalid) {
            assert_destructor_changed(&source);
        }
    }
}

#[test]
fn prism_destructor_normalization_does_not_hide_other_changes() {
    let lf = NODE_C.replace("\r\n", "\n");
    // 単独 CR や識別子の破損は、CRLF の正規化で修復してよい変更ではない。
    for declaration in [
        DECLARATION.replace('\n', "\r"),
        DECLARATION.replace("pm_node_destroy", "pm_node_des\rtroy"),
    ] {
        assert_destructor_changed(&lf.replacen(DECLARATION, &declaration, 1));
    }
}
