#![cfg(feature = "web")]

mod common;

use oxbloom_test::Captured;

use std::error::Error;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oxbloom_boot::{
    Banner, ExitStatus, OxbloomApplication, Service, ServiceError, ShutdownHandle, StartupError,
    StartupPhase, WebServer,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_web::{App, HttpMethod, Route};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(2), f)
        .await
        .expect("test timed out")
}

fn ping_app() -> App {
    App::new()
        .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))
        .unwrap()
}

fn port_zero() -> Config {
    Config::from_str("[server]\nport = 0").unwrap()
}

type RunResult = Result<ExitStatus, StartupError>;

/// Runs an application with `server` registered as a `Service` bean.
fn spawn_run(server: &WebServer) -> (ShutdownHandle, JoinHandle<RunResult>) {
    let (handle, run) = spawn_timed_run(server);
    (handle, tokio::spawn(async move { run.await.unwrap().0 }))
}

/// Like [`spawn_run`], but also reports when `run` returned.
fn spawn_timed_run(server: &WebServer) -> (ShutdownHandle, JoinHandle<(RunResult, Instant)>) {
    let bean = server.clone();
    let builder = common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str("").unwrap())
        .output(std::io::sink())
        .configure(move |b| {
            b.instance(bean)
                .provides(|a: Arc<WebServer>| -> Arc<dyn Service> { a })
        });
    let handle = builder.shutdown_handle();
    let run = builder.run();
    (
        handle,
        tokio::spawn(async move {
            let result = run.await;
            (result, Instant::now())
        }),
    )
}

/// Polls `local_addr` until the server is bound.
async fn bound(server: &WebServer) -> std::net::SocketAddr {
    within(async {
        loop {
            if let Some(addr) = server.local_addr() {
                return addr;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
}

async fn get(addr: std::net::SocketAddr, path: &str) -> String {
    let mut conn = TcpStream::connect(addr).await.unwrap();
    conn.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut reply = String::new();
    conn.read_to_string(&mut reply).await.unwrap();
    reply
}

#[tokio::test]
async fn binds_port_zero_and_serves_a_request() {
    let server = WebServer::new(ping_app(), &port_zero()).unwrap();
    assert_eq!(server.local_addr(), None);
    let (handle, run) = spawn_run(&server);

    let addr = bound(&server).await;
    assert!(addr.ip().is_loopback());
    assert_ne!(addr.port(), 0);
    let reply = within(get(addr, "/ping")).await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.ends_with("pong"), "{reply}");

    handle.trigger();
    let status = within(run).await.unwrap().unwrap();
    assert_eq!(status.code(), 0);
    // The listener is closed once `run` returned.
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn in_flight_request_completes_before_run_returns() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let entered_tx = Arc::new(Mutex::new(Some(entered_tx)));
    let finished: Arc<Mutex<Option<Instant>>> = Arc::default();
    let done = finished.clone();
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/slow", move |_| {
            let entered = entered_tx.lock().unwrap().take();
            let done = done.clone();
            async move {
                if let Some(tx) = entered {
                    let _ = tx.send(());
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
                *done.lock().unwrap() = Some(Instant::now());
                "slow done"
            }
        }))
        .unwrap();
    let server = WebServer::new(app, &port_zero()).unwrap();
    let (handle, run) = spawn_timed_run(&server);
    let addr = bound(&server).await;

    let request = tokio::spawn(get(addr, "/slow"));
    within(entered_rx).await.unwrap();
    handle.trigger();

    let reply = within(request).await.unwrap();
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.ends_with("slow done"), "{reply}");
    let (status, returned) = within(run).await.unwrap();
    assert_eq!(status.unwrap().code(), 0);
    let finished = finished.lock().unwrap().expect("handler finished");
    assert!(finished <= returned);
}

#[tokio::test]
async fn port_in_use_is_service_phase_error() {
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    let config = Config::from_str(&format!("[server]\nport = {port}")).unwrap();
    let server = WebServer::new(ping_app(), &config).unwrap();
    let (_handle, run) = spawn_run(&server);

    let error = within(run).await.unwrap().unwrap_err();
    assert_eq!(error.phase(), StartupPhase::Service);
    let source = error.source().expect("source");
    assert!(source.downcast_ref::<ServiceError>().is_some());
    // The message names neither the address nor the port.
    let message = error.to_string();
    assert!(!message.contains(&port.to_string()), "{message}");
    assert!(!message.contains("127.0.0.1"), "{message}");
    assert_eq!(server.local_addr(), None);
}

#[test]
fn invalid_address_is_a_config_error() {
    let config = Config::from_str("[server]\naddress = \"nope\"").unwrap();
    let Err(error) = WebServer::new(ping_app(), &config) else {
        panic!("expected an error");
    };
    assert!(matches!(&error, ConfigError::Invalid { key, .. } if key == "server.address"));
    let message = error.to_string();
    assert!(!message.contains("nope"), "{message}");
}

#[test]
fn invalid_port_is_a_config_error() {
    let config = Config::from_str("[server]\nport = 70000").unwrap();
    assert!(WebServer::new(ping_app(), &config).is_err());
}

