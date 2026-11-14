# ADR 0076: `WebServer::new` refuses a set `server.ssl.bundle`

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md), [C7 plan](../superpowers/plans/2026-11-14-ssl-bundles.md)

## Context

`server.ssl.bundle` makes the web server speak HTTPS. The TLS configuration comes from the `SslBundles` bean, so the new constructor `WebServer::with_ssl(app, &config, Option<&SslBundles>)` takes the bundles, while `WebServer::new(app, &config)` keeps its signature and has none. Without the feature `ssl` of `oxbloom-boot`, or when an application builds the server with `WebServer::new`, a set key would otherwise be ignored and the port would serve plain HTTP. The slice's rule is that HTTPS is on exactly when the key is set, and that a key is never silently ignored.

## Decision

`WebServer::new` returns a `ConfigError::Invalid` on `server.ssl.bundle` when the key is set. With the feature `ssl` it is `with_ssl(app, config, None)` and the message says that there are no bundles (`available bundles: none`); without the feature the message says that the key needs the `ssl` feature of `oxbloom-boot`. Configurations without the key behave exactly as before. `WebServerAutoConfiguration` calls `with_ssl` with the bean. The key stays in `boot_properties()` even when `WebServerAutoConfiguration` is excluded, as other boot keys do.

## Consequences

Good:

- Setting the key can never give plain HTTP by accident: a missing feature or the wrong constructor fails the startup with a message that names the key.
- The key is new, so no existing configuration changes behaviour.

Bad:

- A user who sets the key and builds the server by hand must switch to `WebServer::with_ssl`.
- The property metadata lists `server.ssl.bundle` also when the web server auto-configuration is excluded or the feature is off.

## Alternatives considered

- Ignore the key in `WebServer::new` and serve plain HTTP: rejected, a silent plaintext fallback breaks "HTTPS only when the key is set".
- Change the signature of `WebServer::new`: rejected, it breaks every caller for a key most of them do not use.
