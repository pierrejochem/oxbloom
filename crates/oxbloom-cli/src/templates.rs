use crate::manifest::Manifest;

/// `tokio` version from the workspace lockfile at CLI build time (see `build.rs`).
pub const TOKIO_VERSION: &str = env!("OXBLOOM_TOKIO_VERSION");

const GITIGNORE: &str = "/target\n";

const CARGO_TOML: &str = r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

# Empty table: keeps the project buildable when created inside another Cargo workspace.
[workspace]

[dependencies]
{dependencies}tokio = { version = "{tokio}", features = ["rt-multi-thread", "macros"] }

[dev-dependencies]
tokio = { version = "{tokio}", features = ["rt", "macros"] }
"#;

const APPLICATION_TOML: &str = r#"# Application configuration, read at startup from the working directory.
# A `--server.port=9000` argument or `APP_SERVER__PORT=9000` overrides it;
# port 0 picks a free port.
[server]
port = 8080
"#;

const LIB_RS: &str = r#"use std::sync::Arc;

use oxbloom_web::{App, IntoResponse, Request, controller};

pub struct HelloController;

#[controller("/api/hello")]
impl HelloController {
    #[get("")]
    async fn index(&self, _req: Request) -> impl IntoResponse {
        "Hello from {name}"
    }
}

/// Builds the application with all controllers registered.
pub fn app() -> App {
    App::new()
        .routes(Arc::new(HelloController).routes())
        .expect("valid routes")
}
"#;

const MAIN_RS: &str = r#"//! Starts the application through `OxbloomApplication`.
//!
//! The port comes from `server.port` (see `application.toml`). Ctrl-C or
//! SIGTERM stops the server gracefully; a startup failure prints a report and
//! exits with a non-zero code.

use std::process::ExitCode;
use std::sync::Arc;

use oxbloom_boot::{OxbloomApplication, RunListener, Service, WebServer};
use oxbloom_config::Config;
use oxbloom_di::{Container, DiError};

/// Builds the web server from the routes in `lib.rs` and the `server.*` properties.
fn web_server(config: Arc<Config>) -> Result<WebServer, DiError> {
    Ok(WebServer::new({crate_name}::app(), &config)?)
}

/// Prints the address once the server is listening.
struct Announce;

impl RunListener for Announce {
    fn ready(&self, container: &Container) {
        if let Some(addr) = container
            .get::<WebServer>()
            .ok()
            .and_then(|s| s.local_addr())
        {
            println!("listening on http://{addr}");
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    OxbloomApplication::builder()
        .args(std::env::args().skip(1))
        .listener(Announce)
        .configure(|b| {
            // Registered as a `Service`, so boot starts and stops it.
            b.try_bean(web_server)
                .provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s })
        })
        .run_and_exit()
        .await
}
"#;

/// The oxbloom crates the generated project depends on, with extra dependency keys.
const DEPENDENCIES: [(&str, &str); 4] = [
    ("oxbloom-boot", r#", features = ["web"]"#),
    ("oxbloom-config", ""),
    ("oxbloom-di", ""),
    ("oxbloom-web", ""),
];

const HELLO_TEST: &str = r#"use oxbloom_web::{HttpMethod, Request};

#[tokio::test]
async fn hello_returns_greeting() {
    let res = {crate_name}::app()
        .handle(Request::new(HttpMethod::Get, "/api/hello"))
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "Hello from {name}");
}
"#;

