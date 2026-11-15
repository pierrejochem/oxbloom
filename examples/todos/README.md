# Todos example

A CRUD REST API on the Oxbloom web, security and logging stack, with the actuator
and the task scheduler enabled. Start it on a free port and note the address it prints.

```text
cargo run -p todos -- --server.port=0
```

The commands below use port 8081, the port in `application.toml`. Start the example
without `--server.port=0` to use it.

## Admin route with HTTP Basic

`GET /api/admin/stats` needs the role `ADMIN`. The example turns on HTTP Basic with
`security.http-basic.enabled = true` and configures two users under `security.users` in
`application.toml`: `admin` with the role `ADMIN` and `viewer` with the role `USER`. Their
passwords are `{argon2}` hashes of the password encoder. Plain-text passwords are refused
at start.

The passwords `changeit` (admin) and `viewonly` (viewer) are demo values only. They are
committed in the open, so change them, and keep real hashes out of version control, before
any real use.

```text
curl -u admin:changeit http://127.0.0.1:8081/api/admin/stats      # 200
curl -u viewer:viewonly http://127.0.0.1:8081/api/admin/stats     # 403
curl http://127.0.0.1:8081/api/admin/stats                        # 401 with a Basic challenge
```

Basic sends the password with every request, so use it over TLS only.

## Actuator

The health probes are open, because the security rules of the example permit
`GET /actuator/health/**`.

```text
curl http://127.0.0.1:8081/actuator/health
curl http://127.0.0.1:8081/actuator/health/liveness
curl http://127.0.0.1:8081/actuator/health/readiness
```

The other actuator endpoints, `/actuator/info` and `/actuator/loggers`, need
authentication, as every path outside `/api/todos/**` and the health probes does. With HTTP
Basic on, any authenticated user can read them, including the demo user `viewer`
(`curl -u viewer:viewonly` gets 200 from `/actuator/info`), because the rules ask for
authentication and no role. An anonymous call answers 401 as written here.

```text
curl http://127.0.0.1:8081/actuator/info
```

An application that configures `security.jwt` can call them with a bearer token. The info
endpoint then shows the build facts that `build.rs` collects (name, version, git sha and
branch, and the build time) and the platform; the git facts are left out when git is not
available at build time.

Logger targets use `::` as the separator, so a dotted target is rejected with 400. In an
application with `security.jwt` configured, this call sets the level of the `todos` target
to `DEBUG` at run time. In this example, an anonymous call answers 401.

```text
curl -X POST -H 'content-type: application/json' \
  -H 'Authorization: Bearer <token>' \
  -d '{"configuredLevel":"DEBUG"}' \
  http://127.0.0.1:8081/actuator/loggers/todos
```

## Scheduled cleanup

The `TodoCleanup` bean in `src/cleanup.rs` has one scheduled task, `TodoCleanup::cleanup`. It
removes every todo that has been done for longer than `todos.cleanup.max-age`, which is 24
hours in `application.toml`. The task uses `fixed_delay = "1m"`, so the first run happens
when the scheduler starts, and every later run starts one minute after the previous run ended.
Setting `tasks.scheduling.initial-delay` postpones the first run. The period is written in the attribute and cannot be changed by configuration; the age can, for
example with `--todos.cleanup.max-age=30m`. A todo that is set back to not done is not removed.

Boot starts the scheduler because the bean is registered as a `dyn Scheduling`. Its settings
are the `tasks.scheduling.*` keys: `enabled` (set it to `false` to run no task),
`max-concurrent`, `shutdown-timeout`, `zone` and `initial-delay`. For example,
`--tasks.scheduling.enabled=false` switches the cleanup off. The scheduler logs its events at the
target `oxbloom::tasks`, and the cleanup logs the number of removed todos at the target `todos`.

### Retry

The cleanup reaches the store through `TodoCleanup::purge_at`, which carries
`#[retryable(max_attempts = 3, initial = "50ms")]`. When the store fails, the call is repeated
after a delay that doubles each time, up to three attempts in total, and the last error is
returned if all of them fail. The attribute needs an `async` method that returns a `Result`
whose error implements `Display`. A call that succeeds at the first attempt logs nothing; each
retry logs a warning and an exhausted call logs an error, both at the target `oxbloom::tasks`.

Boot applies the `tasks.retry.*` keys as the defaults of every `#[retryable]` method:
`max-attempts`, `backoff`, `initial`, `multiplier`, `max-delay` and `jitter`. The commented
`[tasks.retry]` block of `application.toml` lists them with their defaults, and a key written in
the attribute wins over the configuration. The in-memory store never fails by itself, so the tests
call `TodoStore::fail_next(2)` to make the next two calls fail.
