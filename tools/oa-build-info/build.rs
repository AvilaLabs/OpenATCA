//! Build script: embeds the commit this binary was built from, so that a file it writes names the
//! source it came from and not whatever repository the program happens to run in. The commit
//! and whether tracked files differ from it are read with `git -C <this crate's directory>`;
//! `unknown` only if git is not available at build time. The script reruns when the sources, the
//! manifest or the repository's HEAD change, which is what moves either value.

use std::path::PathBuf;
use std::process::Command;

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let commit = git(&dir, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = git(&dir, &["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|s| !s.is_empty());
    let text = format!(
        "commit {commit}{}",
        if dirty {
            " (tracked files modified)"
        } else {
            ""
        }
    );
    println!("cargo:rustc-env=OA_BUILD_INFO={text}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../../tools");
    // The crates it is built from: a change to one of them is a change to the encoder too.
    println!("cargo:rerun-if-changed=../../crates");
    if let Some(p) = git(&dir, &["rev-parse", "--git-path", "HEAD"]) {
        let p = PathBuf::from(&dir).join(p);
        println!("cargo:rerun-if-changed={}", p.display());
    }
    // A commit on a branch moves the branch's ref, not HEAD.
    if let Some(r) = git(&dir, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(p) = git(&dir, &["rev-parse", "--git-path", &r]) {
            let p = PathBuf::from(&dir).join(p);
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
}
