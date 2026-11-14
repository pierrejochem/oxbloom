//! A CRUD REST API on the Oxbloom web, security and logging stack.
//!
//! | File | Shows |
//! |---|---|
//! | `todo.rs` | a thread-safe in-memory store: `Mutex<BTreeMap>` plus an atomic id counter, ids from 1, never reused |
//! | `api.rs` | `#[controller("/api/todos")]` with `#[post("")]`, `#[get]`, `#[put]`, `#[patch]`, `#[delete]`, path parameters, a query filter, `Json<T>` answers, `Request::json` bodies, status codes, a `SecurityCustomizer` |
//! | `cleanup.rs` | `#[scheduling]` with one `#[scheduled(fixed_delay = "1m")]` method that removes done todos older than `todos.cleanup.max-age`, run by boot's `tasks` feature |
//! | `build.rs` | `oxbloom_actuator::build::emit()`, which feeds the build facts of `/actuator/info` |
//! | `application.toml` | `server.port`, `todos.cleanup.max-age` and the `logging.level` table (`root`, and `todos` for the handlers' events) |
//!
//! The handlers log with `tracing::info!` at target `todos`; boot installs the
//! logging subscriber, so the events show on the console. `main.rs` adds the
//! command line and runs [`application`]; the tests add a `ShutdownHandle`
//! instead of a signal.
//!
//! ```text
//! cargo run -p todos -- --server.port=0
//! curl -X POST -H 'content-type: application/json' -d '{"title":"try it"}' http://127.0.0.1:<port>/api/todos
//! ```

pub mod api;
pub mod cleanup;
pub mod todo;

use std::path::PathBuf;
use std::sync::Arc;

use oxbloom_boot::{
    OxbloomApplication, OxbloomApplicationBuilder, RunListener, SecurityCustomizer, WebServer,
};
use oxbloom_di::Container;
use oxbloom_security::SECRET_KEY;
use oxbloom_tasks::Scheduling;
use oxbloom_web::Controller;

use crate::api::{PublicPaths, TodoController};
use crate::cleanup::{TodoCleanup, cleanup_bean};
use crate::todo::TodoStore;

/// Prints the address once the server listens.
struct Announce;

impl RunListener for Announce {
    fn ready(&self, container: &Container) {
        if let Some(addr) = container
            .get::<WebServer>()
            .ok()
            .and_then(|s| s.local_addr())
        {
            println!("listening on http://{addr}");
        }
    }
}

/// The build facts for `/actuator/info`, set by this crate's `build.rs`.
///
/// # Examples
///
/// ```
/// let info = todos::build_info();
/// assert_eq!(info.name.as_deref(), Some("todos"));
/// ```
pub fn build_info() -> oxbloom_actuator::BuildInfo {
    oxbloom_actuator::build_info!()
}

/// The application, reading `application.toml` from `config_dir`.
///
/// The caller adds the arguments, for example `--server.port=0`, and runs it.
///
/// # Examples
///
/// ```
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let builder = todos::application(env!("CARGO_MANIFEST_DIR"))
///     .args(["--server.port=0"])
///     .logging(false)
///     .output(std::io::sink());
/// let handle = builder.shutdown_handle();
/// handle.trigger(); // stop right after the start
/// assert_eq!(builder.run().await?.code(), 0);
/// # Ok(())
/// # }
/// ```
pub fn application(config_dir: impl Into<PathBuf>) -> OxbloomApplicationBuilder {
    let config_dir = config_dir.into();
    OxbloomApplication::builder()
        .config(move |b| b.dir(config_dir).secret_key(SECRET_KEY))
        .listener(Announce)
        .configure(|b| {
            b.bean(build_info)
                .bean(TodoStore::default)
                .try_bean(cleanup_bean)
                .provides(|c: Arc<TodoCleanup>| -> Arc<dyn Scheduling> { c })
                .component::<TodoController>()
                .provides(|c: Arc<TodoController>| -> Arc<dyn Controller> { c })
                .bean(|| PublicPaths)
                .provides(|p: Arc<PublicPaths>| -> Arc<dyn SecurityCustomizer> { p })
        })
        .auto_configurations(oxbloom_boot::auto_configurations())
}
