# Data

Oxbloom connects to a SQL database through the crate `oxbloom-data`, which is built on `sqlx`. It reads the `data.*` keys, builds one connection pool per configured source, runs work in a transaction, reports the health of each pool and connects and closes the pools with the application. You write queries with `sqlx` directly: there is no query builder and no repository layer. This is module M3, parts a and b: the pools, transactions, health and tests (M3a) and the [schema migrations](#migrations) (M3b). The supported databases are PostgreSQL, MySQL (and MariaDB) and SQLite.

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["data-postgres"] }   # or data-mysql, data-sqlite
oxbloom-boot = { version = "0.1", features = ["data-postgres", "ssl"] }   # a source can use an SSL bundle
oxbloom-boot = { version = "0.1", features = ["data-postgres", "actuator"] }   # health indicators `db`
oxbloom-boot = { version = "0.1", features = ["data-postgres", "data-migrate"] }   # run migrations at startup
oxbloom-test = { version = "0.1", features = ["data"] }            # TestDatabase (dev-dependency)
```

The decisions are in [ADR 0096](../adr/0096-data-core.md) and [ADR 0097](../adr/0097-data-migrations.md) (migrations), and every public item has an example in the [API reference](../API.md#416-oxbloom-data).

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

The indicator of a source that migrates also has the detail `migrations`: the highest version recorded as applied, as a number. It is absent when the history table does not exist yet, and it never shows a description or SQL. Outside boot, `with_migrations_table(table)` adds it.

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

## Migrations

Oxbloom applies the schema migrations of a source when the application starts. It uses the migration engine of `sqlx`, so the files and the history table are those of `sqlx-cli`, and you can move between the two without converting anything. Migrations need the cargo feature `data-migrate` of `oxbloom-boot` (or `migrate` of `oxbloom-data`). With `migrations.enabled = true` and without the feature, the start fails with a report that names the feature.

### Keys

The keys are `data.migrations.*` for the default source and `data.sources.<name>.migrations.*` for a named one.

| Key | Default | Meaning |
|---|---|---|
| `data.migrations.enabled` | `false` | Whether the migrations of this source run at startup. |
| `data.migrations.path` | unset | The directory with the migration files. When unset, the source uses an embedded `EmbeddedMigrations` entry. `path` wins when both exist. |
| `data.migrations.table` | `_sqlx_migrations` | The history table. `[A-Za-z_][A-Za-z0-9_]*`, at most 63 characters. See the warning below. |
| `data.migrations.ignore-missing` | `false` | Whether an applied migration without a file is ignored instead of failing the run. |
| `data.migrations.timeout` | `300s` | The upper bound for the whole run, including the wait for the lock. At least 1ms. |
| `data.migrations.locking` | `true` | Whether the run takes the database lock. Only a database without advisory locks, such as CockroachDB, needs `false`. |

- There is no `validate-checksums` key. A file that was changed after it was applied always fails the run; add a new migration instead of editing an applied one.
- The other keys are validated even when `enabled` is `false`, and then dropped.
- `enabled = true` without a `path` and without an embedded entry for the source is a startup error. A `path` always wins over an embedded entry.
- Every error names the full key and never a value.

```toml
[data]
url = "sqlite://app.db?mode=rwc"

[data.migrations]
enabled = true
path    = "migrations"

[data.sources.reporting.migrations]
enabled = true
path    = "migrations/reporting"
```

### Files

A file is `<version>_<description>.sql`, or a reversible pair `<version>_<description>.up.sql` and `<version>_<description>.down.sql`. The version is a number, usually a timestamp. A directory can mix the two kinds, and the migrations are applied in version order. A SQLite script that starts with `-- no-transaction` runs outside a transaction (and then outside the write lock described below).

```sql
-- migrations/20261125120000_create_accounts.up.sql
CREATE TABLE accounts (id INTEGER PRIMARY KEY, name TEXT NOT NULL);

-- migrations/20261125120000_create_accounts.down.sql
DROP TABLE accounts;
```

### Embedded migrations

The files can also be compiled into the binary and registered as an `EmbeddedMigrations` bean. `sqlx::migrate!` expands to `::sqlx`, so the application needs a direct dependency named `sqlx` with the features `macros` and `migrate`. Going through `oxbloom_data::sqlx` is not enough.

```toml
# Cargo.toml
sqlx = { version = "0.9", default-features = false, features = ["macros", "migrate"] }
```

```rust
use oxbloom_data::migrate::EmbeddedMigrations;
use oxbloom_data::sqlx::migrate::Migrator;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

fn embedded() -> EmbeddedMigrations {
    // `default_source` is the source of the unnamed `data.*` keys, the key `default`;
    // `source("reporting", &OTHER)` registers a named source.
    EmbeddedMigrations::new().default_source(&MIGRATOR)
}

