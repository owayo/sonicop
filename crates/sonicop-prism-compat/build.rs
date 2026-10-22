use std::path::{Path, PathBuf};

#[path = "src/build_support.rs"]
mod build_support;

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
    println!("cargo:rerun-if-changed=src/build_support.rs");
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
            build_support::node_translation_unit(&original)
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
