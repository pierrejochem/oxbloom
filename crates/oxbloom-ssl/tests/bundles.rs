//! Loading SSL bundles from configuration: PEM files, inline PEM, PKCS#12 and every error kind.

mod common;

use std::path::Path;

use common::{ca, handshake, pkcs12_certificates_only, write, write_pkcs12};
use oxbloom_config::{Config, ConfigError};
use oxbloom_ssl::{SslBundles, SslError};

fn toml_path(path: &Path) -> String {
    path.display().to_string()
}

fn load(toml: &str) -> Result<SslBundles, SslError> {
    SslBundles::from_config(&Config::from_str(toml).expect("valid TOML"))
}

#[test]
fn pem_bundle_from_files_has_identity_and_trust() {
    let dir = tempfile::tempdir().unwrap();
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    let cert = write(dir.path(), "web.crt", &leaf.chain_pem);
    let key = write(dir.path(), "web.key", &leaf.key_pem);
    let trust = write(dir.path(), "ca.pem", &ca.cert_pem);
    let bundles = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = \"{}\"\nprivate-key = \"{}\"\ntrust = \"{}\"",
        toml_path(&cert),
        toml_path(&key),
        toml_path(&trust)
    ))
    .unwrap();

    assert_eq!(bundles.names(), ["web"]);
    let web = bundles.get("web").unwrap();
    assert_eq!(web.name(), "web");
    assert!(web.has_identity());
    assert!(web.has_trust());

    let client = handshake(
        web.server_config().unwrap(),
        web.client_config().unwrap(),
        "localhost",
    )
    .unwrap();
    assert_eq!(client.peer_certificates().unwrap().len(), 2);
}

#[test]
fn inline_pem_is_detected_by_its_prefix() {
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    let bundles = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n\
         [ssl.bundle.pem.client]\ntrust = '''\n{}'''",
        leaf.cert_pem, leaf.key_pem, ca.cert_pem
    ))
    .unwrap();

    let web = bundles.get("web").unwrap();
    assert!(web.has_identity());
    assert!(!web.has_trust());
    let client = bundles.get("client").unwrap();
    assert!(!client.has_identity());
    assert!(client.has_trust());

    handshake(
        web.server_config().unwrap(),
        client.client_config().unwrap(),
        "localhost",
    )
    .unwrap();
}

#[test]
fn pkcs12_bundle_loads_identity_and_ca_certificates() {
    let dir = tempfile::tempdir().unwrap();
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    let p12 = write_pkcs12(dir.path(), "legacy.p12", &leaf, &ca, "changeit");
    let bundles = load(&format!(
        "[ssl.bundle.pkcs12.legacy]\npath = \"{}\"\npassword = \"changeit\"",
        toml_path(&p12)
    ))
    .unwrap();

    let legacy = bundles.get("legacy").unwrap();
    assert!(legacy.has_identity());
    assert!(legacy.has_trust());
    handshake(
        legacy.server_config().unwrap(),
        legacy.client_config().unwrap(),
        "localhost",
    )
    .unwrap();
}

#[test]
fn unusable_trust_certificate_in_a_pkcs12_file_is_a_pkcs12_error() {
    let dir = tempfile::tempdir().unwrap();
    let ca = ca();
    // Trailing bytes after the certificate: the PKCS#12 reader keeps it, rustls cannot use it
    // as a trust anchor.
    let mut damaged = ca.cert_der.clone();
    damaged.extend_from_slice(&[0x05, 0x00]);
    let p12 = write(
        dir.path(),
        "trust.p12",
        pkcs12_certificates_only(&[&damaged], "changeit"),
    );
    let err = load(&format!(
        "[ssl.bundle.pkcs12.trust]\npath = \"{}\"\npassword = \"changeit\"",
        toml_path(&p12)
    ))
    .unwrap_err();
    assert!(
        matches!(&err, SslError::Pkcs12 { bundle } if bundle == "trust"),
        "{err:?}"
    );
}

#[test]
fn trust_only_bundle_has_no_identity_and_server_config_fails_with_no_identity() {
    let ca = ca();
    let bundles = load(&format!(
        "[ssl.bundle.pem.client]\ntrust = '''\n{}'''",
        ca.cert_pem
    ))
    .unwrap();

    let client = bundles.get("client").unwrap();
    assert!(!client.has_identity());
    assert!(client.client_config().is_ok());
    let err = client.server_config().unwrap_err();
    assert!(matches!(&err, SslError::NoIdentity { bundle } if bundle == "client"));
}