// On the application builder:
// OxbloomApplication::builder().configure(|b| b.instance(embedded()))
```

Register it with `.instance(..)` as shown (see [Dependency injection](03-dependency-injection.md)); the data service reads the optional bean when it starts. Then only `data.migrations.enabled = true` is needed.

### When they run

- The migrations run inside `DataService::start`, after the connectivity check and before the service reports started, so every service with a later phase sees the migrated schema.
- Sources migrate one after the other, the default source first. A failure stops the rest and fails the start. Migrating several sources at once is not supported.
- A source with migrations enabled connects and migrates even when `data.connect-on-startup` is `false`. The other sources stay lazy.
- The failure report names the source, the version of the migration at fault and the key to change. It never contains SQL, a file path or the URL. The error text is fixed (`MigrateError`); the database error stays available through `source()` and may contain SQL object names, so do not log it.
- A dirty history means a migration started and did not finish. On MySQL it means a script failed part way and its earlier statements stay applied. On the other databases it means the transaction was interrupted, for example by a crash of the server. The report names the version and says what to repair; see MySQL below.
- A migration runs on a connection of its own with statement logging switched off, so the SQL text never reaches the logs of `sqlx`.

### Transactions, locking and the databases

- **PostgreSQL** and **MySQL** use the advisory lock of `sqlx`, so two instances that start together apply each migration once. PostgreSQL runs each script in a transaction.
- **SQLite** has no such lock. When `locking` is true, the runner takes the write lock of the database (`BEGIN IMMEDIATE`) for the run, except for a migration marked `-- no-transaction`. After a failure other than a dirty history or a changed file, the runner runs once more, because a second run sees what a concurrent runner has applied.
- A long SQLite migration holds the write lock, so a second instance that starts meanwhile can hit the busy timeout of SQLite and fail its start.
- Two instances can share a SQLite file. Concurrent runners on one shared-cache in-memory database inside one process are not supported: they can fail with SQLite code 262.
- A private-cache in-memory SQLite source (`sqlite://?mode=memory` without a name, or `cache=private`) is refused with an error that names the source, because the migration connection would see another, empty database. `sqlite::memory:` and a named shared-cache memory database (`cache=shared`) work.
- **MySQL commits schema changes implicitly.** A script that fails midway leaves a dirty history row and a partly applied script. The next start fails with the dirty-history report naming the version until you repair the database: finish or undo what the migration changed, then delete its row from the history table. Prefer one statement per file where you can, so a failure leaves nothing half done. PostgreSQL and SQLite roll a failed script back.

### The timeout does not cancel the statement

When `timeout` passes, the runner stops waiting and closes its migration connection, but it does not cancel the statement that is running. On PostgreSQL the backend keeps the advisory lock until the script ends, on SQLite the statement keeps the write lock, and on MySQL a following run can wait. A restart right after a timeout can therefore wait for the old statement. The `MigrationReport`s of concurrent runners can overlap; each migration is still applied exactly once.

### The history table name

`sqlx` changes the table name only through `dangerous_set_table_name`. Renaming the table of an existing database needs a copy of its contents first, or the migrations are applied again. PostgreSQL folds an unquoted name to lower case and `sqlx` creates the table unquoted, so a custom name behaves case-insensitively there.

### Running and reverting from code

`oxbloom_data::migrate::run(&sources, name, &settings, embedded)` applies the pending migrations of one source and returns a `MigrationReport` with the versions it applied. Boot calls it for you. `revert(&sources, name, &settings, embedded, target_version)` goes back to `target_version` with the down files and returns the reverted migrations, highest first; `0` reverts everything. Boot never calls `revert`: it is for your application and your tools.

- Every applied migration above the target needs a down file. Otherwise `revert` fails with `NotReversible` naming the highest such version, before any script runs.
- A changed file does not fail a revert (`sqlx` checks missing files when it undoes, not checksums), but a missing file, a dirty history and a failing script do.
- The settings, the lock, the timeout and the refusal of a private-cache in-memory SQLite database are those of `run`.

For the command line, use `sqlx-cli`; it reads the same files and the same history table.

### Testing migrations

`oxbloom-test` (feature `data`) has `MigrationFiles`, which writes migration files into a temporary directory: `up(version, description, sql)`, `reversible(version, description, up, down)`, `plain(..)` and `path()`. `TestDatabase::sqlite().with_migrations(files.path()).await?` applies them to the test database, and `config_snippet_with_migrations(files.path())` gives a `[data]` section that enables the migrations at startup, for a test that starts the `DataService`.

```rust
use oxbloom_data::sqlx;
use oxbloom_test::{MigrationFiles, TestDatabase};

#[tokio::test]
async fn the_schema_is_migrated() {
    let files = MigrationFiles::new().up(1, "create t", "CREATE TABLE t (id INTEGER)");
    let db = TestDatabase::sqlite().with_migrations(files.path()).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM t")
        .fetch_one(db.sqlite_pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    db.close().await;
}
```

The migration conformance suite runs the same checks against every driver: SQLite always, PostgreSQL and MySQL under `make test-db`, which starts the two servers and runs all test binaries even when one fails.

### Not supported

Repeatable and Flyway-style scripts, a baseline command, automatic down migration, a command line tool of its own (use `sqlx-cli`), migrating several sources concurrently, a dry run, profiles, a lock timeout other than `timeout`, and cancelling a statement after a timeout.

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

- **Migrations have limits.** No repeatable scripts, baseline, automatic down migration, dry run or concurrent migration of several sources, and a timed-out statement is not cancelled. See [Migrations](#migrations).
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
