use std::fs;

use oxbloom_config::{Config, ConfigError, Secret};
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
struct Server {
    port: u16,
    host: String,
}

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn parse_error_has_path_and_line_not_value() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("application.toml"),
        "ok = 1\n\npassword = hunter2\n",
    )
    .unwrap();
    let e = Config::builder()
        .dir(dir.path())
        .env_from(env(&[]))
        .build()
        .unwrap_err();
    let ConfigError::Parse { path, line, .. } = &e else {
        panic!("{e:?}")
    };
    assert!(path.ends_with("application.toml"));
    assert_eq!(*line, Some(3));
    let text = format!("{e} {e:?}");
    assert!(!text.contains("hunter2"), "{text}");
}

#[test]
fn io_error_when_base_path_is_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("application.toml")).unwrap();
    let e = Config::builder()
        .dir(dir.path())
        .env_from(env(&[]))
        .build()
        .unwrap_err();
    assert!(matches!(e, ConfigError::Io { .. }), "{e:?}");
}

#[test]
fn profile_overlay_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("application.toml"),
        "[server]\nport = 8080\nhost = \"0.0.0.0\"",
    )
    .unwrap();
    fs::write(
        dir.path().join("application-dev.toml"),
        "[server]\nhost = \"127.0.0.1\"",
    )
    .unwrap();
    let c = Config::builder()
        .dir(dir.path())
        .profile("dev")
        .env_from(env(&[]))
        .build()
        .unwrap();
    let s: Server = c.get("server").unwrap();
    assert_eq!(
        s,
        Server {
            port: 8080,
            host: "127.0.0.1".into()
        }
    );
}

#[test]
fn env_beats_files_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("application.toml"),
        "[server]\nport = 8080\nhost = \"h\"",
    )
    .unwrap();
    let c = Config::builder()
        .dir(dir.path())
        .env_from(env(&[("APP_SERVER__PORT", "9000"), ("OTHER", "x")]))
        .build()
        .unwrap();
    assert_eq!(c.get::<Server>("server").unwrap().port, 9000);
}

#[test]
fn custom_env_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let c = Config::builder()
        .dir(dir.path())
        .env_prefix("MYAPP")
        .env_from(env(&[
            ("MYAPP_SERVER__PORT", "1"),
            ("MYAPP_SERVER__HOST", "h"),
            ("APP_SERVER__PORT", "2"),
        ]))
        .build()
        .unwrap();
    assert_eq!(
        c.get::<Server>("server").unwrap(),
        Server {
            port: 1,
            host: "h".into()
        }
    );
}

#[test]
fn secret_value_not_in_any_error_output() {
    #[derive(Debug, Deserialize)]
    struct Creds {
        #[allow(dead_code)]
        token: Secret<u16>,
    }
    let dir = tempfile::tempdir().unwrap();
    let c = Config::builder()
        .dir(dir.path())
        .env_from(env(&[("APP_TOKEN", "s3cr3t-value")]))
        .build()
        .unwrap();
    let e = c.get::<Creds>("").unwrap_err();
    let text = format!("{e} {e:?}");
    assert!(!text.contains("s3cr3t"), "{text}");
}

#[test]
fn loading_50k_keys_and_a_50k_element_array_is_fast() {
    const N: usize = 50_000;
    let mut text = String::with_capacity(N * 40);
    for i in 0..N {
        text.push_str(&format!("key{i} = {i}\n"));
    }
    text.push_str("list = [\n");
    for i in 0..N {
        text.push_str(&format!("  {i},\n"));
    }
    text.push_str("]\n");
    let start = std::time::Instant::now();
    let c = Config::from_str(&text).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(
        c.get::<i64>(&format!("key{}", N - 1)).unwrap(),
        (N - 1) as i64
    );
    assert_eq!(c.get::<Vec<i64>>("list").unwrap().len(), N);
    assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
}

// ---- file size and file kind ----------------------------------------------------------

const MIB: usize = 1024 * 1024;

/// `a = 1` followed by a comment, `len` bytes in total.
fn padded(len: usize) -> String {
    let mut text = String::from("a = 1\n#");
    text.push_str(&"x".repeat(len - text.len()));
    text
}

