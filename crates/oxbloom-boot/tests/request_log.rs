//! The request log over a real socket. A binary of its own: the subscriber is global, so
//! every scenario runs in order inside a single test and shares one install and one capture.
#![cfg(all(feature = "web", feature = "security", feature = "logging"))]

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxbloom_boot::{
    Banner, OxbloomApplication, RunListener, SecurityCustomizer, StartupPhase, WebServer,
    auto_configurations,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::Container;
use oxbloom_security::{HttpSecurity, RequestSecurityExt};
use oxbloom_test::Captured;
use oxbloom_web::{Controller, HttpMethod, Request, Route};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

const SECRET: &str = "an-example-secret-of-32-bytes!!!";
const TOKEN: &str = "s3cr3t-token-value";

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

/// Answers `/data` (authenticated) and `/open/x` (permitted).
struct Api;

impl Controller for Api {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![
            Route::new(HttpMethod::Get, "/data", |req: Request| async move {
                req.authentication().map(|_| "data").unwrap_or("nobody")
            }),
            Route::new(HttpMethod::Get, "/open/x", |_| async { "open" }),
            Route::new(HttpMethod::Get, "/open/boom", |_| async {
                if true {
                    panic!("hunter2");
                }
                "unreachable"
            }),
            Route::new(HttpMethod::Post, "/data", |_| async { "posted" }),
        ]
    }
}

struct OpenRules;

impl SecurityCustomizer for OpenRules {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| {
            a.path("/open/**")
                .permit_all()
                .any_request()
                .authenticated()
        })
    }
}

struct Ready(Mutex<Option<oneshot::Sender<SocketAddr>>>);

impl RunListener for Ready {
    fn ready(&self, container: &Container) {
        let addr = container.get::<WebServer>().unwrap().local_addr().unwrap();
        if let Some(tx) = self.0.lock().unwrap().take() {
            let _ = tx.send(addr);
        }
    }
}

