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
