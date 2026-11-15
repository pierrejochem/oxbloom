# Password login

Oxbloom can authenticate a user by username and password. The crate `oxbloom-security` holds the password encoder, the user types, the `DaoAuthenticationProvider` and the `HttpSecurity` methods for HTTP Basic, and the feature `security` of `oxbloom-boot` wires them from configuration. This is security Slice 6a. Form login with sessions is Slice 6b and is not built yet.

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["security"] }
```

The design is in the Slice 6a spec and plan (both are local files under `docs/superpowers`, so this page names them without a link), the decisions are in [ADR 0086](../adr/0086-password-login.md), and every public item has an example in the [API reference](../API.md#47-oxbloom-security). The example `examples/todos` protects `GET /api/admin/stats` with HTTP Basic (see [Example](14-example.md#example)).

## Password encoding

`PasswordEncoder` has three methods: `encode(raw)` hashes with a fresh random salt, `matches(raw, encoded)` checks a password, and `upgrade_encoding(encoded)` says whether a stored hash is weaker than the encoder's settings or cannot be read. Two encoders ship:

- `Argon2PasswordEncoder` hashes with Argon2id and stores a PHC string such as `$argon2id$v=19$m=19456,t=2,p=1$...`. `Argon2PasswordEncoder::default()` uses the OWASP minimum: 19 MiB of memory, 2 iterations and 1 lane. `Argon2PasswordEncoder::new(memory_kib, iterations, parallelism)` returns a `PasswordEncoderError::InvalidParameters` when Argon2 rejects the values.
- `DelegatingPasswordEncoder` writes `{id}hash` and verifies with the encoder that the id names. `DelegatingPasswordEncoder::argon2(encoder)` registers the encoder under the id `argon2` (the constant `ARGON2_ID`), so a stored value looks like `{argon2}$argon2id$v=19$...`. A hash with an unknown or missing prefix never matches. `DelegatingPasswordEncoder::new(default_id, encoders)` builds one with several ids and fails with `UnknownDefaultId` or `DuplicateId`.

The prefix is what lets an application change the algorithm or the cost later while old hashes keep verifying. Only `argon2` exists today.

To make a hash for a configuration file, run a few lines of Rust once (a test or a small binary) and paste the output:

```rust
use oxbloom_security::{Argon2PasswordEncoder, DelegatingPasswordEncoder, PasswordEncoder};

let encoder = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::default());
println!("{}", encoder.encode("a password"));
```

No encoder, `UserDetails` or provider prints a password or a hash in `Debug` output or in an error message.

## Users

`UserDetails::new(username, password, authorities)` holds a user. The `password` is the stored hash, not the plain text. `.roles(&["ADMIN"])` adds `ROLE_ADMIN`, `.enabled(false)` and `.locked(true)` change the account state (a user is enabled and unlocked by default). The `UserDetailsService` trait loads a user by name:

```rust
use async_trait::async_trait;
use oxbloom_security::{UserDetails, UserDetailsError, UserDetailsService};

struct Directory;

#[async_trait]
impl UserDetailsService for Directory {
    async fn load_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        // Look the user up in your own store; `None` means no such user.
        Ok(None)
    }
}
```

`InMemoryUserDetailsService::new(users)` is the store that ships. It fails with `UserStoreError::DuplicateUsername` or `EmptyUsername`. Names are compared exactly, so `Alice` and `alice` are two users.

## HTTP Basic

`DaoAuthenticationProvider::new(users, encoder)` is the provider for username and password requests. It takes an `Arc<dyn UserDetailsService>` and an `Arc<dyn PasswordEncoder>`. Building it hashes one dummy password, which is used later to equalise timing. `HttpSecurity` adds Basic with two calls:

```rust
use std::sync::Arc;
use oxbloom_security::{
    Argon2PasswordEncoder, DaoAuthenticationProvider, DelegatingPasswordEncoder, HttpSecurity,
    InMemoryUserDetailsService,
};

