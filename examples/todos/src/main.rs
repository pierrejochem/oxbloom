//! Runs the todos application.
//!
//! `cargo run -p todos -- --server.port=0` picks a free port and prints the
//! address. Ctrl-C or SIGTERM stops the server gracefully.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    // The configuration file sits next to this crate's `Cargo.toml`, so
    // `cargo run -p todos` finds it from any working directory.
    todos::application(env!("CARGO_MANIFEST_DIR"))
        .args(std::env::args().skip(1))
        .run_and_exit()
        .await
}
