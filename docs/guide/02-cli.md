# CLI

```
cargo run -p oxbloom-cli -- new my-app --dir /some/parent   # creates /some/parent/my-app
cargo run -p oxbloom-cli -- list
```

`new` writes `.gitignore`, `oxbloom.toml`, `Cargo.toml`, `application.toml`, `src/lib.rs`, `src/main.rs` and `tests/hello.rs`. The project depends on `oxbloom-boot` (feature `web`), `oxbloom-config`, `oxbloom-di` and `oxbloom-web` by path (`--oxbloom-path`, default: this workspace); a path that lacks the `Cargo.toml` of one of these four crates is rejected with `not an oxbloom workspace: missing <path>` (exit code 2). The generated project has been verified to pass `cargo check` and `cargo test`, to serve requests and to stop with exit code 0 on Ctrl-C. `--security`, `--database`, `--jobs`, `--ai`, `--tests` and `--frontend` exit with `<feature> is not implemented yet`. Exit code 2 = bad input, 1 = filesystem error.

The generated `application.toml` sets the port; `--server.port=9000` or `APP_SERVER__PORT=9000` overrides it, and port 0 picks a free one. Boot reads it from the working directory:

```toml
[server]
port = 8080
```

The generated `src/main.rs` builds the web server from the routes of `src/lib.rs` and runs it through `OxbloomApplication`, so a startup failure prints a failure report and exits with code 1 (`my_app` stands for the crate name):

```rust
fn web_server(config: Arc<Config>) -> Result<WebServer, DiError> {
    Ok(WebServer::new(my_app::app(), &config)?)
}

#[tokio::main]
async fn main() -> ExitCode {
    OxbloomApplication::builder()
        .args(std::env::args().skip(1))
        .listener(Announce) // prints `listening on http://..` when ready
        .configure(|b| {
            b.try_bean(web_server)
                .provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s })
        })
        .run_and_exit()
        .await
}
```
