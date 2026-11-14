//! Property metadata: the stable JSON text, and the keys the builder lists.

use std::sync::{Arc, Mutex};

use oxbloom_boot::{
    AutoConfiguration, AutoConfigurations, Configuration, OxbloomApplication,
    OxbloomApplicationBuilder, PropertyMetadata, RunListener, boot_properties,
};
use oxbloom_config::Config;
use oxbloom_di::{ContainerBuilder, component};
use oxbloom_test::Captured;

#[test]
fn to_json_is_stable() {
    let list = [
        PropertyMetadata::new("a.b", "u16", Some("8080"), "x"),
        PropertyMetadata::new("c", "String", None, "y"),
    ];
    assert_eq!(
        PropertyMetadata::to_json(&list),
        r#"[{"key":"a.b","type":"u16","default":"8080","description":"x"},{"key":"c","type":"String","default":null,"description":"y"}]"#
    );
    assert_eq!(PropertyMetadata::to_json(&[]), "[]");
}

#[test]
fn to_json_escapes_quotes_backslashes_and_control_chars() {
    let list = [PropertyMetadata::new(
        "k",
        "String",
        None,
        "say \"hi\" \\ a\nb\u{1}",
    )];
    let json = PropertyMetadata::to_json(&list);
    assert!(json.contains(r#"say \"hi\" \\ a\nb\u0001"#), "{json}");
}

#[test]
fn builder_lists_boot_keys_sorted() {
    let keys: Vec<_> = OxbloomApplication::builder()
        .property_metadata()
        .iter()
        .map(|p| p.key)
        .collect();
    let mut expected = vec![
        "application.name",
        "oxbloom.config.import",
        "oxbloom.logging.enabled",
        "oxbloom.main.banner-mode",
        "oxbloom.main.conditions-report",
        "oxbloom.profiles.active",
        "oxbloom.profiles.group",
        "server.ssl.bundle",
    ];
    // The `logging` feature adds the `logging.*` keys, which sort after `application.name`.
    if cfg!(feature = "logging") {
        expected.splice(
            1..1,
            [
                "logging.console.color",
                "logging.console.format",
                "logging.file.enabled",
                "logging.file.format",
                "logging.file.max-history",
                "logging.file.name",
                "logging.file.path",
                "logging.file.rotation",
                "logging.group.<name>",
                "logging.json.fields.<field>",
                "logging.json.names.<field>",
                "logging.level.<target>",
                "logging.level.root",
            ],
        );
    }
    assert_eq!(keys, expected);
    assert_eq!(boot_properties().len(), 8);
}

static ONE_TWO: [PropertyMetadata; 2] = [
    PropertyMetadata::new("x.two", "u8", None, "second"),
    PropertyMetadata::new("x.one", "u8", Some("1"), "first"),
];
static ONE: [PropertyMetadata; 1] = [PropertyMetadata::new("x.one", "String", None, "other")];

#[component]
struct Declares;
impl Configuration for Declares {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}
impl AutoConfiguration for Declares {
    fn properties() -> &'static [PropertyMetadata] {
        &ONE_TWO
    }
}

#[component]
struct Repeats;
impl Configuration for Repeats {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}
impl AutoConfiguration for Repeats {
    fn properties() -> &'static [PropertyMetadata] {
        &ONE
    }
}

fn keys(b: &oxbloom_boot::OxbloomApplicationBuilder) -> Vec<&'static str> {
    b.property_metadata().iter().map(|p| p.key).collect()
}

#[test]
fn builder_adds_registered_and_drops_excluded() {
    let list = AutoConfigurations::new().add::<Declares>().add::<Repeats>();
    let b = OxbloomApplication::builder().auto_configurations(list);
    let all = b.property_metadata();
    let ones: Vec<_> = all.iter().filter(|p| p.key == "x.one").collect();
    assert_eq!(ones.len(), 1, "a key declared twice appears once");
    assert_eq!(ones[0].type_name, "u8", "the first declaration wins");
    let k = keys(&b);
    assert!(k.contains(&"x.two"));
    assert!(k.windows(2).all(|w| w[0] < w[1]), "{k:?}");

    let b = b.exclude::<Declares>();
    assert_eq!(
        b.property_metadata()
            .iter()
            .find(|p| p.key == "x.one")
            .map(|p| p.type_name),
        Some("String"),
        "the excluded declaration no longer wins"
    );
    assert!(!keys(&b).contains(&"x.two"));
    let b = b.exclude::<Repeats>();
    assert!(!keys(&b).contains(&"x.one"));
}

fn boot_key(key: &str) -> PropertyMetadata {
    boot_properties()
        .iter()
        .find(|p| p.key == key)
        .cloned()
        .unwrap_or_else(|| panic!("{key} is not declared"))
}

#[test]
fn banner_mode_and_conditions_report_declare_type_and_default() {
    let banner = boot_key("oxbloom.main.banner-mode");
    assert_eq!(banner.type_name, "String");
    assert_eq!(banner.default, Some("console"));
    let report = boot_key("oxbloom.main.conditions-report");
    assert_eq!(report.type_name, "String");
    assert_eq!(report.default, Some("off"));
    let import = boot_key("oxbloom.config.import");
    assert_eq!(import.type_name, "array of String");
    assert_eq!(import.default, None);
}

#[test]
fn logging_enabled_declares_type_and_default() {
    let enabled = boot_key("oxbloom.logging.enabled");
    assert_eq!(enabled.type_name, "bool");
    assert_eq!(enabled.default, Some("true"));
}

