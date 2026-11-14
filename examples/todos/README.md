# Todos example

A CRUD REST API on the Oxbloom web, security and logging stack, with the actuator
enabled. Start it on a free port and note the address it prints.

```text
cargo run -p todos -- --server.port=0
```

The commands below use port 8081, the port in `application.toml`. Start the example
without `--server.port=0` to use it.

## Actuator

The health probes are open, because the security rules of the example permit
`GET /actuator/health/**`.

```text
curl http://127.0.0.1:8081/actuator/health
curl http://127.0.0.1:8081/actuator/health/liveness
curl http://127.0.0.1:8081/actuator/health/readiness
```

The other actuator endpoints need an authenticated request, as every path outside
`/api/todos/**` does. The example configures no `security.jwt`, so without one they
answer 401.

```text
curl http://127.0.0.1:8081/actuator/info
```

With a token, the info endpoint shows the build facts that `build.rs` collects (name,
version, git sha and branch, and the build time) and the platform. The git facts are
left out when git is not available at build time.

Logger targets use `::` as the separator, so a dotted target is rejected with 400. This
call sets the level of the `todos` target to `DEBUG` at run time:

```text
curl -X POST -H 'content-type: application/json' \
  -d '{"configuredLevel":"DEBUG"}' \
  http://127.0.0.1:8081/actuator/loggers/todos
```
