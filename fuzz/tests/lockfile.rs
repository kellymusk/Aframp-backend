//! Lock-file consistency tests.
//!
//! These tests verify that `Cargo.lock` is committed to the repository and is
//! up-to-date with respect to `Cargo.toml`. They are intentionally
//! dependency-free (only `std`) so they can run without pulling in extra
//! crates.
//!
//! If one of these tests fails, it usually means that `Cargo.toml` or
//! `Cargo.lock` was modified (for example by accidentally running `cargo add`
//! while working on `tests/`). See `CONTRIBUTING.md` for guidance: do not
//! modify `Cargo.toml` or `Cargo.lock` on `dev-backend`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate the repository root by walking up from this crate's manifest
/// directory until a directory containing both `Cargo.toml` and `Cargo.lock`
/// (or a `.git` entry) is found.
fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` points at the `fuzz/` crate directory.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let mut current: Option<&Path> = Some(manifest_dir.as_path());
    while let Some(dir) = current {
        let has_git = dir.join(".git").exists();
        let has_lock = dir.join("Cargo.lock").exists();
        if has_git || has_lock {
            return dir.to_path_buf();
        }
        current = dir.parent();
    }

    panic!(
        "could not locate repository root starting from {}",
        manifest_dir.display()
    );
}

#[test]
fn cargo_lock_exists_at_repo_root() {
    let root = repo_root();
    let lock = root.join("Cargo.lock");
    assert!(
        lock.exists(),
        "Cargo.lock is missing at the repository root ({}). \
         The lock file must be committed. See CONTRIBUTING.md.",
        lock.display()
    );
}

#[test]
fn cargo_lock_is_tracked_by_git() {
    let root = repo_root();

    let output = Command::new("git")
        .arg("ls-files")
        .arg("--error-unmatch")
        .arg("Cargo.lock")
        .current_dir(&root)
        .output()
        .expect("failed to run `git ls-files`; is git installed?");

    assert!(
        output.status.success(),
        "Cargo.lock is not tracked by git (git ls-files --error-unmatch failed). \
         The lock file must be committed. See CONTRIBUTING.md.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn cargo_lock_is_up_to_date() {
    let root = repo_root();

    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--locked")
        .arg("--format-version")
        .arg("1")
        .current_dir(&root)
        .output()
        .expect("failed to run `cargo metadata`; is cargo installed?");

    assert!(
        output.status.success(),
        "`cargo metadata --locked` failed, which means Cargo.lock is out of date \
         with respect to Cargo.toml. Do not modify Cargo.toml or Cargo.lock on \
         dev-backend; see CONTRIBUTING.md.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}