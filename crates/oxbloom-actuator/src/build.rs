//! Build-script support: records the build name, version and git state.
//!
//! Call [`emit`] from the application's `build.rs`, then read the values in the application
//! with [`build_info!`](crate::build_info).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Runs `git` with `args` and returns its trimmed output, or `None` on any failure.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The facts that [`emit`] gathers before it prints anything.
///
/// Gathering touches the environment, `git` and the file system; [`render`] turns the facts
/// into the printed lines and is a pure function, so it can be unit tested.
#[derive(Debug, Default)]
struct Facts {
    name: Option<String>,
    version: Option<String>,
    sha: Option<String>,
    /// The output of `git rev-parse --abbrev-ref HEAD`, which is the literal `HEAD` on a
    /// detached checkout.
    branch: Option<String>,
    time: Option<u64>,
    /// The git files whose change must rerun the build script, in print order.
    watched: Vec<PathBuf>,
}

/// Returns the git files to watch: `HEAD`, the ref it points to and `packed-refs`.
///
/// The ref and `packed-refs` are listed only when they exist, because Cargo reruns a build
/// script on every build when a watched file is missing.
fn watched_git_files() -> Vec<PathBuf> {
    let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) else {
        return Vec::new();
    };
    let git_dir = Path::new(&git_dir);
    let mut watched = vec![git_dir.join("HEAD")];
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        let path = git_dir.join(reference);
        if path.is_file() {
            watched.push(path);
        }
    }
    let packed = git_dir.join("packed-refs");
    if packed.is_file() {
        watched.push(packed);
    }
    watched
}

/// Turns the gathered facts into the lines that [`emit`] prints.
fn render(facts: &Facts) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(name) = &facts.name {
        lines.push(format!("cargo:rustc-env=OXBLOOM_BUILD_NAME={name}"));
    }
    if let Some(version) = &facts.version {
        lines.push(format!("cargo:rustc-env=OXBLOOM_BUILD_VERSION={version}"));
    }
    if let Some(sha) = &facts.sha {
        lines.push(format!("cargo:rustc-env=OXBLOOM_GIT_SHA={sha}"));
    }
    if let Some(branch) = facts.branch.as_deref().filter(|branch| *branch != "HEAD") {
        lines.push(format!("cargo:rustc-env=OXBLOOM_GIT_BRANCH={branch}"));
    }
    if let Some(secs) = facts.time {
        lines.push(format!("cargo:rustc-env=OXBLOOM_BUILD_TIME={secs}"));
    }
    lines.push("cargo:rerun-if-changed=build.rs".to_string());
    for path in &facts.watched {
        lines.push(format!("cargo:rerun-if-changed={}", path.display()));
    }
    lines
}

/// Prints the `cargo:rustc-env=` lines that [`build_info!`](crate::build_info) reads.
///
/// The name and version come from `CARGO_PKG_NAME` and `CARGO_PKG_VERSION` of the calling
/// build script. The git sha and branch are left out when `git` is missing or fails, and the
/// branch is also left out on a detached checkout. The build never fails because of it.
///
/// The build script reruns when `build.rs`, the git `HEAD`, the current branch ref or
/// `packed-refs` change, so a new commit or a branch switch refreshes the recorded sha.
/// Outside a git checkout only `build.rs` is watched.
///
/// # Examples
///
/// ```no_run
/// // The body of `fn main` in the application's build.rs:
/// oxbloom_actuator::build::emit();
/// ```
pub fn emit() {
    let facts = Facts {
        name: std::env::var("CARGO_PKG_NAME").ok(),
        version: std::env::var("CARGO_PKG_VERSION").ok(),
        sha: git(&["rev-parse", "--short", "HEAD"]),
        branch: git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .ok(),
        watched: watched_git_files(),
    };
    for line in render(&facts) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_prints_every_fact_and_the_watched_git_files() {
        let facts = Facts {
            name: Some("todos".to_string()),
            version: Some("1.2.3".to_string()),
            sha: Some("abc1234".to_string()),
            branch: Some("main".to_string()),
            time: Some(42),
            watched: vec![
                PathBuf::from("/repo/.git/HEAD"),
                PathBuf::from("/repo/.git/refs/heads/main"),
                PathBuf::from("/repo/.git/packed-refs"),
            ],
        };
        assert_eq!(
            render(&facts),
            vec![
                "cargo:rustc-env=OXBLOOM_BUILD_NAME=todos",
                "cargo:rustc-env=OXBLOOM_BUILD_VERSION=1.2.3",
                "cargo:rustc-env=OXBLOOM_GIT_SHA=abc1234",
                "cargo:rustc-env=OXBLOOM_GIT_BRANCH=main",
                "cargo:rustc-env=OXBLOOM_BUILD_TIME=42",
                "cargo:rerun-if-changed=build.rs",
                "cargo:rerun-if-changed=/repo/.git/HEAD",
                "cargo:rerun-if-changed=/repo/.git/refs/heads/main",
                "cargo:rerun-if-changed=/repo/.git/packed-refs",
            ]
        );
    }

    #[test]
    fn render_outside_git_watches_only_the_build_script() {
        let facts = Facts {
            name: Some("todos".to_string()),
            ..Facts::default()
        };
        assert_eq!(
            render(&facts),
            vec![
                "cargo:rustc-env=OXBLOOM_BUILD_NAME=todos",
                "cargo:rerun-if-changed=build.rs",
            ]
        );
    }

    #[test]
    fn render_leaves_the_branch_out_on_a_detached_checkout() {
        let facts = Facts {
            sha: Some("abc1234".to_string()),
            branch: Some("HEAD".to_string()),
            ..Facts::default()
        };
        let lines = render(&facts);
        assert!(lines.contains(&"cargo:rustc-env=OXBLOOM_GIT_SHA=abc1234".to_string()));
        assert!(!lines.iter().any(|line| line.contains("OXBLOOM_GIT_BRANCH")));
    }

    #[test]
    fn watched_git_files_never_lists_a_missing_file() {
        for path in watched_git_files() {
            assert!(path.is_file(), "{} does not exist", path.display());
        }
    }
}
