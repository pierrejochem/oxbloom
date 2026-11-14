//! Build-script support: records the build name, version and git state.
//!
//! Call [`emit`] from the application's `build.rs`, then read the values in the application
//! with [`build_info!`](crate::build_info).

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

/// Prints the `cargo:rustc-env=` lines that [`build_info!`](crate::build_info) reads.
///
/// The name and version come from `CARGO_PKG_NAME` and `CARGO_PKG_VERSION` of the calling
/// build script. The git sha and branch are left out when `git` is missing or fails, and the
/// build never fails because of it.
///
/// # Examples
///
/// ```no_run
/// // In the application's build.rs:
/// fn main() {
///     oxbloom_actuator::build::emit();
/// }
/// ```
pub fn emit() {
    if let Ok(name) = std::env::var("CARGO_PKG_NAME") {
        println!("cargo:rustc-env=OXBLOOM_BUILD_NAME={name}");
    }
    if let Ok(version) = std::env::var("CARGO_PKG_VERSION") {
        println!("cargo:rustc-env=OXBLOOM_BUILD_VERSION={version}");
    }
    if let Some(sha) = git(&["rev-parse", "--short", "HEAD"]) {
        println!("cargo:rustc-env=OXBLOOM_GIT_SHA={sha}");
    }
    if let Some(branch) = git(&["rev-parse", "--abbrev-ref", "HEAD"]) {
        println!("cargo:rustc-env=OXBLOOM_GIT_BRANCH={branch}");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs());
    if let Ok(secs) = now {
        println!("cargo:rustc-env=OXBLOOM_BUILD_TIME={secs}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
