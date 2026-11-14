# ADR 0086: Password login uses Argon2id behind a `{id}hash` format, answers every failed login alike, and splits HTTP Basic (6a) from form login (6b)

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** the security Slice 6a design spec `2026-11-14-password-basic-design.md` (a local file, not linked) and its plan

## Context

An application needs to check a username and a password. Several decisions shape that feature. Which hash algorithm is the default, and how can an application change it later without breaking stored hashes? What does a client learn from a failed login, in the answer and in the time it takes? Where does the expensive hash run? How much of Spring's Slice 6 (`PasswordEncoder`, `UserDetailsService`, HTTP Basic, form login with sessions) fits one slice? Which hash crate does the project accept, given that `oxbloom-security` keeps `aws-lc-rs` as its only crypto provider?

The security chain, the providers and the `Authentication` types come from Slice 5. The boot wiring follows [ADR 0048](0048-contributor-beans.md).

## Decision

**Argon2id is the default and the only algorithm.** `Argon2PasswordEncoder` hashes with Argon2id and stores a PHC string (`$argon2id$v=19$m=19456,t=2,p=1$salt$hash`). The defaults are the OWASP minimum: 19 MiB of memory (`19456` KiB), 2 iterations and 1 lane. They are configurable through `security.password.argon2.*` and checked at startup (memory 8 to 1 048 576 KiB and at least 8 KiB per lane, iterations 1 to 100, lanes 1 to 64). bcrypt and scrypt are not offered; the format below leaves room for them.

**The stored form is `{id}hash`.** `DelegatingPasswordEncoder` writes `{argon2}` in front of the PHC string and verifies with the encoder that the prefix names. A hash with an unknown or missing prefix never matches. The prefix lets an application change the default algorithm or the cost later while old hashes keep verifying, and `PasswordEncoder::upgrade_encoding` tells which stored hashes are stale. The only registered id in 6a is `argon2`, and `security.password.default-encoder` accepts only that value.

**No crypto provider beyond `aws-lc-rs`.** The RustCrypto `argon2` crate is pure Rust and does not pull in `ring`. It is checked by hand with `cargo tree -i ring --workspace --all-features` at each slice gate; no CI step exists.

**Every failed login looks alike.** An unknown user, a wrong password, an empty password and a too long credential all end in `AuthenticationError::BadCredentials`. A disabled or a locked account is reported only after the password was right (`AccountDisabled` and `AccountLocked`), so a client without the password cannot learn that an account exists or what state it is in. The chain turns every one of these errors into the same 401 with the same `WWW-Authenticate` header; the variants are for logs and for a custom entry point. Failed logins log at `debug` on the target `oxbloom::security` with the variant and the length of the username, never the username, the password or a hash.

**The same time, as far as it can be.** For an unknown user the provider verifies the password against a dummy hash made by the same encoder when the provider was built, so the expensive step runs in both cases. A user whose stored hash is malformed gets no such step: the hash fails to parse and the answer comes sooner than for a wrong password. That is a narrow difference, which only someone who can write the user store can cause, and it is a known limit. A username or a password longer than 1024 bytes fails before any hashing, because the check costs nothing and a long value would be a cheap way to burn CPU.

**Hashing runs in `spawn_blocking`.** Argon2 with 19 MiB takes a noticeable amount of CPU time, which must not stall the runtime's worker threads. `DaoAuthenticationProvider` calls `matches` inside `tokio::task::spawn_blocking`. A failed join (a panic in the encoder) is `ProviderUnavailable`, which the chain answers with 503.

**Users are a trait, with an in-memory store.** `UserDetailsService::load_user_by_username` is async, so a database store can come later. `InMemoryUserDetailsService` holds users built in code or read from `security.users.<name>`. A password in configuration must already be a `{argon2}` hash; a plain-text value fails the startup with an error that names the key and never shows the value.

