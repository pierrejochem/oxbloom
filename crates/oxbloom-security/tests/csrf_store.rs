//! The CSRF token in the session and on the request, through `App::handle` with the session
//! filter and both session backends.

use std::sync::Arc;

use http::header;
use oxbloom_config::Secret;
use oxbloom_core::SystemClock;
use oxbloom_security::{
    CookieSessionBackend, InMemorySessionStore, RequestSessionExt, SessionBackend, SessionFilter,
    SessionSettings,
};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};

const NAME: &str = "OXBLOOM_SESSION";
const SECRET: &str = "a-session-secret-of-at-least-32-bytes!";
const KEY: &str = "oxbloom.security.csrf-token";

fn backends() -> Vec<(&'static str, SessionBackend)> {
    let settings = SessionSettings::default();
    let store = InMemorySessionStore::new(&settings, Arc::new(SystemClock));
    let cookie = CookieSessionBackend::new(
        &Secret::new(SECRET.to_owned()),
        &[],
        &settings,
        Arc::new(SystemClock),
    )
    .unwrap();
    vec![
        ("server", SessionBackend::Server(Arc::new(store))),
        ("cookie", SessionBackend::Cookie(Arc::new(cookie))),
    ]
}

fn app(backend: SessionBackend) -> App {
    App::new()
        .filter(SessionFilter::new(backend, SessionSettings::default()).unwrap())
        .route(Route::new(
            HttpMethod::Get,
            "/token",
            |req: Request| async move {
                req.csrf_token()
                    .map(|t| t.as_str().to_owned())
                    .unwrap_or_else(|| "none".to_owned())
            },
        ))
        .unwrap()
        .route(Route::new(
            HttpMethod::Get,
            "/stored",
            |req: Request| async move {
                let session = req.session().unwrap();
                session
                    .get::<String>(KEY)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "none".to_owned())
            },
        ))
        .unwrap()
        .route(Route::new(
            HttpMethod::Get,
            "/corrupt",
            |req: Request| async move {
                req.session().unwrap().set(KEY, "not-a-token").unwrap();
                "ok"
            },
        ))
        .unwrap()
        .route(Route::new(
            HttpMethod::Get,
            "/nothing",
            |_req: Request| async move { "ok" },
        ))
        .unwrap()
}

fn bare_app() -> App {
    App::new()
        .route(Route::new(
            HttpMethod::Get,
            "/token",
            |req: Request| async move {
                req.csrf_token()
                    .map(|t| t.as_str().to_owned())
                    .unwrap_or_else(|| "none".to_owned())
            },
        ))
        .unwrap()
}

fn body(res: &Response) -> String {
    String::from_utf8_lossy(res.body()).into_owned()
}

fn cookie_of(res: &Response) -> Option<String> {
    let set = res.headers().get(header::SET_COOKIE)?.to_str().ok()?;
    let pair = set.split(';').next()?;
    pair.starts_with(&format!("{NAME}="))
        .then(|| pair.to_owned())
}

async fn get(app: &App, path: &str, cookie: Option<&str>) -> Response {
    let mut req = Request::new(HttpMethod::Get, path);
    if let Some(c) = cookie {
        req.headers.insert(header::COOKIE, c.parse().unwrap());
    }
    app.handle(req).await
}

#[tokio::test]
async fn csrf_token_is_none_without_a_session_filter() {
    let res = bare_app()
        .handle(Request::new(HttpMethod::Get, "/token"))
        .await;
    assert!(res.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(body(&res), "none");
}

#[tokio::test]
async fn csrf_token_survives_a_cookie_backend_round_trip() {
    for (kind, backend) in backends() {
        let app = app(backend);
        let first = get(&app, "/token", None).await;
        let cookie = cookie_of(&first).unwrap_or_else(|| panic!("{kind}: no cookie"));
        let token = body(&first);
        assert_eq!(token.len(), 43, "{kind}");
        let second = get(&app, "/token", Some(&cookie)).await;
        assert_eq!(body(&second), token, "{kind}");
        let stored = get(&app, "/stored", Some(&cookie)).await;
        assert_eq!(body(&stored), token, "{kind}");
    }
}

#[tokio::test]
async fn a_request_that_never_asks_for_the_token_creates_no_session() {
    for (kind, backend) in backends() {
        let res = get(&app(backend), "/nothing", None).await;
        assert!(res.headers().get(header::SET_COOKIE).is_none(), "{kind}");
    }
}

#[tokio::test]
async fn a_corrupt_stored_token_is_replaced_without_an_error() {
    for (kind, backend) in backends() {
        let app = app(backend);
        let first = get(&app, "/corrupt", None).await;
        let cookie = cookie_of(&first).unwrap_or_else(|| panic!("{kind}: no cookie"));
        let res = get(&app, "/token", Some(&cookie)).await;
        assert_eq!(res.status(), 200, "{kind}");
        let token = body(&res);
        assert_eq!(token.len(), 43, "{kind}");
        assert_ne!(token, "not-a-token", "{kind}");
    }
}
