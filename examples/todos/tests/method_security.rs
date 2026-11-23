//! Method security on `TodoAdmin::purge_done`: called as a service with `with_authentication`,
//! and through `DELETE /api/admin/done` with the configured demo users.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxbloom_boot::{RunListener, WebServer};
use oxbloom_di::Container;
use oxbloom_security::{AccessDenied, Authentication, GrantedAuthority};
use oxbloom_test::{TestClient, with_authentication};
use todos::admin::TodoAdmin;
use todos::todo::TodoStore;
use tokio::sync::oneshot;

/// Sends the server's address once the application is ready.
struct Ready(Mutex<Option<oneshot::Sender<SocketAddr>>>);

impl RunListener for Ready {
    fn ready(&self, container: &Container) {
        let addr = container.get::<WebServer>().unwrap().local_addr().unwrap();
        if let Some(tx) = self.0.lock().unwrap().take() {
            let _ = tx.send(addr);
        }
    }
}

/// `Basic` credentials of the demo users: `admin:changeit` and `viewer:viewonly`.
const ADMIN: &str = "Basic YWRtaW46Y2hhbmdlaXQ=";
const VIEWER: &str = "Basic dmlld2VyOnZpZXdvbmx5";

#[tokio::test]
async fn the_service_checks_the_caller_without_http() {
    let store = Arc::new(TodoStore::default());
    let done = store.create("done".into());
    store.patch(done.id, None, Some(true));
    store.create("open".into());
    let service = TodoAdmin {
        store: store.clone(),
    };

    let user = Authentication::new("ada", [GrantedAuthority::new("ROLE_USER")]);
    let denied = with_authentication(user, async { service.purge_done() }).await;
    assert_eq!(denied, Err(AccessDenied::Forbidden));
    assert_eq!(
        store.list(None).len(),
        2,
        "a denied call must not run the body"
    );

    let admin = Authentication::new("root", [GrantedAuthority::new("ROLE_ADMIN")]);
    let removed = with_authentication(admin, async { service.purge_done() }).await;
    assert_eq!(removed, Ok(1));
    assert_eq!(store.list(None).len(), 1);
}

#[tokio::test]
async fn the_route_answers_401_403_and_200() {
    let (tx, rx) = oneshot::channel();
    let builder = todos::application(Path::new(env!("CARGO_MANIFEST_DIR")))
        .args(["--server.port=0"])
        .logging(false)
        .output(std::io::sink())
        .listener(Ready(Mutex::new(Some(tx))));
    let handle = builder.shutdown_handle();
    let run = tokio::spawn(builder.run());
    let addr = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .expect("test timed out")
        .expect("application ready");
    let client = TestClient::connect(addr);

    let anonymous = client.delete("/api/admin/done").send().await.into_inner();
    assert_eq!(anonymous.status().as_u16(), 401);
    let viewer = client
        .delete("/api/admin/done")
        .header("authorization", VIEWER)
        .send()
        .await
        .into_inner();
    assert_eq!(viewer.status().as_u16(), 403);
    let admin = client
        .delete("/api/admin/done")
        .header("authorization", ADMIN)
        .send()
        .await
        .into_inner();
    assert_eq!(admin.status().as_u16(), 200);
    assert_eq!(admin.body().as_ref(), br#"{"removed":0}"#);

    handle.trigger();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("test timed out")
        .unwrap()
        .unwrap();
}
