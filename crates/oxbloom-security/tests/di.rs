#![cfg(feature = "di")]

use std::sync::Arc;

use oxbloom_di::ContainerBuilder;
use oxbloom_security::{HttpSecurity, SecurityContainerExt, SecurityFilterChain};
use oxbloom_web::{App, HttpMethod, Request, Route};

fn chain() -> SecurityFilterChain {
    HttpSecurity::new()
        .authorize(|a| {
            a.get("/public/**")
                .permit_all()
                .any_request()
                .authenticated()
        })
        .build()
        .unwrap()
}

#[tokio::test]
async fn chain_resolves_from_container_and_guards_app() {
    let container = ContainerBuilder::new()
        .security_chain(chain())
        .build()
        .unwrap();
    let resolved = container.get::<SecurityFilterChain>().unwrap();
    let app = App::new()
        .filter(resolved)
        .route(Route::new(HttpMethod::Get, "/public/hello", |_| async {
            "hi"
        }))
        .unwrap()
        .route(Route::new(HttpMethod::Get, "/private/x", |_| async {
            "secret"
        }))
        .unwrap();

    let ok = app
        .handle(Request::new(HttpMethod::Get, "/public/hello"))
        .await;
    assert_eq!(ok.status(), 200);

    let denied = app
        .handle(Request::new(HttpMethod::Get, "/private/x"))
        .await;
    assert_eq!(denied.status(), 401);
    assert_eq!(denied.headers().get("www-authenticate").unwrap(), "Bearer");
}

#[test]
fn resolving_twice_returns_the_same_arc() {
    let container = ContainerBuilder::new()
        .security_chain(chain())
        .build()
        .unwrap();
    let a = container.get::<SecurityFilterChain>().unwrap();
    let b = container.get::<SecurityFilterChain>().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
}
