# ADR 0074: Boot resolves `security.jwt.jwks-ssl-bundle`, and `oxbloom-security` does not depend on `oxbloom-ssl`

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md), section "JWKS fetcher", and the [C7 plan](../superpowers/plans/2026-11-14-ssl-bundles.md), task 7

## Context

The JWKS fetcher must be able to trust a private CA, and send a client certificate, from a named SSL bundle. The spec said that `jwt_decoder_from_config` takes an optional `SslBundles`, and the plan proposed a lookup closure on a new `jwt_decoder_from_config_with_ssl`. Both would widen the decoder API, and the first would make `oxbloom-security` depend on `oxbloom-ssl`. `jwt_decoder_from_config(&Config, Option<Arc<dyn JwksFetcher>>)` already takes the fetcher as an argument. ADR 0069 keeps the TLS crates apart and puts the wiring in boot.

## Decision

`oxbloom-security` knows the key `security.jwt.jwks-ssl-bundle`: the `security.jwt` section accepts it, `jwt_decoder_from_config` refuses an empty name or a name without `jwk-set-uri`, and the public function `jwks_ssl_bundle(&Config) -> Result<Option<String>, ConfigError>` returns the name. An invalid value is a `ConfigError::Invalid` on the key that never shows the value. `ReqwestJwksFetcher::with_tls(Arc<rustls::ClientConfig>)` builds the client with `reqwest::ClientBuilder::tls_backend_preconfigured` and otherwise the settings of `with_timeout`. For it, `rustls` (no provider feature) becomes an optional dependency of the feature `jwks`; `oxbloom-security` does not depend on `oxbloom-ssl`.

`oxbloom-boot`'s `JwtDecoderAutoConfiguration` resolves the name against the optional `SslBundles` bean (feature `ssl`, which enables `oxbloom-security?/jwks`) and passes `ReqwestJwksFetcher::with_tls(bundle.client_config()?)` to the decoder. The key together with a registered `dyn JwksFetcher` bean is a `ConfigError::Invalid` on `security.jwt.jwks-ssl-bundle` with the fixed message "conflicts with a `JwksFetcher` bean; remove one of them": both are explicit choices of the application, and neither may silently win. An unknown name, a name while there is no `SslBundles` bean, or a name in a boot build without `ssl`, is a `ConfigError::Invalid` on the key whose message lists the available bundle names (names are not secrets), never contents. Without the key nothing changes.

Trust follows the bundle. A bundle with a trust store replaces the platform roots, so only its CAs are trusted and a certificate from a public identity provider is then rejected. A bundle without a trust store keeps the platform verifier and only adds its client certificate.

The fetcher is built once, when the decoder is built. It does not follow a reload of the bundle's files.

## Consequences

Good:

- The decoder API stays as it is; `oxbloom-security` can be used with any rustls configuration.
- A set key is never silently ignored: it is checked in `oxbloom-security` and resolved, or refused, in boot.
- A `JwksFetcher` bean with its own proxy, timeout, caching or authentication is never thrown away unnoticed; the conflict stops the startup and names the key.

Bad:

- A rotated CA or client certificate reaches the JWKS client only after a restart.
- An application with its own `JwksFetcher` bean cannot use the key; it builds its fetcher with `ReqwestJwksFetcher::with_tls` itself, or wraps one, to use a bundle.
- A trust store in the bundle shuts out the platform roots; an issuer behind a public CA needs that CA in the bundle too.
- `tls_backend_preconfigured` matches the rustls version that `reqwest` uses; a `rustls` major version split would make the client unbuildable (`FetchError::Network`).

## Alternatives considered

- `jwt_decoder_from_config` taking an optional `SslBundles`: rejected, it makes `oxbloom-security` depend on `oxbloom-ssl`.
- A lookup closure on a new `jwt_decoder_from_config_with_ssl`: rejected, it adds a third decoder entry point for what one fetcher argument already covers.
- The bundle replaces the `JwksFetcher` bean: rejected, it silently throws away a bean the application registered on purpose, against the convention that user beans take precedence.
- The `JwksFetcher` bean wins and the key is ignored: rejected, it silently ignores a key the application set on purpose, against the rule that a set key is never silently ignored.
- A fetcher that rebuilds its client when the bundle reloads: deferred; it needs a reload hook on `SslBundle`.
