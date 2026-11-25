//! Checks that the driver features of `oxbloom-data` reach the re-exported `sqlx`.

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlx_sqlite_pool_works() {
    use oxbloom_data::sqlx::Row;

    let pool = oxbloom_data::sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .unwrap();
    let row = oxbloom_data::sqlx::query("SELECT 1 AS one")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("one"), 1);
}

/// Builds the crate with the given cargo features and fails with the compiler output when the
/// build does not succeed.
fn build_with_features(features: &str) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = std::process::Command::new(cargo)
        .args(["build", "-p", "oxbloom-data", "--features", features])
        .env("CARGO_INCREMENTAL", "0")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo can be started");
    assert!(
        output.status.success(),
        "building oxbloom-data with the features `{features}` failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_migrate_feature_builds_alone_and_with_each_driver() {
    for features in [
        "migrate",
        "migrate,postgres",
        "migrate,mysql",
        "migrate,sqlite",
        "migrate,postgres,mysql,sqlite",
    ] {
        build_with_features(features);
    }
}
