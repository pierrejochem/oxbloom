// A `Service` must be `Send + Sync`: one holding a `Cell` cannot be registered.
use std::cell::Cell;
use std::sync::Arc;

use oxbloom_boot::{async_trait, OxbloomApplication, Service, ServiceError};

struct S(Cell<u8>);

#[async_trait]
impl Service for S {
    async fn start(&self) -> Result<(), ServiceError> {
        Ok(())
    }

    async fn stop(&self) {}
}

fn main() {
    let _ = OxbloomApplication::builder().configure(|b| {
        b.instance(S(Cell::new(0)))
            .provides(|a: Arc<S>| -> Arc<dyn Service> { a })
    });
}
