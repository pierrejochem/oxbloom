# SSL bundles

An SSL bundle is a named set of TLS material, loaded from configuration: an identity (a certificate chain and its private key), a trust store (CA certificates), or both. The crate `oxbloom-ssl` loads bundles from the `ssl.*` keys, reloads them when their files change and warns about certificates that expire. Two consumers use them today: the web server, which serves HTTPS with a bundle's identity, and the JWKS fetcher of the JWT resource server, which can trust a private CA and send a client certificate. In an application you only write keys; the feature `ssl` of `oxbloom-boot` does the wiring.

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["ssl"] }   # turns `web` on as well
```

The design is in the [C7 spec](../superpowers/specs/2026-11-14-ssl-bundles-design.md), the decisions are ADRs [0068](../adr/0068-rustls-with-aws-lc-rs.md) to [0080](../adr/0080-zero-reload-interval-has-no-validity-repeat.md), and every public item has an example in the [API reference](../API.md#411-oxbloom-ssl).

## Configuration keys

```toml
[ssl]
reload-interval = "30s"   # "0s" turns polling off
expiry-warning  = "14d"

[ssl.bundle.pem.web]
certificate = "config/web.crt"   # a file path, or inline PEM (starts with -----BEGIN)
private-key = "config/web.key"   # Secret; a file path or inline PEM
trust       = "config/ca.pem"    # optional

[ssl.bundle.pkcs12.legacy]
path     = "config/legacy.p12"
password = "${LEGACY_P12_PASSWORD}"   # Secret

[server.ssl]
bundle = "web"

[security.jwt]
jwks-ssl-bundle = "idp"
```

| Key | Default | Meaning |
|---|---|---|
| `ssl.reload-interval` | `30s` | How often the bundle files are checked for changes. `0s` turns polling off. |
| `ssl.expiry-warning` | `14d` | A certificate that expires within this time produces a warning. |
| `ssl.bundle.pem.<name>.certificate` | unset | The certificate chain, leaf first. Requires `private-key`. |
| `ssl.bundle.pem.<name>.private-key` | unset | The unencrypted private key (PKCS#8, PKCS#1 or SEC1), a `Secret`. Requires `certificate`. |
| `ssl.bundle.pem.<name>.trust` | unset | The CA certificates the bundle trusts. |
| `ssl.bundle.pkcs12.<name>.path` | unset | A PKCS#12 file: its key and chain are the identity, every other certificate is the trust store. |
| `ssl.bundle.pkcs12.<name>.password` | empty | The password of the PKCS#12 file, a `Secret`. |
| `server.ssl.bundle` | unset | The bundle the web server serves HTTPS with. |
| `security.jwt.jwks-ssl-bundle` | unset | The bundle the JWKS client uses. |

Rules that hold for every bundle:

- A value that starts with `-----BEGIN` (after leading whitespace) is inline PEM. Any other value is a file path, relative to the current directory of the process.
- `certificate` without `private-key`, or the reverse, is an error naming the missing key. A bundle with neither an identity nor a trust store is an error too.
- A name is unique across `pem` and `pkcs12`. Names and keys match like the rest of the configuration: case-insensitive, ignoring `-` and `_`. An unknown key under a bundle is an error that lists the valid keys.
- `private-key` and `password` are `Secret` values. No error, `Debug` output, log event or failure report contains a private key, a password, a file path or file contents; they name the bundle and the key to fix instead.
- The property metadata lists every key (`OxbloomApplicationBuilder::property_metadata()`), and the conditions report says why `SslAutoConfiguration` ran or not.

## Bundles from PEM

A PEM bundle can hold an identity, a trust store or both. The PEM text can come from files or sit inline in the configuration, which keeps tests free of files:

```rust
use oxbloom_config::Config;
use oxbloom_ssl::SslBundles;

// `cert_pem` and `key_pem` hold PEM text; a file path works the same way.
let config = Config::from_str(&format!(
    "[ssl.bundle.pem.web]\ncertificate = '''\n{cert_pem}'''\nprivate-key = '''\n{key_pem}'''"
))?;
let bundles = SslBundles::from_config(&config)?;
let web = bundles.get("web")?;
assert!(web.has_identity() && !web.has_trust());
```

A file with several certificates (a chain, with LF or CRLF line ends) loads in order. An empty file, a malformed value, an encrypted private key and a private key that does not match the first certificate each fail the startup with an `SslError` (`Pem` or `KeyMismatch`) naming the key. Encrypted PEM keys are not supported: decrypt the key, or use PKCS#12 with a password.

## Bundles from PKCS#12

A PKCS#12 file (`.p12`, `.pfx`) holds the identity and, often, the CA chain. The leaf is the certificate that belongs to the private key; every other certificate in the file becomes the trust store. Keep the password out of the file with a placeholder:

```toml
[ssl.bundle.pkcs12.legacy]
path     = "config/legacy.p12"
password = "${LEGACY_P12_PASSWORD}"
```

```rust
let legacy = SslBundles::from_config(&config)?.get("legacy")?;
assert!(legacy.has_identity());
```

A wrong password or a damaged file is `SslError::Pkcs12`. Files written by OpenSSL 3 and `keytool` (PBES2) load, and so do older files with 3DES or RC2 key bags. JKS keystores are not supported; convert them with `keytool -importkeystore -deststoretype pkcs12`.

## HTTPS for the web server

Set `server.ssl.bundle` to a bundle with an identity, and the port of the auto-configured web server speaks HTTPS:

```toml
[server]
port = 8443

