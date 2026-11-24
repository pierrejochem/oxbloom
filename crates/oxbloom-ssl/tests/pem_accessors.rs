//! The PEM accessors of a bundle: trust anchors and identity re-encoded from the loaded DER,
//! for bundles from PEM and from PKCS#12.

mod common;

use common::{ca, pkcs12_with_key, write};
use oxbloom_config::Config;
use oxbloom_ssl::{SslBundle, SslBundles};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

fn bundle(toml: &str) -> SslBundle {
    let config = Config::from_str(toml).expect("valid TOML");
    let name = toml
        .lines()
        .next()
        .and_then(|line| line.rsplit('.').next())
        .map(|name| name.trim_end_matches(']').to_string())
        .expect("a bundle header");
    SslBundles::from_config(&config)
        .expect("bundles")
        .get(&name)
        .expect("bundle")
}

fn certs(pem: &[u8]) -> Vec<Vec<u8>> {
    CertificateDer::pem_slice_iter(pem)
        .map(|cert| cert.expect("a certificate block").as_ref().to_vec())
        .collect()
}

#[test]
fn a_pem_bundle_round_trips_trust_pem() {
    let ca = ca();
    let other = common::ca();
    let both = format!("{}{}", ca.cert_pem, other.cert_pem);
    let client = bundle(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{both}'''"));
    let pem = client.trust_pem().expect("trust pem");
    assert_eq!(certs(&pem), [ca.cert_der.clone(), other.cert_der.clone()]);
    assert!(
        String::from_utf8(pem)
            .unwrap()
            .starts_with("-----BEGIN CERTIFICATE-----")
    );
}

#[test]
fn identity_pem_round_trips_certificate_chain_and_key() {
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    let web = bundle(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''",
        leaf.cert_pem, leaf.key_pem
    ));
    let (chain, key) = web.identity_pem().expect("identity pem");
    assert_eq!(certs(&chain), [leaf.cert_der.as_slice()]);
    let key_text = String::from_utf8(key.clone()).unwrap();
    assert!(key_text.starts_with("-----BEGIN PRIVATE KEY-----"));
    let parsed = PrivateKeyDer::from_pem_slice(&key).expect("a key");
    assert!(matches!(parsed, PrivateKeyDer::Pkcs8(_)));
    assert_eq!(parsed.secret_der(), leaf.key_der.as_slice());
}

#[test]
fn multiple_certificates_in_the_chain_are_all_emitted() {
    let root = ca();
    let intermediate = root.intermediate();
    let leaf = intermediate.issue(&["localhost"]);
    let web = bundle(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''",
        leaf.chain_pem, leaf.key_pem
    ));
    let (chain, _) = web.identity_pem().unwrap();
    assert_eq!(
        certs(&chain),
        [
            leaf.cert_der.clone(),
            intermediate.cert_der.clone(),
            root.cert_der.clone()
        ]
    );
}

#[test]
fn a_pkcs12_bundle_yields_identity_pem() {
    let dir = tempfile::tempdir().unwrap();
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    let p12 = write(
        dir.path(),
        "web.p12",
        pkcs12_with_key(&leaf, &[&ca.cert_der], "changeit"),
    );
    let web = bundle(&format!(
        "[ssl.bundle.pkcs12.web]\npath = \"{}\"\npassword = \"changeit\"",
        p12.display()
    ));
    let (chain, key) = web.identity_pem().expect("identity pem");
    assert_eq!(certs(&chain), [leaf.cert_der.clone(), ca.cert_der.clone()]);
    let parsed = PrivateKeyDer::from_pem_slice(&key).unwrap();
    assert_eq!(parsed.secret_der(), leaf.key_der.as_slice());
    // The remaining certificate of the file is the trust store.
    assert!(web.has_trust());
    assert!(web.trust_pem().is_some());
}

#[test]
fn a_bundle_without_trust_has_no_trust_pem_and_without_identity_no_identity_pem() {
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    let web = bundle(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''",
        leaf.cert_pem, leaf.key_pem
    ));
    assert!(!web.has_trust());
    assert!(web.trust_pem().is_none());
    let client = bundle(&format!(
        "[ssl.bundle.pem.client]\ntrust = '''\n{}'''",
        ca.cert_pem
    ));
    assert!(!client.has_identity());
    assert!(client.identity_pem().is_none());
}

#[test]
fn a_non_pkcs8_key_keeps_its_own_pem_label() {
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    // A SEC1 key (`EC PRIVATE KEY`), as `openssl ecparam -genkey` writes it: rcgen's PKCS#8
    // key wraps it, so unwrap the inner OCTET STRING by converting through openssl-free DER.
    let sec1 = sec1_from_pkcs8(&leaf.key_der);
    let sec1_pem =
        pem_rfc7468::encode_string("EC PRIVATE KEY", pem_rfc7468::LineEnding::LF, &sec1).unwrap();
    let web = bundle(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{sec1_pem}'''",
        leaf.cert_pem
    ));
    let (_, key) = web.identity_pem().expect("identity pem");
    assert!(
        String::from_utf8(key.clone())
            .unwrap()
            .starts_with("-----BEGIN EC PRIVATE KEY-----")
    );
    let parsed = PrivateKeyDer::from_pem_slice(&key).unwrap();
    assert!(matches!(parsed, PrivateKeyDer::Sec1(_)));
    assert_eq!(parsed.secret_der(), sec1.as_slice());
}

/// The SEC1 `ECPrivateKey` inside a PKCS#8 `PrivateKeyInfo` of an EC key: the content of the
/// OCTET STRING that follows the algorithm identifier.
fn sec1_from_pkcs8(pkcs8: &[u8]) -> Vec<u8> {
    use x509_parser::der_parser::ber::{BerObject, parse_ber};
    let (_, info) = parse_ber(pkcs8).expect("pkcs8");
    let items = info.as_sequence().expect("sequence");
    let octets: &BerObject = &items[2];
    octets.as_slice().expect("octet string").to_vec()
}

#[test]
fn debug_of_the_bundle_never_contains_pem_or_key_bytes() {
    let ca = ca();
    let leaf = ca.issue(&["localhost"]);
    let web = bundle(&format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n\
         [ssl.bundle.pem.client]\ntrust = '''\n{}'''",
        leaf.cert_pem, leaf.key_pem, ca.cert_pem
    ));
    let (chain, key) = web.identity_pem().unwrap();
    let debug = format!("{web:?}");
    assert_eq!(
        debug,
        "SslBundle { name: \"web\", identity: true, trust: false }"
    );
    for secret in [&chain, &key] {
        let text = String::from_utf8(secret.clone()).unwrap();
        let body = text.lines().nth(1).unwrap();
        assert!(!debug.contains(body));
    }
    assert!(!debug.contains("BEGIN"));
}
