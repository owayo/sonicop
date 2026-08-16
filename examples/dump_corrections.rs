//! Dumps every correction each cop asks for, as one JSON line per file.
//!
//! The `-A` comparison against RuboCop can only see what survives to the final bytes. A cop that
//! asks for the wrong *shape* of edit -- an insertion where upstream replaces, one wide edit where
//! upstream makes several narrow ones -- produces the same output on its own, and only diverges
//! once another cop corrects inside the same span. Five of the eight real bugs found on
//! 2026-08-16 were of that kind, and every one of them sat under a byte-identical corpus run.
//!
//! This dumps the layer underneath, so the comparison can be made against upstream's own
//! `corrector` calls rather than against the text they produce.
//!
//! ```text
//! cargo run --release --example dump_corrections -- <path>...
//! ```
//!
//! `kind` is derived from the edit's shape, which is what maps onto upstream's calls:
//! an empty range is an `insert_before` / `insert_after`, an empty replacement is a `remove`,
//! and anything else is a `replace`. Upstream's `wrap` arrives here as two insertions at the
//! ends of the wrapped range, so the comparison has to fold those back together.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;
use sonicop::config::Config;
use sonicop::engine::{self, Selection};

#[derive(Serialize)]
struct FileEdits<'a> {
    path: &'a str,
    edits: Vec<EditRecord<'a>>,
}

#[derive(Serialize)]
struct EditRecord<'a> {
    cop: &'a str,
    kind: &'static str,
    start: usize,
    end: usize,
    text: &'a str,
}

fn main() -> Result<()> {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() {
        eprintln!("使い方: dump_corrections <ファイルかディレクトリ>...");
        std::process::exit(2);
    }
    let cwd = std::env::current_dir()?;
    // The comparison is against `--force-default-config`, so no repository configuration is read.
    let config = Arc::new(Config::load_with_options(None, &cwd, true)?);
    // `correcting` is upstream's `autocorrect?`, which a handful of cops branch on themselves.
    // Dumping with it false would ask a different question than the one `-A` asks.
    let selection = Selection {
        correcting: true,
        ..Selection::default()
    };

    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    for path in &paths {
        for file in ruby_files(path) {
            dump(&file, &config, &selection, &mut stdout)?;
        }
    }
    Ok(())
}

fn ruby_files(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    ignore::WalkBuilder::new(path)
        .build()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|path| path.extension().is_some_and(|extension| extension == "rb"))
        .collect()
}

fn dump(
    path: &Path,
    config: &Arc<Config>,
    selection: &Selection,
    out: &mut impl std::io::Write,
) -> Result<()> {
    let Ok(text) = std::fs::read_to_string(path) else {
        // A file the reader cannot decode is upstream's `Lint/Syntax` territory, not this tool's.
        return Ok(());
    };
    let report = engine::inspect_source(path, text, config, selection)
        .with_context(|| format!("{}: 検査に失敗した", path.display()))?;
    let edits: Vec<EditRecord<'_>> = report
        .offenses
        .iter()
        .flat_map(|offense| {
            offense.corrections.iter().map(|edit| EditRecord {
                cop: offense.cop_name,
                kind: kind_of(edit),
                start: edit.start,
                end: edit.end,
                text: &edit.replacement,
            })
        })
        .collect();
    if edits.is_empty() {
        return Ok(());
    }
    let record = FileEdits {
        path: &path.to_string_lossy(),
        edits,
    };
    serde_json::to_writer(&mut *out, &record)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Which of upstream's `corrector` calls the edit stands for.
fn kind_of(edit: &sonicop::diagnostic::Edit) -> &'static str {
    match (edit.start == edit.end, edit.replacement.is_empty()) {
        (true, _) => "insert",
        (false, true) => "remove",
        (false, false) => "replace",
    }
}