[server.ssl]
bundle = "web"
```

- The port speaks only HTTPS. There is no second HTTP listener and no redirect from HTTP; put a proxy in front when you need one.
- ALPN is `http/1.1` only, so a client that offers only `h2` fails the handshake. Mutual TLS (client certificates on the server side) is not supported.
- Each connection is handshaken in its own task with a 10 second timeout. A failed or timed-out handshake is logged at `debug` on target `oxbloom::web` with the peer address and the error kind, and does not affect other connections. A handshake still running when shutdown starts is dropped at once; established connections drain under `server.shutdown-timeout` as they do without TLS.
- `Request::scheme()` reports `https`, unless a trusted proxy's forwarded headers say otherwise.
- An unknown bundle name, or a bundle without an identity, fails the startup with a `ConfigError` on `server.ssl.bundle` that lists the available bundle names.
- `server.ssl.bundle` without the `ssl` feature is an error too, and so is `WebServer::new` with the key set: the server never falls back to plain HTTP when you asked for HTTPS. Build the server with `WebServer::with_ssl(app, &config, Some(&bundles))` when you construct it by hand.

Without boot, `SslBundle::server_config()` gives the `rustls::ServerConfig` (ALPN `http/1.1` included) for `App::serve_tls`, behind the feature `tls` of `oxbloom-web`:

```rust
use tokio_rustls::TlsAcceptor;

let acceptor = TlsAcceptor::from(bundles.get("web")?.server_config()?);
let listener = tokio::net::TcpListener::bind("127.0.0.1:8443").await?;
app.serve_tls(listener, acceptor, async { let _ = tokio::signal::ctrl_c().await; }).await?;
```

## Reload

Certificates rotate. `SslBundles` runs one background task that wakes every `ssl.reload-interval` and compares the size and modification time of every file a bundle names, following symlinks, so a Kubernetes secret mount that swaps its `..data` symlink is seen. When a file changed, the whole bundle is rebuilt from all its sources and swapped in atomically. The next handshake uses the new certificate; connections that finished their handshake keep the old one, and a handshake never waits for a reload. In boot the container starts the task when it creates the bean and stops it at shutdown.

- The metadata is read before and after the files. When it differs, the attempt is dropped and the next tick tries again, so a half-written file is not loaded.
- A failed reload keeps the old material and logs one `warn` on target `oxbloom::ssl` with the bundle name and the error kind (`pem`, `key-mismatch`, `no-identity`, `io (entity not found)`, ...). It logs again only after the files change again, so a bad file does not flood the log.
- A reload never changes the parts a bundle has. A PKCS#12 file replaced by a certificate-only export (no key) would leave the server without a certificate, so it is refused with `no-identity` and the old identity is served on. New files that drop the trust store (`empty-bundle`) or add an identity or a trust store (`shape-changed`) are refused the same way. Restart the application to change what a bundle holds.
- Replace a certificate and its key together, with a directory or symlink swap as Kubernetes does. Two separate writes can be seen in between and give one `key-mismatch` warning before the second write is picked up.
- `SslBundles::reload()` rebuilds every bundle on demand and returns the result per bundle. Nothing calls it automatically.

Limits of reload:

- It polls, so a change applies within one `ssl.reload-interval` (30 seconds by default). `0s` turns polling off; the bundles then load once at startup.
- Inline PEM values and PKCS#12 passwords are not watched; they come from the configuration, which does not reload.
- An in-place rewrite that keeps the size within one tick of the file system's clock can be missed. Writing a new file and renaming it is detected.
- A client configuration fixes its trust store when it is built, so a reloaded trust store reaches only client configurations built afterwards. The JWKS client in particular does not reload (see below).

## Validity warnings

After every successful load, each certificate's `notBefore` and `notAfter` are checked against the `Clock` bean (the system time without one). An expired certificate, one that is not valid yet, and one that expires within `ssl.expiry-warning` each log a `warn` on target `oxbloom::ssl` with the bundle name, the subject and the date. The warning repeats every 24 hours of the clock while the condition holds, and at once when it changes (an expiring certificate that has expired). These checks only warn: a bad date never stops the startup, because refusing to start would turn a warning into an outage.

- The repeat runs inside the reload task, so it comes within one `ssl.reload-interval` of being due. With `ssl.reload-interval = "0s"` there is no task and no 24 hour repeat: only the load and `SslBundles::reload()` warn.
- `SslBundle::validity()` returns the subject and dates of every certificate (`CertValidity`), for your own checks or a later health endpoint.

## The JWKS fetcher with a bundle

A JWT issuer whose certificate comes from a private CA, or that wants a client certificate, needs its own TLS configuration. With the features `ssl` and `security`, name a bundle:

```toml
[ssl.bundle.pem.idp]
trust = "config/corporate-ca.pem"

