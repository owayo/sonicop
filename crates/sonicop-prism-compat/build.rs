use std::path::{Path, PathBuf};

fn c_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .expect("cannot read vendored Prism sources")
        .map(|entry| entry.expect("cannot read Prism source entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            c_sources(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "c") {
            sources.push(path);
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed=rename.h");
    println!("cargo:rerun-if-changed=shim.c");
    println!("cargo:rerun-if-changed=iterative_destroy.c");
    let mut sources = Vec::new();
    c_sources(&root.join("vendor/src"), &mut sources);
    let mut build = cc::Build::new();
    build
        .include(root.join("vendor/include"))
        .include(&root)
        .define("PRISM_EXPORT_SYMBOLS", "1")
        .warnings(false);
    for (index, source) in sources.iter().enumerate() {
        let wrapper = output.join(format!("prism_{index}.c"));
        let include = source.to_string_lossy().replace('\\', "/");
        let contents = if source == &root.join("vendor/src/node.c") {
            let original =
                std::fs::read_to_string(source).expect("cannot read Prism node destructor");
            let declaration = "PRISM_EXPORTED_FUNCTION void\npm_node_destroy(pm_parser_t *parser, pm_node_t *node)";
            // 上流の生成物は保存し、再帰の入口だけを反復ラッパーへ向ける。版更新時の取り残しも止める。
            assert_eq!(
                original.matches(declaration).count(),
                2,
                "Prism node destructor changed"
            );
            let implementation = original.replace(
                declaration,
                "static void\nsonicop_node_destroy_one(pm_parser_t *parser, pm_node_t *node)",
            );
            format!("#include \"rename.h\"\n{implementation}\n#include \"iterative_destroy.c\"\n")
        } else {
            // 各翻訳単位に先置きし、大域シンボルを他の Prism から分離する。
            format!("#include \"rename.h\"\n#include \"{include}\"\n")
        };
        std::fs::write(&wrapper, contents).expect("cannot generate Prism translation unit");
        build.file(wrapper);
    }
    build
        .file(root.join("shim.c"))
        .compile("sonicop_prism_1_8_1");
}