/// The application with `request` as the extra `logging.request` section; `drive` talks to
/// it. Returns what the application printed and the log lines it added.
async fn run<F, Fut>(request: &str, logs: &Captured, drive: F) -> (String, Vec<String>)
where
    F: FnOnce(SocketAddr) -> Fut,
    Fut: Future<Output = ()>,
{
    let toml = format!(
        "[server]\nport = 0\n[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\n\
         [oxbloom.main]\nconditions-report = \"console\"\n\
         [logging.console]\ncolor = \"never\"\n{request}"
    );
    let (out, (tx, rx)) = (Captured::new(), oneshot::channel());
    let writer = logs.clone();
    let builder = OxbloomApplication::builder()
        .with_config(Config::from_str(&toml).unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .logging_writer(move || writer.clone())
        .auto_configurations(auto_configurations())
        .configure(|b| {
            b.instance(Api)
                .provides(|c: Arc<Api>| -> Arc<dyn Controller> { c })
                .instance(OpenRules)
                .provides(|c: Arc<OpenRules>| -> Arc<dyn SecurityCustomizer> { c })
        })
        .listener(Ready(Mutex::new(Some(tx))));
    let shutdown = builder.shutdown_handle();
    let task = tokio::spawn(builder.run());
    let before = logs.contents().len();
    let addr = within(rx).await.expect("application did not become ready");
    drive(addr).await;
    let all = logs.contents();
    shutdown.trigger();
    within(task).await.unwrap().unwrap();
    let lines = all[before..].lines().map(str::to_owned).collect();
    (out.contents(), lines)
}

/// The status of `GET path` with the extra raw `headers`.
async fn get(addr: SocketAddr, path: &str, headers: &str) -> u16 {
    let mut conn = TcpStream::connect(addr).await.unwrap();
    conn.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: x\r\n{headers}Connection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut reply = String::new();
    within(conn.read_to_string(&mut reply)).await.unwrap();
    reply.get(9..12).and_then(|s| s.parse().ok()).unwrap()
}

/// The whole reply to `method path` with a `body`.
async fn raw(addr: SocketAddr, method: &str, path: &str, headers: &str, body: &str) -> String {
    let mut conn = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\n{headers}Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    // The server may answer 413 and close before reading the whole body.
    let _ = conn.write_all(head.as_bytes()).await;
    let _ = conn.write_all(body.as_bytes()).await;
    let mut reply = String::new();
    let _ = within(conn.read_to_string(&mut reply)).await;
    reply
}

fn status_of(reply: &str) -> u16 {
    reply.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn request_events(lines: &[String]) -> Vec<&String> {
    lines
        .iter()
        .filter(|l| l.contains("oxbloom::web::request"))
        .collect()
}

#[tokio::test]
async fn request_log_scenarios() {
    let logs = Captured::new();

    // request_log_off_by_default: no event, and the conditions report names the skip.
    let (printed, lines) = run("", &logs, |addr| async move {
        assert_eq!(get(addr, "/open/x", "").await, 200);
    })
    .await;
    assert!(request_events(&lines).is_empty(), "{lines:#?}");
    let (_, skipped) = printed
        .split_once("Skipped:")
        .unwrap_or_else(|| panic!("no skipped section: {printed}"));
    let line = skipped
        .lines()
        .find(|l| l.contains("RequestLog"))
        .unwrap_or_else(|| panic!("the request log is not named: {printed}"));
    assert!(line.contains("on_property_value"), "{line}");

    // request_log_on: one event with the status, the method and the path.
    let (_, lines) = run(
        "[logging.request]\nenabled = true",
        &logs,
        |addr| async move {
            assert_eq!(get(addr, "/open/x?token=abc", "").await, 200);
        },
    )
    .await;
    let events = request_events(&lines);
    assert_eq!(events.len(), 1, "{lines:#?}");
    assert!(events[0].contains("INFO"), "{events:#?}");
    assert!(events[0].contains("status=200"), "{events:#?}");
    assert!(events[0].contains("/open/x"), "{events:#?}");
    assert!(!events[0].contains("token"), "no query: {events:#?}");

    // request_log_redacts_authorization_header_on_allowlist: entries are trimmed, empty
    // entries are dropped, and a denied name prints as redacted.
    let toml = "[logging.request]\nenabled = true\nlevel = \"debug\"\n\
                headers = [\" Authorization \", \"\", \"X-Api-Key\", \" user-agent\"]";
    let (_, lines) = run(toml, &logs, |addr| async move {
        // The level is debug: raise the oxbloom::web::request target through the root.
        let h =
            format!("Authorization: Basic {TOKEN}\r\nX-Api-Key: {TOKEN}\r\nUser-Agent: probe\r\n");
        assert_eq!(get(addr, "/open/x", &h).await, 200);
    })
    .await;
    // Debug is below the default root level: no event, which also pins the configured level.
    assert!(request_events(&lines).is_empty(), "{lines:#?}");
    let toml = "[logging.request]\nenabled = true\n\
                headers = [\" Authorization \", \"\", \"X-Api-Key\", \" user-agent\"]";
    let (_, lines) = run(toml, &logs, |addr| async move {
        let h =
            format!("Authorization: Basic {TOKEN}\r\nX-Api-Key: {TOKEN}\r\nUser-Agent: probe\r\n");
        assert_eq!(get(addr, "/open/x", &h).await, 200);
    })
    .await;
    let events = request_events(&lines);
    assert_eq!(events.len(), 1, "{lines:#?}");
    assert!(events[0].contains("<redacted>"), "{events:#?}");
    assert!(events[0].contains("user-agent"), "trimmed: {events:#?}");
    assert!(events[0].contains("probe"), "{events:#?}");
    assert!(!events[0].contains(TOKEN), "{events:#?}");

    // request_log_records_security_rejections: the request log wraps the chain, so a 401 it
    // answers is logged, and the body is problem+json with the challenge kept.
    let (_, lines) = run(
        "[logging.request]\nenabled = true",
        &logs,
        |addr| async move {
            let reply = raw(addr, "GET", "/data", "", "").await;
            assert_eq!(status_of(&reply), 401, "{reply}");
            let lower = reply.to_ascii_lowercase();
            assert!(lower.contains("www-authenticate: bearer"), "{reply}");
            assert!(lower.contains("application/problem+json"), "{reply}");
            assert!(reply.contains("\"status\":401"), "{reply}");
        },
    )
    .await;
    let events = request_events(&lines);
    assert_eq!(events.len(), 1, "{lines:#?}");
    assert!(events[0].contains("status=401"), "{events:#?}");

    // request_log_records_413: an oversized body is refused before the chain, so it is 413
    // and not 401 although no credentials are sent, and the request log records it.
    let (_, lines) = run(
        "[logging.request]\nenabled = true",
        &logs,
        |addr| async move {
            let big = "x".repeat(1024 * 1024 + 1);
            let reply = raw(addr, "POST", "/data", "", &big).await;
            assert_eq!(status_of(&reply), 413, "{reply}");
        },
    )
    .await;
    let events = request_events(&lines);
    assert_eq!(events.len(), 1, "{lines:#?}");
    assert!(events[0].contains("status=413"), "{events:#?}");
    assert!(!events[0].contains("status=401"), "{events:#?}");

    // request_log_records_404_and_panic_500: unknown path and a panicking handler are
    // logged; the panic message is in neither the response nor the request event.
    let (_, lines) = run(
        "[logging.request]\nenabled = true",
        &logs,
        |addr| async move {
            assert_eq!(get(addr, "/open/missing", "").await, 404);
            let reply = raw(addr, "GET", "/open/boom", "", "").await;
            assert_eq!(status_of(&reply), 500, "{reply}");
            assert!(!reply.contains("hunter2"), "{reply}");
        },
    )
    .await;
    let events = request_events(&lines);
    assert_eq!(events.len(), 2, "{lines:#?}");
    assert!(events[0].contains("status=404"), "{events:#?}");
    assert!(events[1].contains("status=500"), "{events:#?}");
    assert!(events.iter().all(|e| !e.contains("hunter2")), "{events:#?}");

    // request_log_records_405: a known path with a method it does not serve is answered 405
    // with `Allow` by the router, and the request log records exactly that one event.
    let (_, lines) = run(
        "[logging.request]\nenabled = true",
        &logs,
        |addr| async move {
            let reply = raw(addr, "DELETE", "/open/x", "", "").await;
            assert_eq!(status_of(&reply), 405, "{reply}");
            assert!(reply.to_ascii_lowercase().contains("allow: "), "{reply}");
        },
    )
    .await;
    let events = request_events(&lines);
    assert_eq!(events.len(), 1, "{lines:#?}");
    assert!(events[0].contains("status=405"), "{events:#?}");
    assert!(events[0].contains("DELETE"), "{events:#?}");

    // invalid_level_fails_startup_naming_key: `off` and junk are refused, value not echoed.
    for bad in ["off", "hunter2"] {
        let err = OxbloomApplication::builder()
            .with_config(
                Config::from_str(&format!(
                    "[server]\nport = 0\n[logging.request]\nenabled = true\nlevel = \"{bad}\""
                ))
                .unwrap(),
            )
            .banner(Banner::off())
            .output(std::io::sink())
            .logging(false)
            .auto_configurations(auto_configurations())
            .configure(|b| {
                b.instance(Api)
                    .provides(|c: Arc<Api>| -> Arc<dyn Controller> { c })
            })
            .run()
            .await
            .unwrap_err();
        assert_eq!(err.phase(), StartupPhase::Container);
        let cause = err.find_cause::<ConfigError>().expect("a ConfigError");
        assert!(
            matches!(cause, ConfigError::Invalid { key, .. } if key == "logging.request.level"),
            "{cause:?}"
        );
        assert!(!format!("{err:?}{err}").contains("hunter2"));
    }
}
