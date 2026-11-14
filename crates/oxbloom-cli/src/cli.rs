use std::fmt;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::components;
use crate::generate::{GenerateError, write_project};
use crate::manifest::Manifest;
use crate::{naming, templates};

#[derive(Parser)]
#[command(name = "oxbloom", version, about = "Oxbloom project tool")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a new project.
    New(NewArgs),
    /// List components and their status.
    List,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Web {
    Rest,
}

impl Web {
    fn as_str(self) -> &'static str {
        match self {
            Self::Rest => "rest",
        }
    }
}

#[derive(Args)]
pub struct NewArgs {
    /// Project name: lowercase letters, digits, '-' or '_'.
    name: String,
    #[arg(long, value_enum, default_value_t = Web::Rest)]
    web: Web,
    /// Oxbloom workspace root used for path dependencies.
    #[arg(long)]
    oxbloom_path: Option<PathBuf>,
    /// Parent directory for the project (default: current directory).
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Not implemented yet.
    #[arg(long, value_name = "KIND")]
    security: Option<String>,
    /// Not implemented yet.
    #[arg(long, value_name = "KIND")]
    database: Option<String>,
    /// Not implemented yet.
    #[arg(long, value_name = "KIND")]
    frontend: Option<String>,
    /// Not implemented yet.
    #[arg(long)]
    jobs: bool,
    /// Not implemented yet.
    #[arg(long)]
    tests: bool,
    /// Not implemented yet.
    #[arg(long)]
    ai: bool,
}