**HTTP Basic is opt-in.** `security.http-basic.enabled` defaults to `false`, and configured users do not open Basic by themselves. A deployment that adds users for a later feature must not suddenly accept passwords on every route. With Basic on, the chain adds every `dyn AuthenticationProvider` bean as a password provider, and `HttpSecurity::build` fails with `HttpBasicWithoutProvider` when none exists. A request without credentials, or with a failed login, gets `WWW-Authenticate: Basic realm="Oxbloom", charset="UTF-8"` (the realm is configurable); a bearer provider on the same chain adds its challenge after the Basic one.

**The split from Slice 6b.** Slice 6a is everything that does not need server-side state: the encoder, the user store, the provider and Basic. Form login needs sessions, a session store, logout, CSRF and a redirect flow, and it is a different kind of feature (state between requests). Splitting keeps 6a reviewable and lets Basic ship alone. Slice 6b has form login, sessions and logout.

## Consequences

Good:

- Passwords are stored with a modern, memory-hard hash, in a format that can change algorithm.
- A failed login gives away neither the existence nor the state of an account in the answer, and almost nothing in the time.
- A bad password setting, a plain-text password and a missing provider fail at startup with the key named.
- Adding users never changes who can log in over Basic.

Bad:

- `matches` verifies with the memory, iterations and lanes stored in the hash, so each concurrent login uses the memory of that hash (19 MiB with the default cost) on a blocking thread, and a client can send many Basic requests at once. There is no limit on concurrent logins in 6a. Anonymous requests are bounded only by the blocking pool of tokio (512 threads by default), so the worst case is about 512 x 19 MiB, roughly 9.5 GiB at the default cost. A login limit or a semaphore around the hash is deferred. This is the reason a login limit comes later, together with rate limiting.
- The equal time of an unknown user holds only while stored hashes use the configured parameters: the dummy hash uses the configured cost and a stored hash keeps its own. Raising the cost without re-hashing lets timing reveal which usernames exist.
- There is no rate limiting and no lockout counter. A client can guess passwords as fast as the hash lets it. Basic sends the password with every request, so it belongs behind TLS.
- There is no database store, and a user change needs a restart.
- A stale hash is not re-encoded on login. `upgrade_encoding` exists, and the application can call it, but nothing does it for it.
- A user with a malformed stored hash answers faster than other users (the narrow timing difference above).
- A malformed `Authorization` header gets the built-in challenge and never reaches a custom `entry_point`.
- The request enum `AuthenticationRequest` gained the variant `UsernamePassword`, the error enum `AuthenticationError` gained `AccountDisabled` and `AccountLocked`, and `SecurityBuildError` gained `HttpBasicWithoutProvider` and `InvalidRealm`. None of these enums is `#[non_exhaustive]`, so a downstream `match` without a wildcard arm stops compiling. This is a semver-breaking change for a published crate.
- Keys under `security.users.<name>` follow the configuration crate, which matches keys ignoring case, `-` and `_`. Two names that differ only in these fail to load from one file (`DuplicateKey`) and merge when they come from two sources. The spelling as written is kept and compared exactly at login.

## Alternatives considered

- bcrypt or PBKDF2 as the default: rejected. Argon2id is the current OWASP first choice and resists GPU attacks better.
- A bare PHC string without a prefix: rejected, because a later change of algorithm would then have no way to tell the schemes apart.
- Hashing on the async worker: rejected, see `spawn_blocking`.
- Making the user store synchronous: rejected, because a database store would then block a worker thread.
- Basic on whenever users exist: rejected, see above.
- Plain-text passwords in configuration with hashing at startup: rejected, because the file would then hold secrets that work as they are.
- Form login in the same slice: rejected, see the split.
- Marking the enums `#[non_exhaustive]` in this slice: not done. That would be a second break for the same `match` expressions on top of the new variants, and it is left for a separate decision. Adding the attribute before a first release would stop this kind of break for later variants.