/// Renders every project file from the manifest, sorted by path.
///
/// `oxbloom_path` is the oxbloom workspace root; each oxbloom crate becomes a TOML-escaped
/// path dependency below it.
pub fn render(manifest: &Manifest, oxbloom_path: &str) -> Vec<(String, String)> {
    let name = &manifest.project.name;
    let crate_name = name.replace('-', "_");
    // Each path is TOML-escaped, so any character in `oxbloom_path` stays literal.
    let dependencies: String = DEPENDENCIES
        .iter()
        .map(|(dep, extra)| {
            let path = toml::Value::String(format!("{oxbloom_path}/crates/{dep}"));
            format!("{dep} = {{ path = {path}{extra} }}\n")
        })
        .collect();
    let fill = |t: &str| {
        // `{dependencies}` goes last: it holds user input that must not be re-substituted.
        t.replace("{tokio}", TOKIO_VERSION)
            .replace("{crate_name}", &crate_name)
            .replace("{name}", name)
            .replace("{dependencies}", &dependencies)
    };
    vec![
        (".gitignore".into(), GITIGNORE.into()),
        ("Cargo.toml".into(), fill(CARGO_TOML)),
        ("application.toml".into(), APPLICATION_TOML.into()),
        ("oxbloom.toml".into(), manifest.to_toml()),
        ("src/lib.rs".into(), fill(LIB_RS)),
        ("src/main.rs".into(), fill(MAIN_RS)),
        ("tests/hello.rs".into(), fill(HELLO_TEST)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    fn files() -> Vec<(String, String)> {
        render(&Manifest::new("demo-app", "rest"), "/opt/rs")
    }

    fn get<'a>(f: &'a [(String, String)], p: &str) -> &'a str {
        &f.iter().find(|(n, _)| n == p).unwrap().1
    }

    #[test]
    fn file_list_is_sorted_and_exact() {
        let names: Vec<_> = files().into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            [
                ".gitignore",
                "Cargo.toml",
                "application.toml",
                "oxbloom.toml",
                "src/lib.rs",
                "src/main.rs",
                "tests/hello.rs"
            ]
        );
    }

    #[test]
    fn cargo_toml_has_path_dep_and_pinned_tokio() {
        let f = files();
        let c = get(&f, "Cargo.toml");
        assert!(c.contains("name = \"demo-app\""), "{c}");
        assert!(c.contains("edition = \"2024\""), "{c}");
        assert!(
            c.contains("oxbloom-web = { path = \"/opt/rs/crates/oxbloom-web\" }"),
            "{c}"
        );
        assert!(
            c.contains(&format!("tokio = {{ version = \"{TOKIO_VERSION}\"")),
            "{c}"
        );
    }

    #[test]
    fn cargo_toml_depends_on_boot_with_web_feature() {
        let f = files();
        let c = get(&f, "Cargo.toml");
        assert!(
            c.contains(
                "oxbloom-boot = { path = \"/opt/rs/crates/oxbloom-boot\", features = [\"web\"] }"
            ),
            "{c}"
        );
        let table: toml::Table = toml::from_str(c).unwrap();
        let deps = &table["dependencies"];
        for (dep, dir) in [
            ("oxbloom-config", "oxbloom-config"),
            ("oxbloom-di", "oxbloom-di"),
            ("oxbloom-web", "oxbloom-web"),
        ] {
            assert_eq!(
                deps[dep]["path"].as_str(),
                Some(format!("/opt/rs/crates/{dir}").as_str()),
                "{c}"
            );
        }
        let features: Vec<_> = deps["tokio"]["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(features, ["rt-multi-thread", "macros"], "{c}");
        assert_eq!(deps["tokio"]["version"].as_str(), Some(TOKIO_VERSION));
    }

    #[test]
    fn application_toml_sets_default_port() {
        let f = files();
        let table: toml::Table = toml::from_str(get(&f, "application.toml")).unwrap();
        assert_eq!(table["server"]["port"].as_integer(), Some(8080));
    }

    #[test]
    fn cargo_toml_escapes_special_paths() {
        let weird = "/tmp/a \"b\"\\c d";
        let f = render(&Manifest::new("demo", "rest"), weird);
        let table: toml::Table = toml::from_str(get(&f, "Cargo.toml")).expect("valid toml");
        for dep in [
            "oxbloom-boot",
            "oxbloom-config",
            "oxbloom-di",
            "oxbloom-web",
        ] {
            let path = table["dependencies"][dep]["path"].as_str().unwrap();
            assert_eq!(path, format!("{weird}/crates/{dep}"));
        }
    }

    #[test]
    fn oxbloom_toml_is_manifest() {
        let m = Manifest::new("demo-app", "rest");
        assert_eq!(get(&render(&m, "/opt/rs"), "oxbloom.toml"), m.to_toml());
    }

    #[test]
    fn sources_reference_project_contract() {
        let f = files();
        let lib = get(&f, "src/lib.rs");
        assert!(lib.contains("pub fn app() -> App"), "{lib}");
        assert!(lib.contains("#[controller(\"/api/hello\")]"), "{lib}");
        let main = get(&f, "src/main.rs");
        assert!(main.contains("OxbloomApplication::builder()"), "{main}");
        assert!(main.contains(".args(std::env::args().skip(1))"), "{main}");
        assert!(main.contains(".run_and_exit()"), "{main}");
        assert!(main.contains("fn main() -> ExitCode"), "{main}");
        assert!(main.contains("#[tokio::main]"), "{main}");
        assert!(main.contains("try_bean(web_server)"), "{main}");
        assert!(
            main.contains("provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s })"),
            "{main}"
        );
        assert!(
            main.contains("WebServer::new(demo_app::app(), &config)"),
            "{main}"
        );
        assert!(!main.contains("app().run("), "{main}");
        assert!(!main.contains("PORT"), "{main}");
        let t = get(&f, "tests/hello.rs");
        assert!(t.contains("demo_app::app()") && t.contains("200"), "{t}");
    }

    #[test]
    fn render_is_deterministic() {
        assert_eq!(files(), files());
    }

    #[test]
    fn placeholders_in_oxbloom_path_are_not_substituted() {
        let f = render(&Manifest::new("inj", "rest"), "/x/{name}x/{tokio}");
        let table: toml::Table = toml::from_str(get(&f, "Cargo.toml")).unwrap();
        for dep in [
            "oxbloom-boot",
            "oxbloom-config",
            "oxbloom-di",
            "oxbloom-web",
        ] {
            let path = table["dependencies"][dep]["path"].as_str().unwrap();
            assert_eq!(path, format!("/x/{{name}}x/{{tokio}}/crates/{dep}"));
        }
    }

    #[test]
    fn dependency_placeholders_in_oxbloom_path_are_not_substituted() {
        let f = render(
            &Manifest::new("inj", "rest"),
            "/x/{dependencies}{crate_name}",
        );
        let table: toml::Table = toml::from_str(get(&f, "Cargo.toml")).unwrap();
        let path = table["dependencies"]["oxbloom-boot"]["path"]
            .as_str()
            .unwrap();
        assert_eq!(path, "/x/{dependencies}{crate_name}/crates/oxbloom-boot");
    }

    #[test]
    fn generated_project_is_its_own_workspace() {
        let f = files();
        let table: toml::Table = toml::from_str(get(&f, "Cargo.toml")).unwrap();
        assert!(
            table.contains_key("workspace"),
            "needs [workspace] to build inside another workspace"
        );
    }
}