let users = Arc::new(InMemoryUserDetailsService::new(vec![])?);
let dao = DaoAuthenticationProvider::new(users, Arc::new(DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::default())));
let chain = HttpSecurity::new()
    .http_basic(|b| b.realm("Admin area"))
    .password_provider(Arc::new(dao))
    .authorize(|a| a.any_request().authenticated())
    .build()?;
```

`http_basic` switches the extractor on. Without it an `Authorization: Basic` header leaves the request anonymous. `password_provider` adds a provider for `AuthenticationRequest::UsernamePassword`; providers are tried in the order they were added. `build()` fails with `SecurityBuildError::HttpBasicWithoutProvider` when `http_basic` has no `password_provider`, and with `SecurityBuildError::InvalidRealm` when the realm contains a double quote, a backslash or a control character.

The chain decodes the Base64 token, splits the text at the first colon (so a password may contain colons) and hands the pair to the providers. The username and the password are each limited to 1024 bytes, and a longer value fails before any hashing. An empty username or an empty password never logs in.

### Answers

| Case | Status | `WWW-Authenticate` |
|---|---|---|
| No `Authorization` header, route needs authentication | 401 | `Basic realm="Oxbloom", charset="UTF-8"`, and `Bearer` after it when a bearer provider exists |
| Wrong password, unknown user, empty password | 401 | the Basic challenge |
| Right password, account disabled or locked | 401 | the Basic challenge (the same answer as a wrong password) |
| Malformed Basic header (bad Base64, no colon, not UTF-8, empty token) | 401, never 400 | the Basic challenge |
| Logged in, but the rule denies (for example the wrong role) | 403 | none |
| The user store fails, or the hash task panics | 503 | none |
| `Basic` header while `http_basic` is off | anonymous | as for an anonymous request |

The realm defaults to `Oxbloom`. The chain builds empty bodies, and the error filter of `oxbloom-web` renders them as `application/problem+json` and keeps the challenge header. A client cannot tell from the answer whether a user exists or whether an account is disabled or locked; the reason is in the `debug` log on the target `oxbloom::security`, which has the failure variant and the length of the username, and never the username, the password or a hash.

With a custom `entry_point`, failed and anonymous requests get its answer instead of the Basic challenge. A malformed `Authorization` header is the exception: the chain answers it with its own 401 and the built-in challenge and never calls the custom entry point.

A failed login is slower on purpose. For an unknown user the provider verifies the password against the dummy hash, so an unknown user and a wrong password take about the same time, as long as the stored hashes use the configured parameters: the dummy hash uses the configured cost, while a stored hash keeps the cost it was made with, so raising the cost without re-hashing the users lets the timing reveal which usernames exist. The other exception is a user whose stored hash is malformed: that check fails at once, so such a user answers faster. Only someone who controls the user store can create that case.

The hash runs in `tokio::task::spawn_blocking`, so a login does not block the worker threads of the runtime.

## Configuration

With the feature `security` of `oxbloom-boot`, `PasswordEncoderAutoConfiguration` reads these keys and `SecurityAutoConfiguration` uses them.

| Key | Default | Meaning |
|---|---|---|
| `security.http-basic.enabled` | `false` | Turns HTTP Basic on. Users alone do not turn it on. |
| `security.http-basic.realm` | `Oxbloom` | The realm of the challenge; no double quote, backslash or control character. |
| `security.password.default-encoder` | `argon2` | The id new hashes are stored with. Only `argon2` is accepted. |
| `security.password.argon2.memory-kib` | `19456` | Memory in KiB, 8 to 1 048 576 and at least 8 per lane. |
| `security.password.argon2.iterations` | `2` | Passes, 1 to 100. |
| `security.password.argon2.parallelism` | `1` | Lanes, 1 to 64. |
| `security.users.<name>.password` | required | The stored hash: `{argon2}` followed by an Argon2 PHC string. |
| `security.users.<name>.roles` | empty | Role names without the `ROLE_` prefix; an empty entry is an error. |
| `security.users.<name>.enabled` | `true` | A disabled user cannot log in. |
| `security.users.<name>.locked` | `false` | A locked user cannot log in. |

These keys are listed in the property metadata of `PasswordEncoderAutoConfiguration`, including the two `security.http-basic.*` keys that `SecurityAutoConfiguration` reads. Unknown keys in each section are rejected with the key named, and so is a value of the wrong type or out of range (the message gives the range, never the value). A plain-text password fails the startup with the key `security.users.<name>.password` and not the value.

```toml
[security.http-basic]
enabled = true
realm   = "Admin area"

