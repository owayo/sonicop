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

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // The commit marker changes only with git. The embedded cache fingerprint, however, must move
    // with every input that can change diagnostics so a development build never accepts reports
    // from different code merely because the package version stayed put.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    for input in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "config/default.yml",
        "src",
    ] {
        println!("cargo:rerun-if-changed={input}");
    }
    let fingerprint = build_fingerprint().expect("failed to fingerprint Sonicop build inputs");
    println!("cargo:rustc-env=SONICOP_BUILD_FINGERPRINT={fingerprint}");

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

/// Stable identity of everything that can affect the executable's lint result.
///
/// Hashing this once while compiling replaces reading and hashing the entire executable at every
/// startup. Besides source and embedded configuration, the lockfile covers dependency revisions;
/// compiler/target/cfg inputs cover builds whose source tree is byte-identical but semantics are
/// not. Test and documentation files are intentionally absent because they never enter the binary.
fn build_fingerprint() -> io::Result<blake3::Hash> {
    let mut fingerprint = blake3::Hasher::new();
    for input in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "config/default.yml",
        "src",
    ] {
        hash_path(Path::new(input), &mut fingerprint)?;
    }

    let mut environment = std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            matches!(
                name.as_ref(),
                "TARGET" | "PROFILE" | "OPT_LEVEL" | "DEBUG" | "CARGO_ENCODED_RUSTFLAGS"
            ) || name.starts_with("CARGO_CFG_")
                || name.starts_with("CARGO_FEATURE_")
        })
        .collect::<Vec<(OsString, OsString)>>();
    environment.sort_unstable();
    for (name, value) in environment {
        hash_part(&mut fingerprint, name.as_encoded_bytes());
        hash_part(&mut fingerprint, value.as_encoded_bytes());
    }

    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let version = Command::new(rustc).arg("-vV").output()?;
    if !version.status.success() {
        return Err(io::Error::other("rustc -vV failed"));
    }
    hash_part(&mut fingerprint, &version.stdout);
    Ok(fingerprint.finalize())
}

fn hash_path(path: &Path, fingerprint: &mut blake3::Hasher) -> io::Result<()> {
    hash_part(fingerprint, path.as_os_str().as_encoded_bytes());
    if path.is_dir() {
        hash_part(fingerprint, b"directory");
        let mut children = fs::read_dir(path)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort_unstable();
        for child in children {
            hash_path(&child, fingerprint)?;
        }
    } else {
        hash_part(fingerprint, b"file");
        hash_part(fingerprint, &fs::read(path)?);
    }
    Ok(())
}

fn hash_part(fingerprint: &mut blake3::Hasher, bytes: &[u8]) {
    fingerprint.update(&(bytes.len() as u64).to_le_bytes());
    fingerprint.update(bytes);
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
