//! Records which commit the binary next to this build was made from.
//!
//! The fingerprint a measurement prints -- the binary's own hash -- answers "which binary", not
//! "whose fixes are in it". Those are different questions, and on 2026-08-17 the gap cost the team
//! five re-runs of a sixty-minute gate and two reports of bugs that had already been fixed on a
//! branch nobody's tree had merged. Each side was rigorous on its own: one printed the fingerprint,
//! the other reported how far its branch was ahead. Neither answers the question in between.
//!
//! Writing the commit beside the binary closes it without touching the CLI. `--version` stays
//! byte-identical to what RuboCop prints, which the drop-in replacement depends on, and nothing
//! reads this file at run time -- it exists for the measurement scripts, which can now say
//! `git merge-base --is-ancestor <fix> <sha>` instead of asking a person.
//!
//! The file lands next to the executable rather than inside `OUT_DIR` so that the scripts can find
//! it from the binary path alone; they are handed a path to `sonicop`, not a Cargo layout. A build
//! that cannot reach git writes `unknown` rather than failing: this is a convenience for measuring,
//! and a release built from a source tarball has no repository to ask.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Only the git directory matters for staleness; rerunning on every source edit would be wrong
    // anyway, since the commit does not change when the working tree does.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");

    let Some(profile_dir) = profile_dir() else {
        return;
    };
    let sha = head_sha().unwrap_or_else(|| "unknown".to_owned());
    let dirty = match working_tree_is_dirty() {
        true => " dirty",
        false => "",
    };
    let _ = std::fs::write(
        profile_dir.join(".sonicop-build-sha"),
        format!("{sha}{dirty}\n"),
    );
}

/// `target/<profile>`, derived from where Cargo put this script's scratch space.
///
/// `OUT_DIR` is `target/<profile>/build/<pkg>-<hash>/out`, so the profile directory is three levels
/// up. Cargo has no environment variable that names it directly.
fn profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR")?);
    Some(out_dir.ancestors().nth(3)?.to_path_buf())
}

fn head_sha() -> Option<String> {
    let done = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    match done.status.success() {
        true => Some(String::from_utf8(done.stdout).ok()?.trim().to_owned()),
        false => None,
    }
}

/// Whether the tree the binary was built from carries uncommitted changes.
///
/// **A commit alone would overstate what the binary contains.** Half of the re-runs on 2026-08-17
/// were of trees whose commit was right and whose working copy had moved on, so the scripts need to
/// see the difference between "built from this commit" and "built from this commit plus edits".
fn working_tree_is_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .is_ok_and(|done| done.status.success() && !done.stdout.is_empty())
}
