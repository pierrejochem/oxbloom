//! Pins the generated project's `tokio` version to the workspace lockfile.

use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    let lock = fs::read_to_string("../../Cargo.lock").expect("read workspace Cargo.lock");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == "name = \"tokio\"" {
            let version = lines
                .next()
                .and_then(|l| l.strip_prefix("version = \""))
                .and_then(|l| l.strip_suffix('"'))
                .expect("tokio entry in Cargo.lock has a version line");
            println!("cargo:rustc-env=OXBLOOM_TOKIO_VERSION={version}");
            return;
        }
    }
    panic!("tokio not found in workspace Cargo.lock; the generated project cannot be pinned");
}
