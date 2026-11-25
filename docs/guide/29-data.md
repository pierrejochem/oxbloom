# Data

Oxbloom connects to a SQL database through the crate `oxbloom-data`, which is built on `sqlx`. It reads the `data.*` keys, builds one connection pool per configured source, runs work in a transaction, reports the health of each pool and connects and closes the pools with the application. You write queries with `sqlx` directly: there is no query builder, no repository layer and no migration runner yet. This is module M3, part a (M3a). The supported databases are PostgreSQL, MySQL (and MariaDB) and SQLite.

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["data-postgres"] }   # or data-mysql, data-sqlite
oxbloom-boot = { version = "0.1", features = ["data-postgres", "ssl"] }   # a source can use an SSL bundle
oxbloom-boot = { version = "0.1", features = ["data-postgres", "actuator"] }   # health indicators `db`
oxbloom-test = { version = "0.1", features = ["data"] }            # TestDatabase (dev-dependency)
```

The decisions are in [ADR 0096](../adr/0096-data-core.md) and every public item has an example in the [API reference](../API.md#416-oxbloom-data).

The features of `oxbloom-boot` are `data-postgres`, `data-mysql` and `data-sqlite`; name the drivers your application uses. Each one turns on the matching driver of `oxbloom-data`, rustls with `aws-lc-rs` and the platform root certificates (never `ring`) and `transaction_with_retry`. Without the `data-*` features of boot, use `oxbloom-data` directly with its own features `postgres`, `mysql`, `sqlite` and `retry`. With no driver feature, `oxbloom-data` builds without any TLS crate, and a source whose driver is missing fails with `Unsupported`. Every driver feature, `sqlite` included, carries the rustls and `aws-lc-rs` stack. The `sqlite` feature also compiles SQLite itself from source (`sqlx` bundles it), so it needs a C compiler and adds build time.

## Configuration

The unnamed keys under `data` form the default source. Further sources live under `data.sources.<name>` and take the same keys.

```toml
[data]
url             = "postgres://app:${DB_PASSWORD}@db.internal:5432/app"   # Secret
max-connections = 10
acquire-timeout = "30s"

[data.sources.reporting]
url             = "mysql://report:${REPORT_PASSWORD}@reports.internal/reports"
max-connections = 4
```

| Key | Default | Meaning |
|---|---|---|
| `data.connect-on-startup` | `true` | Whether the pools connect when the application starts. Global. |
| `data.url` | unset | The connection URL, a `Secret`. The scheme picks the driver: `postgres` or `postgresql`, `mysql` or `mariadb`, `sqlite`. |
| `data.max-connections` | `10` (1 for an in-memory SQLite URL) | The most connections of the pool. |
| `data.min-connections` | `0` | The connections the pool keeps open. Not above `max-connections`. A lazy pool honours it only loosely. |
| `data.acquire-timeout` | `30s` | How long acquiring a connection may wait. Above zero. |
| `data.idle-timeout` | `sqlx` default (10 minutes); never for an in-memory SQLite URL | How long a connection may sit idle. Above zero. |
| `data.max-lifetime` | `sqlx` default (30 minutes); never for an in-memory SQLite URL | How long a connection may live. Above zero. |
| `data.test-before-acquire` | `true` | Whether a connection is checked before it is handed out. |
| `data.ssl-mode` | the driver | `disable`, `prefer`, `require`, `verify-ca` or `verify-full`. Not for SQLite. |
| `data.ssl-bundle` | unset | The name of an SSL bundle. See [TLS with an SSL bundle](#tls-with-an-ssl-bundle). Not for SQLite. |
| `data.sources.<name>.*` | as above | The same keys for a named source. |

Rules for every source:

- A source needs a `url`. A name is `[a-z0-9-]+` and is not `default`. Names with a dash are best set in the configuration file, because an environment variable override such as `..._DATA__SOURCES__REPORTING_DB__URL` spells the name `reporting_db`, which is refused. Schemes are matched in lower case, and the network schemes need `://`.
- An unknown key, a value of the wrong type, a bad duration, a zero `max-connections`, `min-connections` above `max-connections`, an unknown `ssl-mode`, an empty `ssl-bundle`, and `ssl-mode` or `ssl-bundle` on a SQLite source are errors. Each error names the full key and never the value.
- The URL is a `Secret`. No error, `Debug` output, log event or health detail contains the URL, the password, the user, the host or a SQLite path.
- A password with special characters must be percent-encoded in the URL.
- The property metadata lists every key, and the conditions report says why `DataAutoConfiguration` ran or not. It runs when the configuration has a `data` section.

