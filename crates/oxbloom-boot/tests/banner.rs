mod common;

use std::future::Future;
use std::time::Duration;

use oxbloom_boot::{Banner, OxbloomApplication, OxbloomApplicationBuilder, StartupPhase};
use oxbloom_config::Config;
use oxbloom_test::{Captured, Log};

fn default_banner() -> String {
    let version = format!("(v{})", env!("CARGO_PKG_VERSION"));
    let gap = " ".repeat(61 - " :: Oxbloom ::".len() - version.len());
    format!(
        "{}\n :: Oxbloom ::{gap}{version}\n",
        include_str!("banner_art.txt").trim_end()
    )
}

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

fn app(toml: &str, out: &Captured) -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str(toml).unwrap())
        .output(out.clone())
}

#[tokio::test]
async fn default_banner_names_oxbloom_and_version() {
    let out = Captured::new();
    within(app("", &out).run()).await.unwrap();
    assert_eq!(out.contents(), default_banner());
}

#[tokio::test]
async fn off_prints_nothing() {
    let out = Captured::new();
    within(app("", &out).banner(Banner::off()).run())
        .await
        .unwrap();
    assert_eq!(out.contents(), "");
}

#[tokio::test]
async fn custom_text() {
    let out = Captured::new();
    within(app("", &out).banner(Banner::text("My service 2.0")).run())
        .await
        .unwrap();
    assert_eq!(out.contents(), "My service 2.0\n");
}

#[tokio::test]
async fn property_off_overrides_builder() {
    let out = Captured::new();
    within(
        app("oxbloom.main.banner-mode = \"off\"", &out)
            .banner(Banner::text("hello"))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(out.contents(), "");
}

#[tokio::test]
async fn property_console_overrides_off() {
    let out = Captured::new();
    within(
        app("oxbloom.main.banner-mode = \"console\"", &out)
            .banner(Banner::off())
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(out.contents(), default_banner());
    // `console` keeps a custom banner.
    let out = Captured::new();
    within(
        app("oxbloom.main.banner-mode = \"console\"", &out)
            .banner(Banner::text("custom"))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(out.contents(), "custom\n");
}

#[tokio::test]
async fn invalid_mode_is_config_phase_without_the_value() {
    for value in ["zq-secret-7731", "Console", "OFF"] {
        let out = Captured::new();
        let err = within(app(&format!("oxbloom.main.banner-mode = \"{value}\""), &out).run())
            .await
            .unwrap_err();
        assert_eq!(err.phase(), StartupPhase::Config);
        let shown = format!("{err} {err:?}");
        assert!(!shown.contains(value), "{shown}");
        assert!(shown.contains("oxbloom.main.banner-mode"));
        assert!(!out.contents().contains(value));
        // Nothing is printed for a rejected mode.
        assert_eq!(out.contents(), "");
    }
}

#[tokio::test]
async fn non_string_mode_is_config_phase() {
    let out = Captured::new();
    let err = within(app("oxbloom.main.banner-mode = [1, 2]", &out).run())
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
}

struct Probe;

#[tokio::test]
async fn banner_printed_before_container_built() {
    let out = Captured::new();
    let log = Log::new();
    let (l, o) = (log.clone(), out.clone());
    within(
        app("", &out)
            .configure(move |b| {
                b.factory(move |_| {
                    l.push(format!("constructor saw {:?}", o.contents()));
                    Ok(Probe)
                })
            })
            .configure(|b| b.bean(|_: std::sync::Arc<Probe>| 1u8))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(
        log.entries(),
        [format!("constructor saw {:?}", default_banner())]
    );
}