#[test]
fn application_name_declares_type_and_default() {
    let name = boot_key("application.name");
    assert_eq!(name.type_name, "String");
    assert_eq!(name.default, Some("application"));
}

#[cfg(feature = "logging")]
#[test]
fn metadata_lists_logging_keys() {
    let list = OxbloomApplication::builder()
        .auto_configurations(AutoConfigurations::new().add::<Declares>())
        .exclude::<Declares>()
        .property_metadata();
    for &(key, type_name, default, description) in oxbloom_logging::PROPERTIES {
        let found = list.iter().find(|p| p.key == key);
        assert_eq!(
            found,
            Some(&PropertyMetadata::new(key, type_name, default, description)),
            "{key}"
        );
    }
    assert_eq!(oxbloom_logging::PROPERTIES.len(), 13);
    assert!(list.iter().any(|p| p.key == "oxbloom.logging.enabled"));
}

/// Runs an application with `toml` as its configuration, returning what it printed.
async fn output_of(toml: &str) -> String {
    let out = Captured::new();
    OxbloomApplication::builder()
        .with_config(Config::from_str(toml).unwrap())
        .output(out.clone())
        .run()
        .await
        .unwrap();
    out.contents()
}

#[tokio::test]
async fn banner_mode_key_switches_the_banner() {
    let console = output_of("oxbloom.main.banner-mode = \"console\"").await;
    assert!(console.contains(":: Oxbloom ::"), "{console:?}");
    let off = output_of("oxbloom.main.banner-mode = \"off\"").await;
    assert_eq!(off, "");
}

#[tokio::test]
async fn conditions_report_key_prints_the_report() {
    let console = output_of(
        "oxbloom.main.banner-mode = \"off\"\noxbloom.main.conditions-report = \"console\"",
    )
    .await;
    assert!(
        console.contains("CONDITIONS EVALUATION REPORT"),
        "{console:?}"
    );
    let off = output_of("oxbloom.main.banner-mode = \"off\"").await;
    assert!(!off.contains("CONDITIONS EVALUATION REPORT"), "{off:?}");
}

/// Remembers what the prepared environment holds.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<(Vec<String>, Option<String>)>>);

impl RunListener for Seen {
    fn environment_prepared(&self, config: &Config) {
        let imported = config.get::<String>("imported").ok();
        if let Ok(mut seen) = self.0.lock() {
            *seen = (config.active_profiles().to_vec(), imported);
        }
    }
}

/// Runs with `files` written to a temporary config directory and no environment.
async fn seen_with(files: &[(&str, &str)]) -> (Vec<String>, Option<String>) {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let path = dir.path().to_path_buf();
    let seen = Seen::default();
    let app: OxbloomApplicationBuilder = OxbloomApplication::builder()
        .config(move |b| b.dir(path).env_from([]))
        .banner(oxbloom_boot::Banner::off())
        .output(Captured::new())
        .listener(seen.clone());
    app.run().await.unwrap();
    let (profiles, imported) = seen.0.lock().unwrap().clone();
    (profiles, imported)
}

#[tokio::test]
async fn profiles_active_key_activates_profiles() {
    let (profiles, _) = seen_with(&[
        ("application.toml", "oxbloom.profiles.active = \"dev\"\n"),
        ("application-dev.toml", "x = 1\n"),
    ])
    .await;
    assert_eq!(profiles, ["dev"]);
}

#[tokio::test]
async fn profiles_group_key_expands_profiles() {
    let (profiles, _) = seen_with(&[
        (
            "application.toml",
            "oxbloom.profiles.active = \"prod\"\n[oxbloom.profiles.group]\nprod = [\"db\"]\n",
        ),
        ("application-prod.toml", ""),
        ("application-db.toml", ""),
    ])
    .await;
    assert_eq!(profiles, ["prod", "db"]);
}

#[tokio::test]
async fn config_import_key_merges_the_listed_file() {
    let (_, imported) = seen_with(&[
        (
            "application.toml",
            "oxbloom.config.import = [\"extra.toml\"]\n",
        ),
        ("extra.toml", "imported = \"yes\"\n"),
    ])
    .await;
    assert_eq!(imported.as_deref(), Some("yes"));
}

/// Whether the container of a run with `toml` held a `LoggingSystem`.
#[cfg(feature = "logging")]
async fn has_logging_bean(toml: &str) -> Result<bool, oxbloom_boot::StartupError> {
    #[derive(Clone, Default)]
    struct Bean(Arc<Mutex<bool>>);
    impl RunListener for Bean {
        fn context_ready(&self, container: &oxbloom_di::Container) {
            *self.0.lock().unwrap() = container.get::<oxbloom_logging::LoggingSystem>().is_ok();
        }
    }
    let bean = Bean::default();
    OxbloomApplication::builder()
        .with_config(Config::from_str(toml).unwrap())
        .output(Captured::new())
        .listener(bean.clone())
        .run()
        .await?;
    let found = *bean.0.lock().unwrap();
    Ok(found)
}

#[cfg(feature = "logging")]
#[tokio::test]
async fn logging_enabled_key_skips_the_install() {
    assert!(
        !has_logging_bean("oxbloom.logging.enabled = false")
            .await
            .unwrap()
    );
    let err = has_logging_bean("oxbloom.logging.enabled = \"yes\"")
        .await
        .unwrap_err();
    assert_eq!(err.phase(), oxbloom_boot::StartupPhase::Config);
    assert!(err.to_string().contains("oxbloom.logging.enabled"), "{err}");
}