## Injecting the pool

With a `data` section, the default source also becomes a bean of its pool type: `sqlx::PgPool`, `sqlx::MySqlPool` or `sqlx::SqlitePool`, injected as `Arc<..>`. The bean backs off when your application registers its own bean of that type. Named sources are not beans: take the `DataSources` bean and ask for a pool by name.

Register the component like any other, for example with `.configure(|b| b.component::<Orders>())` on the application builder (see [Dependency injection](03-dependency-injection.md)).

```rust
use std::sync::Arc;
use oxbloom_data::DataSources;
use oxbloom_data::sqlx::{self, PgPool};
use oxbloom_di::component;

#[component]
struct Orders {
    pool: Arc<PgPool>,
    sources: Arc<DataSources>,
}

impl Orders {
    async fn count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM orders").fetch_one(&*self.pool).await
    }

    async fn report_rows(&self) -> Result<i64, sqlx::Error> {
        let reporting = self.sources.postgres("reporting").expect("the reporting source is Postgres");
        sqlx::query_scalar("SELECT COUNT(*) FROM orders").fetch_one(reporting).await
    }
}
```

`DataSources` has `postgres(name)`, `mysql(name)` and `sqlite(name)` (each exists when the driver feature is on, and returns `None` for an unknown name or another driver), and also `names`, `default_name`, `driver_of`, `len`, `is_empty`, `ping`, `ping_all`, `pool_counts`, `close_all` and `close_all_within`. `Driver::from_url` tells the driver of a URL by the same scheme rule as the settings.

Pools are lazy. Building them opens no connection and needs no database; the first use connects. A lazy pool needs a Tokio runtime, because `sqlx` ties the maintenance task of a pool to the runtime that is current when it is built. Bean creation runs on the application's runtime, so this holds in an application. `DataSources::build` called outside a runtime fails with `DataError::Pool`, and a pool you build on a temporary runtime stops working when that runtime ends.

Never log `connect_options()` of a pool. The `Debug` output of `sqlx`'s own connect options contains the password. The `Debug` output of `DataSources`, `DataSettings` and `SourceSettings` is safe: it shows source names and drivers only.

## Transactions

`transaction(&pool, &TransactionOptions, async |tx| ..)` runs an async closure in a transaction on a connection of the pool. It commits when the closure returns `Ok` and rolls back when it returns `Err`, when it panics and when the future is dropped before it ends. The error type of the closure is your own and comes back as `TransactionError::Inner`.

```rust
use oxbloom_data::sqlx::{self, sqlite::SqlitePoolOptions};
use oxbloom_data::{TransactionError, TransactionOptions, transaction};

#[derive(Debug)]
enum TransferError {
    Database(sqlx::Error),
    InsufficientFunds,
}

impl From<sqlx::Error> for TransferError {
    fn from(error: sqlx::Error) -> Self {
        TransferError::Database(error)
    }
}

let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
sqlx::query("CREATE TABLE accounts (name TEXT PRIMARY KEY, balance INTEGER)")
    .execute(&pool)
    .await?;

let options = TransactionOptions::default();
let result = transaction(&pool, &options, async |tx| {
    sqlx::query("UPDATE accounts SET balance = balance - 500 WHERE name = 'alice'")
        .execute(&mut **tx)
        .await?;
    let balance: i64 = sqlx::query_scalar("SELECT balance FROM accounts WHERE name = 'alice'")
        .fetch_one(&mut **tx)
        .await?;
    if balance < 0 {
        return Err(TransferError::InsufficientFunds);
    }
    Ok(())
})
.await;
assert!(matches!(result, Err(TransactionError::Inner(TransferError::InsufficientFunds))));
```

`TransactionError<E>` is `Begin` (no connection, or the begin statement failed), `Commit`, `Unsupported` (the driver cannot honor the options; nothing ran) and `Inner(E)`. `Commit` means the database refused the commit, and then the transaction is rolled back, or the connection was lost during the `COMMIT`, and then the outcome is unknown: the transaction may or may not have been committed. The `Display` text of the first three is fixed and the `Debug` output names only the variant, so no SQL, row data or driver text reaches a log. `into_inner` returns an `Option<E>`: the error of the closure for `Inner`, `None` for the other variants. `source` returns that error when it is a standard error. Because `Debug` of the `Inner` variant prints `Inner(..)`, log your own error where you have it.