[security.jwt]
algorithms      = ["RS256"]
jwk-set-uri     = "https://idp.corp.example/jwks.json"
jwks-ssl-bundle = "idp"
```

Trust rules:

- A bundle with a trust store replaces the platform roots: the JWKS client trusts only those CAs, so a certificate from a public CA is then rejected.
- A bundle without a trust store keeps the platform verifier and only adds its identity as a client certificate.
- Without the key nothing changes: the platform verifier stays the default.

Errors and limits:

- An unknown name, or no bundles at all, is a `ConfigError::Invalid` on `security.jwt.jwks-ssl-bundle` that lists the available bundle names. The key without `jwk-set-uri` is an error as well.
- The key together with a `JwksFetcher` bean of your own is a `ConfigError::Invalid` on the key ("conflicts with a `JwksFetcher` bean; remove one of them"). Boot does not choose silently between your explicit bean and your explicit key; build the TLS configuration into your own fetcher with `ReqwestJwksFetcher::with_tls` instead.
- The client is built once, when the decoder is built, and does not follow a reload of the bundle. Restart the application to use new JWKS certificates.
- `oxbloom-security` does not depend on `oxbloom-ssl`; boot resolves the name ([ADR 0074](../adr/0074-jwks-ssl-bundle-resolved-by-boot.md)).

## Testing

Test certificates are generated at test time, for example with `rcgen`; never commit key material. `TestApp` starts an application with a real server on port 0, and the feature `tls` of `oxbloom-test` adds a client that speaks HTTPS and trusts only the CA you give it:

```rust
use oxbloom_boot::SslAutoConfiguration;

let key = rcgen::KeyPair::generate()?;
let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])?.self_signed(&key)?;
let app = TestApp::web()
    .auto_configuration::<SslAutoConfiguration>()
    .config_str(format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n\
         [server.ssl]\nbundle = \"web\"\n",
        cert.pem(),
        key.serialize_pem()
    ))
    .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    .start()
    .await;
let client = app.client_tls(&cert.pem());   // a self-signed certificate is its own CA
assert_eq!(client.get("/ping").send().await.text(), "pong");
drop(client);
app.stop().await;
```

- `TestClient::connect_tls(addr, ca_pem)` connects to any HTTPS server; `RunningApp::client_tls(ca_pem)` connects to the running application. `app.client()` cannot reach an HTTPS port.
- Every connection sends and verifies the server name `localhost`, so a test certificate must carry the DNS name `localhost` (an IP address alone does not work).
- A `ca_pem` without a usable certificate panics at once with a fixed message; a failed handshake panics at `send` with the method, the path and the rustls cause, never with the PEM text.
- Use a short `ssl.reload-interval` (for example `50ms`) to test a certificate rotation, and a new client per check: a client keeps its connection, and a connection keeps the certificate of its handshake.
- A `FixedClock` or `ManualClock` bean drives the validity warnings.
- Giving `oxbloom-test` the feature `tls` turns on boot's `ssl` in your test builds (Cargo feature unification); see [Features and dev-dependencies](../API.md#features-and-dev-dependencies).

## Limits

- No mutual TLS and no `client-auth` on the server.
- No HTTP to HTTPS redirect and no ALPN `h2` (HTTP/2).
- The JWKS client does not hot reload.
- No OCSP stapling and no revocation checks.
- Reload polls, so a change applies within `ssl.reload-interval`.
- Expired certificates only warn.
- JKS keystores and encrypted PEM private keys are not supported.
- There is no limit on concurrent connections or half-open handshakes beyond the 10 second handshake timeout, the same exposure as plain HTTP.
- Bundles are used by the web server and the JWKS fetcher only; database and HTTP clients come later.
