//! Prints the offenses a file draws and the exact edits each of them asks for.
//!
//! The correction that reaches disk has been through the edit applier and the syntax guard, so a
//! cop that builds a broken set of edits shows up only as "not written". This prints what the cop
//! actually asked for.
//!
//! ```text
//! cargo run --release --example dump_corrections -- --config .rubocop.yml --only Style/Foo file.rb
//! ```

use std::path::PathBuf;
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let mut only = Vec::new();
    let mut config_path: Option<PathBuf> = None;
    let mut path: Option<PathBuf> = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--only" => only.push(arguments.next().expect("--only needs a cop")),
            "--config" => config_path = arguments.next().map(PathBuf::from),
            other => path = Some(PathBuf::from(other)),
        }
    }
    let path = path.expect("a file to inspect");
    let text = std::fs::read_to_string(&path)?;

    let cwd = std::env::current_dir()?;
    let config = sonicop::config::Config::load_with_options(
        config_path.as_deref(),
        &cwd,
        config_path.is_none(),
    )?;
    let selection = sonicop::engine::Selection {
        only: only.clone(),
        correcting: true,
        ..Default::default()
    };
    let report =
        sonicop::engine::inspect_source(&path, text.clone(), &Arc::new(config), &selection)?;
    for offense in &report.offenses {
        println!(
            "[{}] {}..{} {}",
            offense.cop_name, offense.start, offense.end, offense.message
        );
        for edit in &offense.corrections {
            println!(
                "    {}..{} -> {:?} (safe={})",
                edit.start, edit.end, edit.replacement, edit.safe
            );
        }
    }
    Ok(())
}