There are no ambient transactions. A function does not join a transaction by itself; pass the `&mut Transaction` along, or run its statements inside the closure.

### Savepoints

A nested unit of work inside the closure uses `tx.begin()` (from `sqlx::Acquire`), which creates a savepoint. Rolling the savepoint back undoes only what ran after it. This example continues the previous one: `pool` is the pool from there.

```rust
use oxbloom_data::sqlx::{self, Acquire};
use oxbloom_data::{TransactionOptions, transaction};

transaction(&pool, &TransactionOptions::default(), async |tx| {
    sqlx::query("INSERT INTO log VALUES ('kept')").execute(&mut **tx).await?;
    let mut savepoint = tx.begin().await?;
    sqlx::query("INSERT INTO log VALUES ('undone')").execute(&mut *savepoint).await?;
    savepoint.rollback().await?;
    Ok::<_, sqlx::Error>(())
})
.await?;
```

### Isolation and read-only

`TransactionOptions::default().isolation(Isolation::Serializable).read_only(true)` sets the level and the mode. `Isolation` has `ReadUncommitted`, `ReadCommitted`, `RepeatableRead` and `Serializable`. The options go through each driver's begin statement:

| Driver | Begin statement | Accepted options |
|---|---|---|
| PostgreSQL | `BEGIN ISOLATION LEVEL .. READ ONLY` | every level, read-only |
| MySQL, MariaDB | `SET TRANSACTION ISOLATION LEVEL ..` and `START TRANSACTION READ ONLY` in one statement | every level, read-only |
| SQLite | `BEGIN` | the default and `Serializable` only |

An option the driver cannot honor is refused with `TransactionError::Unsupported` before a connection is acquired; it is never ignored. SQLite transactions are always serializable and cannot be read-only, so `read_only(true)` and every level except `Serializable` are refused.

### Transactions in spawned tasks and handlers

