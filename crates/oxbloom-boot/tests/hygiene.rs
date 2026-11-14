mod common;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use oxbloom_boot::async_trait;
use oxbloom_boot::{
    ApplicationArguments, ApplicationRunner, OxbloomApplication, RunnerError, StartupError,
    StartupPhase,
};
use oxbloom_config::{Config, Secret};
use oxbloom_test::Captured;

const MARKER: &str = "zq-secret-7731";

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

struct Failing;

#[async_trait]
impl ApplicationRunner for Failing {
    async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
        Err(RunnerError::new("runner refused"))
    }
}

fn app(dir: &std::path::Path, out: &Captured) -> oxbloom_boot::OxbloomApplicationBuilder {
    let dir = dir.to_owned();
    common::quiet(OxbloomApplication::builder())
        .args([format!("--api.key={MARKER}"), "serve".to_owned()])
        .config(move |b| b.dir(dir).env_from([]))
        .output(out.clone())
}

fn assert_clean(err: &StartupError) {
    let shown = format!("{err} | {err:?}");
    assert!(!shown.contains(MARKER), "{shown}");
}

#[tokio::test]
async fn marker_never_leaks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("application.toml"),
        format!("plain = \"{MARKER}\"\n[db]\npassword = \"{MARKER}\"\n"),
    )
    .unwrap();

    // The arguments and config as seen by the application, captured by a bean.
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

    // Successful run.
    let out = Captured::new();
    let s = seen.clone();
    within(
        app(dir.path(), &out)
            .configure(move |b| {
                b.bean(move |a: Arc<ApplicationArguments>, c: Arc<Config>| {
                    let secret: Secret<String> = c.get("db.password").unwrap();
                    s.lock().unwrap().push(format!("{a:?}"));
                    s.lock().unwrap().push(format!("{c:?}"));
                    s.lock().unwrap().push(format!("{secret:?} {secret}"));
                    assert_eq!(secret.expose(), MARKER);
                    assert_eq!(a.option("api.key"), Some(MARKER));
                    assert_eq!(c.get::<String>("plain").unwrap(), MARKER);
                    7u8
                })
            })
            .listener(Eager)
            .run(),
    )
    .await
    .unwrap();
    assert!(!out.contents().contains(MARKER), "{}", out.contents());

    // Failing runner.
    let out = Captured::new();
    let s = seen.clone();
    let err = within(
        app(dir.path(), &out)
            .configure(move |b| {
                b.instance(Failing)
                    .provides(|f: Arc<Failing>| -> Arc<dyn ApplicationRunner> { f })
                    .bean(move |a: Arc<ApplicationArguments>, c: Arc<Config>| {
                        s.lock().unwrap().push(format!("{a:?} {c:?}"));
                        7u16
                    })
            })
            .listener(Eager)
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Runner);
    assert_clean(&err);
    assert!(!out.contents().contains(MARKER), "{}", out.contents());

    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty());
    for text in seen.iter() {
        assert!(!text.contains(MARKER), "{text}");
    }
    // Direct checks of the public Debug impls.
    let args = ApplicationArguments::new([format!("--api.key={MARKER}")]);
    assert!(!format!("{args:?}").contains(MARKER));
    assert!(!format!("{args}").contains(MARKER));
    assert_eq!(
        args.to_string(),
        "ApplicationArguments(1 options, 0 non-option arguments)"
    );
    let config = Config::from_str(&format!("[db]\npassword = \"{MARKER}\"")).unwrap();
    assert!(!format!("{config:?}").contains(MARKER));
}

/// Forces the lazily-created beans above to exist by resolving them on
/// `context_ready`.
struct Eager;

impl oxbloom_boot::RunListener for Eager {
    fn context_ready(&self, c: &oxbloom_di::Container) {
        let _ = c.get::<u8>();
        let _ = c.get::<u16>();
    }
}

#[tokio::test]
async fn config_phase_errors_do_not_leak_the_marker() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("application.toml"),
        format!("oxbloom.main.banner-mode = \"{MARKER}\"\n"),
    )
    .unwrap();
    let out = Captured::new();
    let err = within(app(dir.path(), &out).run()).await.unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    assert_clean(&err);
    assert!(!out.contents().contains(MARKER));
}