fn build_dir(dir: &std::path::Path) -> Result<Config, ConfigError> {
    Config::builder().dir(dir).env_from(env(&[])).build()
}

#[test]
fn file_over_cap_is_file_too_large() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("application.toml"), padded(MIB + 1)).unwrap();
    let e = build_dir(dir.path()).unwrap_err();
    assert!(
        matches!(&e, ConfigError::FileTooLarge { path } if path.ends_with("application.toml")),
        "{e:?}"
    );
    assert_eq!(
        e.to_string(),
        "configuration file is larger than the size limit"
    );
}

#[test]
fn file_exactly_at_cap_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("application.toml"), padded(MIB)).unwrap();
    let c = build_dir(dir.path()).unwrap();
    assert_eq!(c.get::<u8>("a").unwrap(), 1);
}

fn huge_cap_loads_base_profile_and_import(cap: usize) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("application.toml"),
        "a = 1\noxbloom.config.import = [\"extra.toml\"]\n",
    )
    .unwrap();
    fs::write(dir.path().join("application-dev.toml"), "p = 2\n").unwrap();
    fs::write(dir.path().join("extra.toml"), "i = 3\n").unwrap();
    let c = Config::builder()
        .dir(dir.path())
        .max_file_size(cap)
        .profile("dev")
        .env_from(env(&[]))
        .build()
        .unwrap();
    assert_eq!(c.get::<u8>("a").unwrap(), 1);
    assert_eq!(c.get::<u8>("p").unwrap(), 2);
    assert_eq!(c.get::<u8>("i").unwrap(), 3);
}

#[test]
fn max_file_size_usize_max_loads_files_normally() {
    huge_cap_loads_base_profile_and_import(usize::MAX);
}

#[test]
fn max_file_size_usize_max_minus_one_loads_files_normally() {
    huge_cap_loads_base_profile_and_import(usize::MAX - 1);
}

#[test]
fn custom_cap_applies_to_base_profile_and_imports() {
    let build = |dir: &std::path::Path, cap: usize, profile: Option<&str>| {
        let mut b = Config::builder()
            .dir(dir)
            .max_file_size(cap)
            .env_from(env(&[]));
        if let Some(p) = profile {
            b = b.profile(p);
        }
        b.build()
    };
    let too_large = |r: Result<Config, ConfigError>, file: &str| match r {
        Err(ConfigError::FileTooLarge { path }) => assert!(path.ends_with(file), "{path:?}"),
        other => panic!("{file}: {other:?}"),
    };
    // Base file.
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("application.toml"), padded(65)).unwrap();
    too_large(build(dir.path(), 64, None), "application.toml");
    assert!(build(dir.path(), 65, None).is_ok());
    // Profile file.
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("application-p.toml"), padded(65)).unwrap();
    too_large(build(dir.path(), 64, Some("p")), "application-p.toml");
    assert!(build(dir.path(), 65, Some("p")).is_ok());
    // Import.
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("application.toml"),
        "oxbloom.config.import = [\"x.toml\"]\n",
    )
    .unwrap();
    fs::write(dir.path().join("x.toml"), padded(65)).unwrap();
    too_large(build(dir.path(), 64, None), "x.toml");
    assert!(build(dir.path(), 65, None).is_ok());
    // 0: only empty files pass.
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("application.toml"), "").unwrap();
    assert!(build(dir.path(), 0, None).is_ok());
    fs::write(dir.path().join("application.toml"), "\n").unwrap();
    too_large(build(dir.path(), 0, None), "application.toml");
}

#[test]
fn file_too_large_error_has_no_content() {
    let dir = tempfile::tempdir().unwrap();
    let text = format!("secret = \"hunter2\"\n#{}", "x".repeat(MIB));
    fs::write(dir.path().join("application.toml"), text).unwrap();
    let e = build_dir(dir.path()).unwrap_err();
    assert!(matches!(e, ConfigError::FileTooLarge { .. }), "{e:?}");
    let text = format!("{e} {e:?}");
    assert!(
        !text.contains("hunter2") && !text.contains("xxxx"),
        "{text}"
    );
}

/// Builds `dir` on another thread; panics if that takes longer than 5 s.
#[cfg(unix)]
fn build_within_5s(dir: std::path::PathBuf) -> Result<Config, ConfigError> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(build_dir(&dir));
    });
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("build did not finish within 5 s")
}

