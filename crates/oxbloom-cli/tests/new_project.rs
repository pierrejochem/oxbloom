use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_oxbloom");

#[test]
fn list_prints_components() {
    let out = Command::new(BIN).arg("list").output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "web   available (rest)\nsecurity, database, jobs, ai, tests, frontend   planned\n"
    );
}

#[test]
fn new_bad_name_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(BIN)
        .args(["new", "Bad Name", "--dir"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("error: "));
}

#[test]
fn unimplemented_flag_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(BIN)
        .args(["new", "demo", "--ai", "--dir"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("ai is not implemented yet"));
}

/// Slow: compiles a generated project in the configured shared target directory.
/// Run with `-- --ignored`.
#[test]
#[ignore]
fn generated_project_checks_and_tests() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(BIN)
        .args(["new", "demo", "--dir"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let project = tmp.path().join("demo");
    for sub in ["check", "test"] {
        let res = Command::new("cargo")
            .arg(sub)
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(
            res.status.success(),
            "cargo {sub} failed:\n{}",
            String::from_utf8_lossy(&res.stderr)
        );
    }
}