/// A failed command; the variant decides the exit code.
#[derive(Debug)]
pub enum CliError {
    /// Bad input from the user (exit 2).
    Usage(String),
    /// Filesystem failure (exit 1).
    Io(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Io(_) => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(m) | Self::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for CliError {}

fn usage(e: impl fmt::Display) -> CliError {
    CliError::Usage(format!("{e:#}"))
}

/// Runs a command and returns the text to print on success.
pub fn run(cli: Cli) -> Result<String, CliError> {
    match cli.command {
        Command::List => Ok(components::list_text()),
        Command::New(args) => new_project(args),
    }
}

fn new_project(args: NewArgs) -> Result<String, CliError> {
    naming::validate_project_name(&args.name).map_err(usage)?;

    let requested = [
        ("security", args.security.is_some()),
        ("database", args.database.is_some()),
        ("jobs", args.jobs),
        ("ai", args.ai),
        ("tests", args.tests),
        ("frontend", args.frontend.is_some()),
    ];
    for (name, set) in requested {
        if set {
            components::require_available(name).map_err(usage)?;
        }
    }

    // Cargo resolves a relative path against the project dir, not the cwd, so make it absolute.
    let oxbloom = match args.oxbloom_path {
        Some(p) => std::path::absolute(&p)
            .map_err(|e| CliError::Usage(format!("invalid --oxbloom-path {}: {e}", p.display())))?,
        None => default_oxbloom_path(),
    };
    // The generated project depends on these four crates by path.
    for krate in [
        "oxbloom-web",
        "oxbloom-boot",
        "oxbloom-config",
        "oxbloom-di",
    ] {
        let manifest = oxbloom.join("crates").join(krate).join("Cargo.toml");
        if !manifest.is_file() {
            return Err(CliError::Usage(format!(
                "not an oxbloom workspace: missing {}",
                manifest.display()
            )));
        }
    }
    let oxbloom = oxbloom
        .to_str()
        .ok_or_else(|| CliError::Usage("--oxbloom-path must be valid UTF-8".into()))?;

    let manifest = Manifest::new(&args.name, args.web.as_str());
    let files = templates::render(&manifest, oxbloom);
    let parent = args.dir.unwrap_or_else(|| Path::new(".").to_path_buf());
    match write_project(&parent, &args.name, &files) {
        Ok(path) => Ok(format!("created {}", path.display())),
        Err(GenerateError::Conflict(m)) => Err(CliError::Usage(m)),
        Err(GenerateError::Io(e)) => Err(CliError::Io(format!("{e:#}"))),
    }
}

/// The workspace this CLI was built from (two levels above its manifest dir).
fn default_oxbloom_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("cli crate sits at <workspace>/crates/oxbloom-cli")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn run_args(args: &[&str]) -> Result<String, CliError> {
        let mut v = vec!["oxbloom"];
        v.extend_from_slice(args);
        run(Cli::try_parse_from(v).unwrap())
    }

    fn new_in(tmp: &std::path::Path, extra: &[&str]) -> Result<String, CliError> {
        let dir = tmp.to_str().unwrap();
        let mut a = vec!["new", "demo", "--dir", dir];
        a.extend_from_slice(extra);
        run_args(&a)
    }

    #[test]
    fn unimplemented_flags_fail_and_write_nothing() {
        for (flag, feature) in [
            (&["--security", "jwt"][..], "security"),
            (&["--database", "postgres"], "database"),
            (&["--jobs"], "jobs"),
            (&["--ai"], "ai"),
            (&["--tests"], "tests"),
            (&["--frontend", "react"], "frontend"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let err = new_in(tmp.path(), flag).unwrap_err();
            assert_eq!(err.exit_code(), 2, "{feature}");
            assert!(
                err.to_string()
                    .contains(&format!("{feature} is not implemented yet")),
                "{err}"
            );
            assert!(!tmp.path().join("demo").exists(), "{feature}");
        }
    }

    #[test]
    fn invalid_name_is_usage_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err =
            run_args(&["new", "Bad Name", "--dir", tmp.path().to_str().unwrap()]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn bad_oxbloom_path_is_usage_error() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        let err = new_in(
            tmp.path(),
            &["--oxbloom-path", empty.path().to_str().unwrap()],
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("oxbloom-web/Cargo.toml"), "{err}");
        assert!(!tmp.path().join("demo").exists());
    }

    #[test]
    fn workspace_without_boot_is_usage_error() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(fake.path().join("crates/oxbloom-web")).unwrap();
        std::fs::write(fake.path().join("crates/oxbloom-web/Cargo.toml"), "").unwrap();
        let err = new_in(
            tmp.path(),
            &["--oxbloom-path", fake.path().to_str().unwrap()],
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(
            err.to_string()
                .starts_with("not an oxbloom workspace: missing "),
            "{err}"
        );
        assert!(err.to_string().contains("oxbloom-boot/Cargo.toml"), "{err}");
        assert!(!tmp.path().join("demo").exists());

        // Every crate the template depends on is required.
        for (present, missing) in [
            ("oxbloom-boot", "oxbloom-config/Cargo.toml"),
            ("oxbloom-config", "oxbloom-di/Cargo.toml"),
        ] {
            std::fs::create_dir_all(fake.path().join("crates").join(present)).unwrap();
            std::fs::write(
                fake.path().join("crates").join(present).join("Cargo.toml"),
                "",
            )
            .unwrap();
            let err = new_in(
                tmp.path(),
                &["--oxbloom-path", fake.path().to_str().unwrap()],
            )
            .unwrap_err();
            assert_eq!(err.exit_code(), 2);
            assert!(err.to_string().contains(missing), "{err}");
        }
    }

    #[test]
    fn creates_project() {
        let tmp = tempfile::tempdir().unwrap();
        let out = new_in(tmp.path(), &[]).unwrap();
        assert!(out.starts_with("created "), "{out}");
        for f in [
            ".gitignore",
            "Cargo.toml",
            "application.toml",
            "oxbloom.toml",
            "src/lib.rs",
            "src/main.rs",
            "tests/hello.rs",
        ] {
            assert!(tmp.path().join("demo").join(f).is_file(), "{f}");
        }
    }

    #[test]
    fn occupied_target_is_usage_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("demo")).unwrap();
        std::fs::write(tmp.path().join("demo/keep"), "x").unwrap();
        let err = new_in(tmp.path(), &[]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn list_returns_component_table() {
        assert_eq!(run_args(&["list"]).unwrap(), crate::components::list_text());
    }

    #[test]
    fn relative_oxbloom_path_is_written_absolute() {
        // cargo runs unit tests with cwd = the crate dir, so ../.. is the workspace root.
        let tmp = tempfile::tempdir().unwrap();
        new_in(tmp.path(), &["--oxbloom-path", "../.."]).unwrap();
        let cargo = std::fs::read_to_string(tmp.path().join("demo/Cargo.toml")).unwrap();
        let table: toml::Table = toml::from_str(&cargo).unwrap();
        for dep in [
            "oxbloom-boot",
            "oxbloom-config",
            "oxbloom-di",
            "oxbloom-web",
        ] {
            let path = table["dependencies"][dep]["path"].as_str().unwrap();
            assert!(std::path::Path::new(path).is_absolute(), "{path}");
            assert!(
                std::path::Path::new(path).join("Cargo.toml").is_file(),
                "{path}"
            );
        }
    }
}