#[cfg(unix)]
fn is_not_regular_file_error(e: &ConfigError, file: &str) -> bool {
    matches!(e, ConfigError::Io { path, source }
        if path.ends_with(file)
            && source.kind() == std::io::ErrorKind::InvalidInput
            && source.to_string() == "not a regular file")
}

#[cfg(unix)]
#[test]
fn fifo_as_base_file_is_io_error_not_a_hang() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("application.toml");
    match std::process::Command::new("mkfifo").arg(&fifo).status() {
        Ok(status) if status.success() => {}
        _ => {
            eprintln!("mkfifo unavailable: skipped");
            return;
        }
    }
    let e = build_within_5s(dir.path().to_path_buf()).unwrap_err();
    assert!(is_not_regular_file_error(&e, "application.toml"), "{e:?}");
}

#[cfg(unix)]
#[test]
fn symlink_to_dev_zero_is_rejected_quickly() {
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/dev/zero", dir.path().join("application.toml")).unwrap();
    let start = std::time::Instant::now();
    let e = build_within_5s(dir.path().to_path_buf()).unwrap_err();
    assert!(is_not_regular_file_error(&e, "application.toml"), "{e:?}");
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    // A profile file too.
    fs::remove_file(dir.path().join("application.toml")).unwrap();
    std::os::unix::fs::symlink("/dev/zero", dir.path().join("application-p.toml")).unwrap();
    let e = Config::builder()
        .dir(dir.path())
        .profile("p")
        .env_from(env(&[]))
        .build()
        .unwrap_err();
    assert!(is_not_regular_file_error(&e, "application-p.toml"), "{e:?}");
}

// ---- active_profiles ---------------------------------------------------------------------

/// Writes an empty `application-{p}.toml` for each name.
fn profile_files(dir: &std::path::Path, names: &[&str]) {
    for p in names {
        fs::write(dir.join(format!("application-{p}.toml")), "").unwrap();
    }
}

#[test]
fn active_profiles_lists_expanded_profiles_in_order() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("application.toml"),
        "[oxbloom.profiles.group]\nprod = [\"db\", \"metrics\"]\ndb = [\"pool\", \"metrics\"]\n",
    )
    .unwrap();
    profile_files(dir.path(), &["prod", "db", "pool", "metrics", "extra"]);
    let config = Config::builder()
        .dir(dir.path())
        .profile("extra")
        .profile("prod")
        .profile("db")
        .env_from(env(&[]))
        .build()
        .unwrap();
    // Group members follow their group; each name once, first occurrence kept.
    assert_eq!(
        config.active_profiles(),
        ["extra", "prod", "db", "pool", "metrics"]
    );
}

#[test]
fn active_profiles_empty_without_profiles() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("application.toml"), "name = \"demo\"\n").unwrap();
    let config = Config::builder()
        .dir(dir.path())
        .env_from(env(&[]))
        .build()
        .unwrap();
    assert!(config.active_profiles().is_empty());
    assert!(
        Config::from_str("name = \"demo\"")
            .unwrap()
            .active_profiles()
            .is_empty()
    );
}

#[test]
fn active_profiles_from_builder_cli_env_and_base_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("application.toml"),
        "oxbloom.profiles.active = \"base\"\n",
    )
    .unwrap();
    profile_files(dir.path(), &["base", "env", "cli1", "cli2", "builder"]);
    let build = |profile: Option<&str>, vars: &[(&str, &str)], args: &[&str]| {
        let mut b = Config::builder()
            .dir(dir.path())
            .env_from(env(vars))
            .args(args.iter().map(|a| a.to_string()));
        if let Some(p) = profile {
            b = b.profile(p);
        }
        b.build().unwrap().active_profiles().to_vec()
    };
    let env_var = [("OXBLOOM_PROFILE", "env")];
    let cli = ["--oxbloom.profiles.active=cli1,cli2"];
    assert_eq!(build(None, &[], &[]), ["base"]);
    assert_eq!(build(None, &env_var, &[]), ["env"]);
    assert_eq!(build(None, &env_var, &cli), ["cli1", "cli2"]);
    assert_eq!(build(Some("builder"), &env_var, &cli), ["builder"]);
}