When the closure borrows from the surrounding code, the future of `transaction` is not `Send`, because of a current limitation of the compiler with async closures (rust-lang/rust#110338). Where the future must be `Send`, as in `tokio::spawn`, write the closure as `async move` over owned data and clone the pool in first. This example also continues the transaction example above, with its `pool`:

```rust
use oxbloom_data::sqlx;
use oxbloom_data::{TransactionOptions, transaction};

let task_pool = pool.clone();
let name = String::from("alice");
tokio::spawn(async move {
    transaction(&task_pool, &TransactionOptions::default(), async move |tx| {
        sqlx::query("INSERT INTO users VALUES (?)").bind(&name).execute(&mut **tx).await?;
        Ok::<_, sqlx::Error>(())
    })
    .await
})
.await??;
```

Outside such contexts, borrowing is fine.

### Retrying after a serialization failure

With the cargo feature `retry` (on in the `data-*` features of boot), `transaction_with_retry(&pool, &options, &policy, work)` runs the closure again, in a new transaction, after a serialization failure or a deadlock. The policy is an `oxbloom_tasks::RetryPolicy`: `max_attempts` runs at most (zero acts as one), with the delay of its backoff and jitter. The feature is additive: `transaction` keeps one signature whether or not it is on.

The error type of the closure must implement `RetryableError`. It is implemented for `sqlx::Error`, for a boxed error that holds a `sqlx::Error` anywhere in its source chain, and for `TransactionError<E>` when `E` is retryable. An application error that wraps `sqlx::Error` delegates to it:

```rust
use oxbloom_data::RetryableError;
use oxbloom_data::sqlx;

enum OrderError {
    Database(sqlx::Error),
    OutOfStock,
}

impl RetryableError for OrderError {
    fn is_retryable(&self) -> bool {
        match self {
            OrderError::Database(e) => e.is_retryable(),
            OrderError::OutOfStock => false,
        }
    }
}
```

The failures that count are PostgreSQL SQLSTATE `40001` and `40P01`, MySQL errors `1213` and `1205`, and SQLite `SQLITE_BUSY` and `SQLITE_LOCKED`, whether the closure returns them or the commit fails with them. A failed begin and every other error, such as a constraint violation, are returned at once. When the attempts run out, the last error is returned. The retry loop is its own and logs nothing, so no driver error text reaches the logs.

The closure must be idempotent. Everything it did in the database is rolled back, but effects outside the database, such as a sent message, are not. SQLite reports `SQLITE_LOCKED` also for a conflict within one connection, such as dropping a table that an open statement still reads. Such an error fails the same way on every run, so it is retried until the attempts run out and then returned.

## Startup, shutdown and errors

`DataService` is a boot service with the lowest phase, so it starts before every other service and stops after them. It is registered only when the `data` section configures a source; a section with nothing but `connect-on-startup` registers none.

- At start, with `data.connect-on-startup = true` (the default), it pings the sources one at a time, in the order of `DataSources::names`, each with a 10 second timeout, and stops at the first one that fails, so several unreachable sources cost one wait, not one each. An unreachable database fails the start. The failure report names the source and the key `<prefix>.url` and never shows the URL. A refused connection can end as a pool timeout, because `sqlx` retries it until the `acquire-timeout`; the report treats both as unreachable. With the default `acquire-timeout` of 30 seconds, an unreachable network server makes the start wait the full 10 seconds of its check. Set a shorter `acquire-timeout` to fail sooner. With `connect-on-startup = false`, the pools connect on first use.
- At stop, it marks every pool closed at once, so no pool hands out a new connection, and then waits up to 10 seconds in all for the pools to close, which needs the connections in use to be returned (`DataSources::close_all_within`). When that time runs out, it logs one warning at target `oxbloom::data` with the fixed text "these data sources did not close within 10 seconds" and a `sources` field that lists only the sources whose close did not finish. Their connections in use are closed when they are returned. `Service::stop` returns nothing, so this warning is the only report of it.
- `DataService` does not keep running (`Service::keeps_running` is `false`): it only holds the pools between start and stop. An application whose runners do the work and that has no service that keeps running, such as a web server or a scheduler, therefore ends after its runners, and the pools are still closed on the way out. With a web server, the application waits for a shutdown signal as before.
- A failed start does not call `stop`; the pools are dropped with the container.
- When your application registers its own `DataSources` bean, it wins: boot builds no registry of its own, and the default pool bean, `DataService` and the health indicators use your bean.

Errors have fixed text. `DataError` is `Config`, `Connect`, `Pool`, `Unsupported`, `Tls` and `VerifyCaWithTrust`, none with a payload; the enum is `#[non_exhaustive]`, like `Driver`. `DataSources::build` returns a `SourceError` that names the source (`source_name()`), the `DataError` (`error()`) and the key to fix (`config_prefix()`, which is `data` or `data.sources.<name>`); its text is "data source `<name>`: <fixed text>". The startup failure report gives an action for each case: the missing `data-<driver>` feature for `Unsupported`, the TLS keys for `Tls`, the key `<prefix>.ssl-mode` and `verify-full` for `VerifyCaWithTrust`, a Tokio runtime for `Pool` outside one, and the URL options for the rest.

## Health

With the features `data-*` and `actuator` of `oxbloom-boot`, `ActuatorAutoConfiguration` adds one health indicator per source. The default source is named `db` and a named source is `db.<name>`. An indicator that is up has the details `driver`, `size` and `idle`. A down indicator has `error` and `driver`, with the fixed text `connection failed` or `timed out` (a pool error after the whole 3 second timeout of the check counts as `timed out`).

```toml
[management.health.readiness]
include = ["db", "db.reporting"]   # these indicators also decide readiness
```

The indicators are built by one `DataIndicators` bean and chained after the `dyn HealthIndicator` beans of the health runner, because the container cannot register a bean per source name. An application that collects the `dyn HealthIndicator` beans itself does not see them. Outside boot, `DataHealthIndicator::new(sources, "default")` (feature `data` of `oxbloom-actuator`) gives the same indicator; `with_timeout` changes the 3 seconds.

## TLS with an SSL bundle

A PostgreSQL or MySQL source can name an SSL bundle, with a trust store for a private CA, a client certificate, or both. Boot resolves the name against the `SslBundles` bean (feature `ssl`) and hands the PEM to the pool.

```toml
[ssl.bundle.pem.dbclient]
trust       = "config/db-ca.pem"
certificate = "config/db-client.crt"   # optional client certificate (mutual TLS)
private-key = "config/db-client.key"

[data]
url        = "postgres://app:${DB_PASSWORD}@db.internal/app"
ssl-bundle = "dbclient"
```

Rules:

- For a database, the bundle's trust certificates are **added** to the platform root certificates; they do not replace them. `sqlx` loads the platform roots and then adds the bundle's certificates. This is unlike the other users of an SSL bundle, such as the HTTP client, where the bundle's trust replaces the platform roots.
- A source with `ssl-bundle` and no `ssl-mode` connects with `verify-full` (MySQL `VerifyIdentity`): the certificate must chain to the bundle's trust or to a platform root, and the host name must match. Because the platform roots stay, `verify-full` also accepts a publicly trusted certificate for the database's host name. This overrides a mode in the URL and the `PGSSLMODE` variable.
- An explicit `ssl-mode` wins. `prefer` and `require` encrypt but do not check the certificate, and `prefer` falls back to plain text, also with a bundle. `verify-full` checks it.
- `ssl-mode = "verify-ca"` together with a bundle that has a trust store is refused when the pools are built: `verify-ca` does not check the host name, and with the platform roots added it would accept any publicly trusted certificate for any host. The startup failure report names `<prefix>.ssl-mode` and says to use `verify-full`. `verify-ca` with a bundle that holds only a client certificate stays allowed; it then checks the certificate against the platform roots.
- `ssl-bundle` with `ssl-mode = "disable"` is a settings error.
- A bundle that holds only a client certificate and no trust store, with no `ssl-mode`, also defaults to `verify-full`, against the platform roots. A server behind a private CA then needs the CA in the bundle's trust or an explicit `ssl-mode`.
- The `oxbloom-ssl` bundle wipes its copy of the private key DER when the material is dropped, but `sqlx` keeps its own copy of the client key PEM in the connect options of each pool, unwiped, for the lifetime of the pool.
- An unknown bundle name fails the start on `<prefix>.ssl-bundle` and lists the bundles that exist. Without the `ssl` feature of boot, naming a bundle is an error on the same key.
- The PEM is parsed on the first connection, so a bad PEM shows up as `DataError::Connect` then, not at build time.
- Pools keep the material they were built with. They do not follow a reload of the bundle (see [SSL bundles](16-ssl-bundles.md#ssl-bundles)): restart the application after a rotation.

`SslBundle::trust_pem()` and `SslBundle::identity_pem()` in `oxbloom-ssl` give the PEM. The identity key keeps its own encoding under the matching label: `PRIVATE KEY` (PKCS#8), `RSA PRIVATE KEY` (PKCS#1) or `EC PRIVATE KEY` (SEC1). `sqlx` with rustls reads all three; a native-tls build would need PKCS#8. The key comes back as a plain `Vec<u8>`: never log it.

## SQLite

A SQLite source takes `sqlite:` URLs. It has no `ssl-mode` and no `ssl-bundle`. An in-memory database (`sqlite::memory:`, or `mode=memory`) disappears when its last connection closes, so:

- the default `max-connections` is 1;
- an explicit `max-connections` above 1 is refused unless the URL has `cache=shared`;
- `idle-timeout` and `max-lifetime` default to none, so the pool never closes its connections;
- a value you set for `idle-timeout` or `max-lifetime` can still destroy a database that is not shared.

## Testing

`TestDatabase` in `oxbloom-test` (feature `data`) gives a throwaway database. `TestDatabase::sqlite()` is a shared-cache in-memory database with a unique name; one connection stays open for its lifetime, so every pool connection sees the same data. `config_snippet()` returns a `[data]` section for `ContextRunner`, and `sqlite_pool()` the pool. Call `close().await` at the end of the test.

```rust
use oxbloom_boot::DataAutoConfiguration;
use oxbloom_data::sqlx::{self, SqlitePool};
use oxbloom_test::{ContextRunner, TestDatabase};

#[tokio::test(flavor = "multi_thread")]
async fn the_context_gets_a_pool_bean() {
    let db = TestDatabase::sqlite();
    sqlx::query("CREATE TABLE t (id INTEGER)").execute(db.sqlite_pool()).await.unwrap();
    sqlx::query("INSERT INTO t (id) VALUES (7)").execute(db.sqlite_pool()).await.unwrap();
    ContextRunner::new()
        .config_str(db.config_snippet())
        .auto_configuration::<DataAutoConfiguration>()
        .run(|view| {
            let pool = view.get::<SqlitePool>().expect("the pool bean");
            let handle = tokio::runtime::Handle::current();
            let (id,): (i64,) = tokio::task::block_in_place(|| {
                handle.block_on(sqlx::query_as("SELECT id FROM t").fetch_one(&*pool))
            })
            .unwrap();
            assert_eq!(id, 7);
        })
        .await;
    db.close().await;
}
```

For PostgreSQL and MySQL, the features `data-postgres` and `data-mysql` of `oxbloom-test` add `TestDatabase::postgres()` and `TestDatabase::mysql()`, which are `async`. They create a database named `oxbloom_test_<suffix>` on the server whose URL is in `OXBLOOM_TEST_POSTGRES_URL` or `OXBLOOM_TEST_MYSQL_URL`, and drop it in `close`. The `Drop` cleanup is best effort and usually does not run on a current-thread `#[tokio::test]`, so a test that panics before `close` leaves its `oxbloom_test_*` database on a persistent server; drop such leftovers by hand.

### The conformance suite and `make test-db`

The conformance suite runs the same transaction, rollback, savepoint, retry, health, startup and secrecy checks against every driver. The tests are named `data_conformance_*` and live in `crates/oxbloom-test/tests/`, not in `oxbloom-data`: a dev-dependency of `oxbloom-test` on `oxbloom-data` would turn on `sqlite` and `retry` in every test build of `oxbloom-data`, and the builds without a driver would no longer be tested.

- The SQLite tests always run, with `cargo test`.
- The PostgreSQL and MySQL tests are `#[ignore]`d. They need the two environment variables above.
- `make test-db` starts both servers from `compose.test.yaml` (`postgres:17` on 127.0.0.1:55432 and `mysql:8.4` on 127.0.0.1:53306, both on tmpfs), exports the two URLs, runs `cargo test --workspace --all-features -- --ignored data_conformance` and removes the containers with `down -v`, also when the tests fail. It needs `docker compose`, `docker-compose` or `podman compose`.
- The MySQL isolation check reads `information_schema.innodb_trx`, which needs the `PROCESS` privilege. A restricted `OXBLOOM_TEST_MYSQL_URL` user fails that test.

## Limits

- **No migrations yet.** Schema migrations are M3b, the next part. Until then, run them with the `sqlx` tooling or your own code.
- **No ambient transactions.** A transaction is a closure with an explicit `&mut Transaction`; nothing joins one by itself.
- **No repository layer and no query API.** Use `sqlx` directly.
- **No qualifier mechanism.** Named sources are looked up by name on the `DataSources` bean, not injected by type.
- **Pools do not follow bundle reloads.** A rotated certificate needs a restart.
- **Three databases only.** PostgreSQL, MySQL (and MariaDB) and SQLite.
- **Lazy pools need the application's runtime.** `DataError::Pool` outside a Tokio runtime; a pool built on a temporary runtime stops working with it.
- **`min-connections` is only loosely honoured** by a lazy pool: `sqlx` opens and refills those connections in the background.
- **The future of `transaction` is not `Send` when the closure borrows.** Use `async move` over owned data inside `tokio::spawn` (rust-lang/rust#110338).
- **`transaction_with_retry` needs an idempotent closure.** Effects outside the database are not rolled back, and SQLite `LOCKED` from a conflict within one connection is retried until the attempts run out.
- **SQLite accepts only the default isolation and `Serializable`,** and no read-only transactions. In-memory SQLite has the rules above.
- **Health indicators are not `dyn HealthIndicator` beans.** An application that collects those beans itself does not see them.
- **No end-to-end TLS test.** The compose services of `make test-db` run without TLS, so the TLS settings are tested by the connect options they produce, not against a TLS server.
- **Bundle trust is added to the platform roots.** `verify-full` also accepts a publicly trusted certificate for the host name, and `verify-ca` with a bundle that has a trust store is refused.
- **A test that panics before `close` leaks a database** on a persistent PostgreSQL or MySQL server.
- **PostgreSQL and MySQL tests are env-gated.** They run only with the two URLs set, for example through `make test-db`.
