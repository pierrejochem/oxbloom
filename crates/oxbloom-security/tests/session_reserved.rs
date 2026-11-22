//! The reserved session key prefix `oxbloom.security.`: `Session::set` rejects it, `remove`
//! ignores it, `get` reads it, and Oxbloom's own login, logout, CSRF and saved-request code keeps
//! working over both session backends.

use std::sync::Arc;

use http::{HeaderValue, StatusCode, header};
use oxbloom_config::Secret;
use oxbloom_core::SystemClock;
use oxbloom_security::{
    Argon2PasswordEncoder, CookieSessionBackend, DaoAuthenticationProvider, HttpSecurity,
    InMemorySessionStore, InMemoryUserDetailsService, PasswordEncoder, RequestSecurityExt,
    RequestSessionExt, SessionBackend, SessionError, SessionFilter, SessionSettings, UserDetails,
};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};

const NAME: &str = "OXBLOOM_SESSION";
const SECRET: &str = "a-session-secret-of-at-least-32-bytes!";

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

fn filter(backend: SessionBackend) -> SessionFilter {
    SessionFilter::new(backend, SessionSettings::default()).unwrap()
}

/// Tries to set `key`, answers the error text or `ok`.
fn set_route() -> Route {
    Route::new(HttpMethod::Get, "/set", |req: Request| async move {
        let key = req.query.clone().unwrap_or_default();
        match req.session().unwrap().set(&key, "planted") {
            Ok(()) => "ok".to_owned(),
            Err(e) => format!("{e:?}|{e}"),
        }
    })
}

fn remove_route() -> Route {
    Route::new(HttpMethod::Get, "/remove", |req: Request| async move {
        let key = req.query.clone().unwrap_or_default();
        req.session().unwrap().remove(&key);
        "removed"
    })
}

fn get_route() -> Route {
    Route::new(HttpMethod::Get, "/get", |req: Request| async move {
        let key = req.query.clone().unwrap_or_default();
        format!("{:?}", req.session().unwrap().get::<String>(&key).unwrap())
    })
}

/// Writes through the hidden bypass, as the crate's own writers do.
fn set_reserved_route() -> Route {
    Route::new(
        HttpMethod::Get,
        "/set-reserved",
        |req: Request| async move {
            let key = req.query.clone().unwrap_or_default();
            req.session().unwrap().set_reserved(&key, "legit").unwrap();
            "ok"
        },
    )
}

fn remove_reserved_route() -> Route {
    Route::new(
        HttpMethod::Get,
        "/remove-reserved",
        |req: Request| async move {
            let key = req.query.clone().unwrap_or_default();
            req.session().unwrap().remove_reserved(&key);
            "ok"
        },
    )
}

fn app(backend: SessionBackend) -> App {
    App::new()
        .filter(filter(backend))
        .route(set_route())
        .unwrap()
        .route(remove_route())
        .unwrap()
        .route(get_route())
        .unwrap()
        .route(set_reserved_route())
        .unwrap()
        .route(remove_reserved_route())
        .unwrap()
}

fn body(res: &Response) -> String {
    String::from_utf8_lossy(res.body()).into_owned()
}

fn cookie_of(res: &Response) -> Option<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| s.split(';').next())
        .find(|pair| pair.starts_with(&format!("{NAME}=")))
        .map(str::to_owned)
}

/// A tiny browser that keeps the session cookie.
struct Browser {
    app: App,
    cookie: std::sync::Mutex<Option<String>>,
}

impl Browser {
    fn new(app: App) -> Self {
        Self {
            app,
            cookie: std::sync::Mutex::new(None),
        }
    }

    async fn send(&self, mut req: Request) -> Response {
        if let Some(c) = self.cookie.lock().unwrap().clone() {
            req.headers.insert(header::COOKIE, c.parse().unwrap());
        }
        req.headers
            .insert(header::HOST, HeaderValue::from_static("example.com"));
        let res = self.app.handle(req).await;
        if let Some(c) = cookie_of(&res) {
            let expired = res
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .any(|v| v.to_str().unwrap().contains("Max-Age=0"));
            *self.cookie.lock().unwrap() = if expired { None } else { Some(c) };
        }
        res
    }

    async fn get(&self, path: &str, query: &str) -> String {
        let mut req = Request::new(HttpMethod::Get, path);
        req.query = Some(query.to_owned());
        body(&self.send(req).await)
    }
}

const RESERVED: [&str; 4] = [
    "oxbloom.security.authentication",
    "oxbloom.security.csrf-token",
    "oxbloom.security.principal",
    "oxbloom.security.x",
];

const LOOKALIKES: [&str; 4] = [
    "oxbloom.security",
    "oxbloom.securityX",
    "Oxbloom.Security.x",
    "app.oxbloom.security.x",
];