[security.password.argon2]
memory-kib = 19456
iterations = 2

[security.users.admin]
password = "{argon2}$argon2id$v=19$m=19456,t=2,p=1$...$..."
roles    = ["ADMIN"]

[security.users.viewer]
password = "{argon2}$argon2id$v=19$m=19456,t=2,p=1$...$..."
roles    = ["USER"]
locked   = false
```

Rules for the user names:

- The key under `security.users` is the user name. A name with dots needs quotes in TOML (`[security.users."ops.bot"]`). Environment variables give lowercase names.
- `oxbloom-config` matches keys ignoring case, `-` and `_`. Two names that differ only in these, such as `Alice` and `alice`, are the same key: from one file they fail to load with a duplicate key error, and from two sources they merge into one user. The spelling that was written is kept and compared exactly at login, so a client must send `Alice` and not `alice`.

### What the auto-configuration registers

`PasswordEncoderAutoConfiguration` registers, in this order:

1. A `DelegatingPasswordEncoder` from `security.password.*`, provided as `dyn PasswordEncoder`, unless the application registered its own `dyn PasswordEncoder`.
2. An `InMemoryUserDetailsService` with the users of `security.users`, provided as `dyn UserDetailsService`, when the section `security.users` exists and the application registered no `dyn UserDetailsService`.
3. A `DaoAuthenticationProvider`, provided as `dyn AuthenticationProvider`, when both a `dyn UserDetailsService` and a `dyn PasswordEncoder` exist.

`SecurityAutoConfiguration` runs after it. With `security.http-basic.enabled = true` it calls `http_basic` with the realm and adds every `dyn AuthenticationProvider` bean as a password provider, not only the DAO provider. With Basic on and no such bean the startup fails with `HttpBasicWithoutProvider`, which the security failure analyzer reports. The keys of `security.password` are not read when the application registered its own encoder, and those of `security.users` are not read when it registered its own user store.

To use a database store, register your own bean:

```rust
.configure(|b| {
    b.instance(Directory)
        .provides(|d: Arc<Directory>| -> Arc<dyn UserDetailsService> { d })
})
```

The DAO provider then uses it with the configured encoder.

## Limits

- There is no rate limiting and no lockout counter. Nothing slows down or stops a client that guesses passwords, except the cost of the hash.
- There is no database store; `InMemoryUserDetailsService` is the only one. Users from configuration change only with a restart.
- A stale hash is not re-encoded on login. `upgrade_encoding` reports it, and the application can re-hash itself, but nothing does it automatically.
- `matches` verifies with the memory, iterations and lanes recorded in the stored hash, so each concurrent login uses the memory of that hash (19 MiB with the default cost) on a blocking thread. There is no limit on concurrent logins yet. Anonymous requests are bounded only by the blocking pool of tokio (512 threads by default), so the worst case is about 512 x 19 MiB, roughly 9.5 GiB at the default cost. A login limit or a semaphore around the hash is deferred. Rate limiting comes with it.
- A malformed `Authorization` header bypasses a custom `entry_point` and gets the built-in Basic challenge.
- Timing of an unknown user is equalised with a dummy hash, but a user with a malformed stored hash answers faster. The difference is narrow and needs control of the user store.
- Basic sends the password with every request. Use it over TLS (see [SSL bundles](16-ssl-bundles.md#ssl-bundles)).
- The raw password is not zeroized after use.
- Form login, sessions, logout and remember-me are Slice 6b.

### Semver

The slice added variants to public enums that are not `#[non_exhaustive]`: `AuthenticationRequest::UsernamePassword`, `AuthenticationError::AccountDisabled` and `AccountLocked`, and `SecurityBuildError::HttpBasicWithoutProvider` and `InvalidRealm`. A `match` on one of them without a wildcard arm stops compiling, which is a breaking change for a published crate.