#[test]
fn server_must_be_a_table_when_present() {
    let config = Config::from_str("server = 1").unwrap();
    assert!(matches!(
        WebServer::new(ping_app(), &config),
        Err(ConfigError::Invalid { .. })
    ));
}

#[tokio::test]
async fn address_and_port_bind_from_command_line_strings() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::builder()
        .dir(dir.path())
        .env_from([])
        .args(["--server.port=0", "--server.address=127.0.0.1"])
        .build()
        .unwrap();
    let server = WebServer::new(ping_app(), &config).unwrap();
    within(server.start()).await.unwrap();
    let addr = server.local_addr().unwrap();
    assert!(addr.ip().is_loopback());
    assert_ne!(addr.port(), 0);
    within(server.stop()).await;
}

#[tokio::test]
async fn second_start_is_an_error() {
    let server = WebServer::new(ping_app(), &port_zero()).unwrap();
    within(server.start()).await.unwrap();
    let addr = server.local_addr().unwrap();
    let error = within(server.start()).await.unwrap_err();
    assert!(error.to_string().contains("already"), "{error}");
    // The first start is unaffected.
    assert_eq!(server.local_addr(), Some(addr));
    let reply = within(get(addr, "/ping")).await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    within(server.stop()).await;
}

#[tokio::test]
async fn stop_without_start_is_a_noop() {
    let server = WebServer::new(ping_app(), &port_zero()).unwrap();
    within(server.stop()).await;
    within(server.stop()).await;
    // Still startable afterwards.
    within(server.start()).await.unwrap();
    within(server.stop()).await;
    // A second stop after a real one is a no-op too.
    within(server.stop()).await;
}

#[test]
fn starts_last_and_stops_first() {
    let server = WebServer::new(ping_app(), &port_zero()).unwrap();
    assert_eq!(server.phase(), i32::MAX);
}

/// Runs `server` with a captured output and `failure_report(report)`.
async fn failing_run(server: &WebServer, report: bool) -> (StartupError, Captured) {
    let out = Captured::new();
    let bean = server.clone();
    let error = within(
        OxbloomApplication::builder()
            .with_config(Config::from_str("").unwrap())
            .output(out.clone())
            .failure_report(report)
            .configure(move |b| {
                b.instance(bean)
                    .provides(|a: Arc<WebServer>| -> Arc<dyn Service> { a })
            })
            .run(),
    )
    .await
    .unwrap_err();
    (error, out)
}

async fn taken_server() -> (TcpListener, u16, WebServer) {
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    let config = Config::from_str(&format!("[server]\nport = {port}")).unwrap();
    (taken, port, WebServer::new(ping_app(), &config).unwrap())
}

#[tokio::test]
async fn port_in_use_report_shows_address() {
    let (_taken, port, server) = taken_server().await;
    let (error, out) = failing_run(&server, true).await;

    assert_eq!(error.phase(), StartupPhase::Service);
    let analysis = error.analysis().expect("analysis");
    let description = format!(
        "The web server could not listen on `127.0.0.1:{port}` because the port is already in use."
    );
    assert_eq!(analysis.description(), description);
    let report = out.contents();
    assert!(report.contains(&description), "{report}");
    assert!(
        report.contains(
            "Stop the process that uses the port, or set `server.port` to a free port (0 picks one)."
        ),
        "{report}"
    );
}

#[tokio::test]
async fn port_report_is_the_only_place_with_the_address() {
    let (_taken, port, server) = taken_server().await;
    let (error, out) = failing_run(&server, true).await;
    assert!(out.contents().contains(&port.to_string()));
    let port = port.to_string();
    assert!(!error.to_string().contains(&port));
    assert!(!format!("{error:?}").contains(&port));

    // Without the report nothing is printed and the error still hides the port.
    let (_taken, port, server) = taken_server().await;
    let (error, out) = failing_run(&server, false).await;
    let port = port.to_string();
    assert!(!out.contents().contains("APPLICATION FAILED TO START"));
    assert!(!out.contents().contains(&port));
    assert!(!error.to_string().contains(&port));
    assert!(!format!("{error:?}").contains(&port));
}

#[tokio::test]
async fn run_and_exit_without_report_writes_only_the_error_line() {
    let (_taken, port, server) = taken_server().await;
    let out = Captured::new();
    let bean = server.clone();
    let code = within(
        OxbloomApplication::builder()
            .with_config(Config::from_str("").unwrap())
            .output(out.clone())
            .banner(Banner::off())
            .failure_report(false)
            .configure(move |b| {
                b.instance(bean)
                    .provides(|a: Arc<WebServer>| -> Arc<dyn Service> { a })
            })
            .run_and_exit(),
    )
    .await;
    assert_eq!(
        format!("{code:?}"),
        format!("{:?}", std::process::ExitCode::from(1))
    );

    // The same failure through `run()` gives the error whose line is printed.
    let (_taken2, _port2, server2) = taken_server().await;
    let (error, _) = failing_run(&server2, false).await;
    let line = error.to_string();
    let text = out.contents();
    // Boot's diagnostic line for the failed service, then the bare error line.
    assert_eq!(
        text,
        format!("service WebServer failed to start\n{line}\n"),
        "{text}"
    );
    assert!(!text.contains("APPLICATION FAILED TO START"), "{text}");
    assert!(!text.contains(&port.to_string()), "{text}");
}