#[test]
fn certificate_without_private_key_is_an_error() {
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    let err = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''",
        leaf.cert_pem
    ))
    .unwrap_err();
    assert!(
        matches!(&err, SslError::Config(ConfigError::Missing { key }) if key == "ssl.bundle.pem.web.private-key"),
        "{err:?}"
    );

    let err = load(&format!(
        "[ssl.bundle.pem.web]\nprivate-key = '''\n{}'''",
        leaf.key_pem
    ))
    .unwrap_err();
    assert!(
        matches!(&err, SslError::Config(ConfigError::Missing { key }) if key == "ssl.bundle.pem.web.certificate"),
        "{err:?}"
    );
}

#[test]
fn key_that_does_not_match_the_certificate_is_key_mismatch() {
    let ca = ca();
    let first = ca.issue(&["localhost"]);
    let second = ca.issue(&["localhost"]);
    let err = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''",
        first.cert_pem, second.key_pem
    ))
    .unwrap_err();
    assert!(matches!(&err, SslError::KeyMismatch { bundle } if bundle == "web"));
}

#[test]
fn empty_file_is_a_pem_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let leaf = ca().issue(&["localhost"]);
    let cert = write(dir.path(), "empty.crt", "");
    let key = write(dir.path(), "web.key", &leaf.key_pem);
    let err = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = \"{}\"\nprivate-key = \"{}\"",
        toml_path(&cert),
        toml_path(&key)
    ))
    .unwrap_err();
    assert!(
        matches!(&err, SslError::Pem { bundle, key } if bundle == "web" && key == "ssl.bundle.pem.web.certificate"),
        "{err:?}"
    );
}

#[test]
fn missing_file_is_an_io_error_that_names_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let ca = ca();
    let missing = dir.path().join("missing.pem");
    let err = load(&format!(
        "[ssl.bundle.pem.client]\ntrust = \"{}\"",
        toml_path(&missing)
    ))
    .unwrap_err();
    assert!(
        matches!(&err, SslError::Io { bundle, key, kind }
            if bundle == "client" && key == "ssl.bundle.pem.client.trust" && *kind == std::io::ErrorKind::NotFound),
        "{err:?}"
    );
    drop(ca);
}

#[test]
fn crlf_chain_of_three_certificates_loads() {
    let dir = tempfile::tempdir().unwrap();
    let root = ca();
    let intermediate = root.intermediate();
    let leaf = intermediate.issue(&["localhost"]);
    let chain = leaf.chain_pem.replace('\n', "\r\n");
    assert_eq!(chain.matches("BEGIN CERTIFICATE").count(), 3);
    let cert = write(dir.path(), "chain.crt", chain);
    let key = write(dir.path(), "web.key", leaf.key_pem.replace('\n', "\r\n"));
    let bundles = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = \"{}\"\nprivate-key = \"{}\"\n\
         [ssl.bundle.pem.client]\ntrust = '''\n{}'''",
        toml_path(&cert),
        toml_path(&key),
        root.cert_pem
    ))
    .unwrap();

    let client = handshake(
        bundles.get("web").unwrap().server_config().unwrap(),
        bundles.get("client").unwrap().client_config().unwrap(),
        "localhost",
    )
    .unwrap();
    assert_eq!(client.peer_certificates().unwrap().len(), 3);
}

#[test]
fn encrypted_private_key_is_a_pem_error() {
    let leaf = ca().issue(&["localhost"]);
    // The body is irrelevant: an encrypted PKCS#8 block is never decrypted.
    let body = leaf
        .key_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("\n");
    let encrypted = format!(
        "-----BEGIN ENCRYPTED PRIVATE KEY-----\n{body}\n-----END ENCRYPTED PRIVATE KEY-----\n"
    );
    let err = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''",
        leaf.cert_pem, encrypted
    ))
    .unwrap_err();
    assert!(
        matches!(&err, SslError::Pem { bundle, key } if bundle == "web" && key == "ssl.bundle.pem.web.private-key"),
        "{err:?}"
    );
}

