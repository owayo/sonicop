pub(crate) fn node_translation_unit(original: &str) -> String {
    // Git の CRLF checkout でも宣言を検出し、vendor 自体は書き換えない。
    let normalized = original.replace("\r\n", "\n");
    let declaration =
        "PRISM_EXPORTED_FUNCTION void\npm_node_destroy(pm_parser_t *parser, pm_node_t *node)";
    // 上流の生成物は保存し、再帰の入口だけを反復ラッパーへ向ける。版更新時の取り残しも止める。
    assert_eq!(
        normalized.matches(declaration).count(),
        2,
        "Prism node destructor changed"
    );
    let implementation = normalized.replace(
        declaration,
        "static void\nsonicop_node_destroy_one(pm_parser_t *parser, pm_node_t *node)",
    );
    format!("#include \"rename.h\"\n{implementation}\n#include \"iterative_destroy.c\"\n")
}
