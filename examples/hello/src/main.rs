//! Runs the hello application.
//!
//! `cargo run -p hello -- --server.port=0` picks a free port and prints the
//! address. `--oxbloom.profiles.active=dev` adds `application-dev.toml`.
//! `--hello.custom-greeter=true` replaces the auto-configured guest greeter
//! with the application's own, and `--oxbloom.main.conditions-report=console`
//! prints which conditional beans were kept or skipped, and why.
//! Ctrl-C or SIGTERM stops the server gracefully; the exit code then comes
//! from `hello.exit-code`.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    // The configuration files sit next to this crate's `Cargo.toml`, so
    // `cargo run -p hello` finds them from any working directory.
    hello::application(env!("CARGO_MANIFEST_DIR"))
        .args(std::env::args().skip(1))
        .run_and_exit()
        .await
}