#[test]
fn certificate_value_that_holds_only_a_private_key_is_a_pem_error() {
    let leaf = ca().issue(&["localhost"]);
    let err = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{0}'''\nprivate-key = '''\n{0}'''",
        leaf.key_pem
    ))
    .unwrap_err();
    assert!(
        matches!(&err, SslError::Pem { bundle, key } if bundle == "web" && key == "ssl.bundle.pem.web.certificate"),
        "{err:?}"
    );
}

#[test]
fn legacy_openssl_encrypted_key_is_a_pem_error() {
    let leaf = ca().issue(&["localhost"]);
    // The traditional OpenSSL format with encryption headers; it is never decrypted.
    let encrypted = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\n\
                     DEK-Info: AES-256-CBC,0123456789ABCDEF0123456789ABCDEF\n\n\
                     MIIBOgIBAAJBAKj34GkxFhD90vcNLYLInFEX6Ppy1tPf9Cnzj4p4WGeKLs1Pt8Qu\n\
                     -----END RSA PRIVATE KEY-----\n";
    let err = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''",
        leaf.cert_pem, encrypted
    ))
    .unwrap_err();
    assert!(
        matches!(&err, SslError::Pem { bundle, key } if bundle == "web" && key == "ssl.bundle.pem.web.private-key"),
        "{err:?}"
    );
}

#[test]
fn empty_bundle_is_an_error() {
    let err = load("[ssl.bundle.pem.web]\n").unwrap_err();
    assert!(
        matches!(&err, SslError::EmptyBundle { bundle } if bundle == "web"),
        "{err:?}"
    );
}

#[test]
fn unknown_key_under_a_bundle_is_a_config_error() {
    let err = load("[ssl.bundle.pem.web]\ncert = \"x\"\n").unwrap_err();
    assert!(
        matches!(&err, SslError::Config(ConfigError::Invalid { .. })),
        "{err:?}"
    );
    assert!(err.to_string().contains("certificate"), "{err}");
}

#[test]
fn unknown_key_under_ssl_is_a_config_error_that_lists_the_valid_keys() {
    let err = load("[ssl]\nreload-intervall = \"MARKER-VALUE-91\"\n").unwrap_err();
    assert!(
        matches!(&err, SslError::Config(ConfigError::Invalid { .. })),
        "{err:?}"
    );
    let text = err.to_string();
    assert!(text.contains("`ssl`"), "{text}");
    assert!(text.contains("`reload-intervall`"), "{text}");
    for valid in ["reload_interval", "expiry_warning", "bundle"] {
        assert!(text.contains(valid), "`{valid}` missing from: {text}");
    }
    assert!(!text.contains("MARKER-VALUE-91"), "value leaked: {text}");
}

#[test]
fn duplicate_name_across_pem_and_pkcs12_is_an_error() {
    let ca = ca();
    let err = load(&format!(
        "[ssl.bundle.pem.web]\ntrust = '''\n{}'''\n[ssl.bundle.pkcs12.web]\npath = \"web.p12\"",
        ca.cert_pem
    ))
    .unwrap_err();
    assert!(
        matches!(&err, SslError::DuplicateName { name } if name == "web"),
        "{err:?}"
    );
}

#[test]
fn unknown_bundle_lists_the_available_names() {
    let ca = ca();
    let bundles = load(&format!(
        "[ssl.bundle.pem.web]\ntrust = '''\n{0}'''\n[ssl.bundle.pem.api]\ntrust = '''\n{0}'''",
        ca.cert_pem
    ))
    .unwrap();

    assert_eq!(bundles.names(), ["api", "web"]);
    let err = bundles.get("nope").unwrap_err();
    assert!(
        matches!(&err, SslError::UnknownBundle { name, available } if name == "nope" && available == &["api", "web"]),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "unknown SSL bundle `nope`; available bundles: api, web"
    );
}

#[test]
fn no_ssl_keys_give_an_empty_set_with_default_intervals() {
    let bundles = load("").unwrap();
    assert!(bundles.names().is_empty());
    assert_eq!(
        bundles.reload_interval(),
        std::time::Duration::from_secs(30)
    );
    assert_eq!(
        bundles.expiry_warning(),
        std::time::Duration::from_secs(14 * 86_400)
    );
    let custom = load("[ssl]\nreload-interval = \"0s\"\nexpiry-warning = \"30d\"").unwrap();
    assert_eq!(custom.reload_interval(), std::time::Duration::ZERO);
    assert_eq!(
        custom.expiry_warning(),
        std::time::Duration::from_secs(30 * 86_400)
    );
}

#[test]
fn secrets_never_appear_in_errors_or_debug_output() {
    const PASSWORD: &str = "MARKER-PASSWORD-7c41";
    let dir = tempfile::tempdir().unwrap();
    let ca = ca();
    let first = ca.issue(&["localhost"]);
    let second = ca.issue(&["localhost"]);
    // A line from the middle of the private key PEM: text that must never leak.
    let key_marker = second.key_pem.lines().nth(1).unwrap().to_string();
    let p12 = write_pkcs12(dir.path(), "legacy.p12", &first, &ca, "other-password");
    let garbage = write(dir.path(), "garbage.p12", "not a PKCS#12 file");

    let errors = [
        // Key mismatch with the key inline.
        load(&format!(
            "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''",
            first.cert_pem, second.key_pem
        ))
        .unwrap_err(),
        // A private key without a certificate.
        load(&format!(
            "[ssl.bundle.pem.web]\nprivate-key = '''\n{}'''",
            second.key_pem
        ))
        .unwrap_err(),
        // A private key value that is not PEM, treated as a path.
        load(&format!(
            "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = \"{}\"",
            first.cert_pem, key_marker
        ))
        .unwrap_err(),
        // A wrong password.
        load(&format!(
            "[ssl.bundle.pkcs12.legacy]\npath = \"{}\"\npassword = \"{PASSWORD}\"",
            toml_path(&p12)
        ))
        .unwrap_err(),
        // Not a PKCS#12 file.
        load(&format!(
            "[ssl.bundle.pkcs12.legacy]\npath = \"{}\"\npassword = \"{PASSWORD}\"",
            toml_path(&garbage)
        ))
        .unwrap_err(),
        // A password of the wrong type.
        load("[ssl.bundle.pkcs12.legacy]\npath = \"x.p12\"\npassword = [\"MARKER-PASSWORD-7c41\"]")
            .unwrap_err(),
    ];
    assert!(matches!(errors[0], SslError::KeyMismatch { .. }));
    assert!(matches!(errors[3], SslError::Pkcs12 { .. }));
    assert!(matches!(errors[4], SslError::Pkcs12 { .. }));
    for err in &errors {
        for text in [err.to_string(), format!("{err:?}")] {
            assert!(!text.contains(PASSWORD), "password leaked: {text}");
            assert!(!text.contains(&key_marker), "key leaked: {text}");
        }
    }

    let p12_ok = write_pkcs12(dir.path(), "ok.p12", &second, &ca, PASSWORD);
    let bundles = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n\
         [ssl.bundle.pkcs12.legacy]\npath = \"{}\"\npassword = \"{PASSWORD}\"",
        second.cert_pem,
        second.key_pem,
        toml_path(&p12_ok)
    ))
    .unwrap();
    for text in [
        format!("{bundles:?}"),
        format!("{:?}", bundles.get("web").unwrap()),
        format!("{:?}", bundles.get("legacy").unwrap()),
    ] {
        assert!(!text.contains(PASSWORD), "password leaked: {text}");
        assert!(!text.contains(&key_marker), "key leaked: {text}");
    }
}

#[test]
fn server_config_offers_only_http_1_1_alpn() {
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    let bundles = load(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\ntrust = '''\n{}'''",
        leaf.cert_pem, leaf.key_pem, ca.cert_pem
    ))
    .unwrap();
    let web = bundles.get("web").unwrap();
    let server = web.server_config().unwrap();
    assert_eq!(server.alpn_protocols, [b"http/1.1".to_vec()]);

    let mut client = (*web.client_config().unwrap()).clone();
    client.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let conn = handshake(server, std::sync::Arc::new(client), "localhost").unwrap();
    assert_eq!(conn.alpn_protocol(), Some(&b"http/1.1"[..]));
}

#[test]
fn ssl_error_is_send_sync_and_static() {
    fn check<T: Send + Sync + 'static + std::error::Error>() {}
    check::<SslError>();
}