#[tokio::test]
async fn set_rejects_a_reserved_key_and_writes_nothing() {
    for (kind, backend) in backends() {
        let b = Browser::new(app(backend));
        for key in RESERVED {
            let answer = b.get("/set", key).await;
            assert!(answer.starts_with("ReservedKey|"), "{kind} {key}: {answer}");
            assert_eq!(b.get("/get", key).await, "None", "{kind} {key}");
        }
    }
}

#[tokio::test]
async fn the_error_text_is_fixed_and_does_not_contain_the_key() {
    for (kind, backend) in backends() {
        let b = Browser::new(app(backend));
        let answer = b.get("/set", "oxbloom.security.authentication").await;
        let text = answer.split_once('|').unwrap().1;
        assert_eq!(text, SessionError::ReservedKey.to_string(), "{kind}");
        assert!(!text.contains("oxbloom"), "{kind}: {text}");
        assert!(!text.contains("authentication"), "{kind}: {text}");
    }
}

#[tokio::test]
async fn remove_ignores_a_reserved_key() {
    for (kind, backend) in backends() {
        let b = Browser::new(app(backend));
        for key in RESERVED {
            assert_eq!(b.get("/set-reserved", key).await, "ok", "{kind} {key}");
            assert_eq!(b.get("/remove", key).await, "removed", "{kind} {key}");
            assert_eq!(
                b.get("/get", key).await,
                "Some(\"legit\")",
                "{kind} {key}: remove left the value"
            );
            b.get("/remove-reserved", key).await;
            assert_eq!(b.get("/get", key).await, "None", "{kind} {key}");
        }
    }
}

#[tokio::test]
async fn a_key_that_only_resembles_the_prefix_works() {
    for (kind, backend) in backends() {
        let b = Browser::new(app(backend));
        for key in LOOKALIKES {
            assert_eq!(b.get("/set", key).await, "ok", "{kind} {key}");
            assert_eq!(
                b.get("/get", key).await,
                "Some(\"planted\")",
                "{kind} {key}"
            );
            b.get("/remove", key).await;
            assert_eq!(b.get("/get", key).await, "None", "{kind} {key}");
        }
    }
}

#[tokio::test]
async fn get_of_a_reserved_key_works() {
    for (kind, backend) in backends() {
        let b = Browser::new(app(backend));
        b.get("/set-reserved", "oxbloom.security.principal").await;
        assert_eq!(
            b.get("/get", "oxbloom.security.principal").await,
            "Some(\"legit\")",
            "{kind}"
        );
    }
}

// --- Oxbloom's own writers still work ---

fn flow_app(backend: SessionBackend) -> App {
    let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    let users = InMemoryUserDetailsService::new(vec![
        UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
    ])
    .unwrap();
    let security = HttpSecurity::new()
        .password_provider(Arc::new(DaoAuthenticationProvider::new(
            Arc::new(users),
            encoder,
        )))
        .form_login(|f| f)
        .csrf(|c| c.enabled(false))
        .authorize(|a| a.any_request().authenticated());
    App::new()
        .filter(filter(backend))
        .filter(security.build().unwrap())
        .route(Route::new(
            HttpMethod::Get,
            "/secret",
            |req: Request| async move {
                req.authentication()
                    .map(|a| a.principal().to_string())
                    .unwrap_or_else(|| "anonymous".to_owned())
            },
        ))
        .unwrap()
}

#[tokio::test]
async fn the_csrf_token_still_persists_in_the_session() {
    for (kind, backend) in backends() {
        let app = App::new()
            .filter(filter(backend))
            .route(Route::new(
                HttpMethod::Get,
                "/token",
                |req: Request| async move {
                    req.csrf_token()
                        .map(|t| t.as_str().to_owned())
                        .unwrap_or_else(|| "none".to_owned())
                },
            ))
            .unwrap();
        let b = Browser::new(app);
        let token = b.get("/token", "").await;
        assert_eq!(token.len(), 43, "{kind}");
        assert_eq!(b.get("/token", "").await, token, "{kind}");
    }
}

#[tokio::test]
async fn login_logout_and_saved_request_still_work() {
    for (kind, backend) in backends() {
        let b = Browser::new(flow_app(backend));
        let mut req = Request::new(HttpMethod::Get, "/secret");
        req.headers
            .insert(header::ACCEPT, HeaderValue::from_static("text/html"));
        let res = b.send(req).await;
        assert_eq!(res.status(), StatusCode::FOUND, "{kind}");

        let mut req = Request::new(HttpMethod::Post, "/login");
        req.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        req.body = b"username=alice&password=s3cret".to_vec().into();
        let res = b.send(req).await;
        assert_eq!(res.status(), StatusCode::FOUND, "{kind}");
        assert_eq!(
            res.headers().get(header::LOCATION).unwrap(),
            "/secret",
            "{kind}: the saved request"
        );
        assert_eq!(b.get("/secret", "").await, "alice", "{kind}");

        let res = b.send(Request::new(HttpMethod::Post, "/logout")).await;
        assert_eq!(res.status(), StatusCode::FOUND, "{kind}");
        assert_ne!(b.get("/secret", "").await, "alice", "{kind}");
    }
}
