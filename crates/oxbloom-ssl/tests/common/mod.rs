//! Test certificates generated with `rcgen` at test time, and small file and handshake helpers.
//! No certificate or key file is committed to the repository.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, ServerConfig, ServerConnection};

/// A certificate authority that issues leaf and intermediate certificates.
pub struct TestCa {
    issuer: CertifiedIssuer<'static, KeyPair>,
    /// The PEM text of the authority's own certificate.
    pub cert_pem: String,
    /// The DER bytes of the authority's own certificate.
    pub cert_der: Vec<u8>,
    /// The PEM text of every certificate above this one, intermediate first, root last.
    parents_pem: String,
}

/// A leaf certificate with its private key.
pub struct Issued {
    /// The PEM text of the leaf certificate alone.
    pub cert_pem: String,
    /// The PEM text of the leaf's PKCS#8 private key.
    pub key_pem: String,
    /// The PEM text of the leaf certificate followed by every issuer up to the root.
    pub chain_pem: String,
    /// The DER bytes of the leaf certificate.
    pub cert_der: Vec<u8>,
    /// The DER bytes of the leaf's PKCS#8 private key.
    pub key_der: Vec<u8>,
}

fn ca_params(common_name: &str) -> CertificateParams {
    let mut params = CertificateParams::new(Vec::<String>::new()).expect("no names");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params
}

/// A new self-signed root authority.
pub fn ca() -> TestCa {
    let key = KeyPair::generate().expect("key generation");
    let issuer = CertifiedIssuer::self_signed(ca_params("Oxbloom Test Root"), key)
        .expect("self-signed root");
    TestCa {
        cert_pem: issuer.pem(),
        cert_der: issuer.der().to_vec(),
        issuer,
        parents_pem: String::new(),
    }
}

/// Sets the validity of `params` to run from `not_before` to `not_after`, cut to whole
/// seconds as a certificate stores them.
fn set_dates(params: &mut CertificateParams, not_before: SystemTime, not_after: SystemTime) {
    let seconds = |time: SystemTime| {
        let since = time
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("a test date after 1970");
        Duration::from_secs(since.as_secs())
    };
    params.not_before = rcgen::date_time_ymd(1970, 1, 1) + seconds(not_before);
    params.not_after = rcgen::date_time_ymd(1970, 1, 1) + seconds(not_after);
}

/// A new self-signed root authority named `common_name`, valid from `not_before` to
/// `not_after`.
pub fn ca_between(common_name: &str, not_before: SystemTime, not_after: SystemTime) -> TestCa {
    let key = KeyPair::generate().expect("key generation");
    let mut params = ca_params(common_name);
    set_dates(&mut params, not_before, not_after);
    let issuer = CertifiedIssuer::self_signed(params, key).expect("self-signed root");
    TestCa {
        cert_pem: issuer.pem(),
        cert_der: issuer.der().to_vec(),
        issuer,
        parents_pem: String::new(),
    }
}

impl TestCa {
    /// A new leaf certificate for the DNS `names`, signed by this authority and valid from
    /// `not_before` to `not_after`.
    pub fn issue_between(
        &self,
        names: &[&str],
        not_before: SystemTime,
        not_after: SystemTime,
    ) -> Issued {
        let key = KeyPair::generate().expect("key generation");
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let mut params = CertificateParams::new(names.clone()).expect("names");
        params.distinguished_name.push(
            DnType::CommonName,
            names.first().cloned().unwrap_or_default(),
        );
        set_dates(&mut params, not_before, not_after);
        let cert = params.signed_by(&key, &self.issuer).expect("leaf");
        Issued {
            chain_pem: format!("{}{}{}", cert.pem(), self.cert_pem, self.parents_pem),
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
            cert_der: cert.der().to_vec(),
            key_der: key.serialize_der(),
        }
    }

    /// A new intermediate authority signed by this one.
    pub fn intermediate(&self) -> TestCa {
        let key = KeyPair::generate().expect("key generation");
        let issuer =
            CertifiedIssuer::signed_by(ca_params("Oxbloom Test Intermediate"), key, &self.issuer)
                .expect("intermediate");
        TestCa {
            cert_pem: issuer.pem(),
            cert_der: issuer.der().to_vec(),
            issuer,
            parents_pem: format!("{}{}", self.cert_pem, self.parents_pem),
        }
    }

    /// A new leaf certificate for the DNS `names`, signed by this authority.
    pub fn issue(&self, names: &[&str]) -> Issued {
        let key = KeyPair::generate().expect("key generation");
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let mut params = CertificateParams::new(names.clone()).expect("names");
        params.distinguished_name.push(
            DnType::CommonName,
            names.first().cloned().unwrap_or_default(),
        );
        let cert = params.signed_by(&key, &self.issuer).expect("leaf");
        Issued {
            chain_pem: format!("{}{}{}", cert.pem(), self.cert_pem, self.parents_pem),
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
            cert_der: cert.der().to_vec(),
            key_der: key.serialize_der(),
        }
    }
}

/// Writes `contents` to `dir/name` and returns the path.
pub fn write(dir: &Path, name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write test file");
    path
}

/// Writes a PKCS#12 file holding `issued` with `ca` in its chain, encrypted with `password`.
pub fn write_pkcs12(
    dir: &Path,
    name: &str,
    issued: &Issued,
    ca: &TestCa,
    password: &str,
) -> PathBuf {
    write(
        dir,
        name,
        pkcs12_with_key(issued, &[&ca.cert_der], password),
    )
}

/// The bytes of a PKCS#12 file holding the key of `issued` with a chain of its certificate
/// followed by `chain`, encrypted with `password`. Without `chain` the file has no
/// certificate besides the leaf, so a bundle loaded from it has no trust store.
pub fn pkcs12_with_key(issued: &Issued, chain: &[&[u8]], password: &str) -> Vec<u8> {
    use p12_keystore::{Certificate, KeyStore, KeyStoreEntry, PrivateKey, PrivateKeyChain};

    let key = PrivateKey::from_der(&issued.key_der).expect("pkcs8 key");
    let leaf = Certificate::from_der(&issued.cert_der).expect("leaf certificate");
    let others = chain
        .iter()
        .map(|der| Certificate::from_der(der).expect("chain certificate"));
    let mut store = KeyStore::new();
    store.add_entry(
        "server",
        KeyStoreEntry::PrivateKeyChain(PrivateKeyChain::new(
            b"server-key".to_vec(),
            key,
            std::iter::once(leaf).chain(others),
        )),
    );
    store.writer(password).write().expect("pkcs12 encoding")
}

/// The bytes of a PKCS#12 file holding only the certificates `certs`, without a key bag,
/// encrypted with `password`: a certificate-only export.
pub fn pkcs12_certificates_only(certs: &[&[u8]], password: &str) -> Vec<u8> {
    use p12_keystore::{Certificate, KeyStore, KeyStoreEntry};

    let mut store = KeyStore::new();
    for (i, der) in certs.iter().enumerate() {
        let cert = Certificate::from_der(der).expect("certificate");
        store.add_entry(&format!("cert-{i}"), KeyStoreEntry::Certificate(cert));
    }
    store.writer(password).write().expect("pkcs12 encoding")
}

/// Runs a TLS handshake in memory and returns the finished client connection.
pub fn handshake(
    server: Arc<ServerConfig>,
    client: Arc<ClientConfig>,
    name: &str,
) -> Result<ClientConnection, rustls::Error> {
    let server_name = ServerName::try_from(name.to_string()).expect("server name");
    let mut client = ClientConnection::new(client, server_name)?;
    let mut server = ServerConnection::new(server)?;
    for _ in 0..20 {
        let mut buf = Vec::new();
        client.write_tls(&mut buf).expect("client write");
        let mut slice = &buf[..];
        while !slice.is_empty() {
            server.read_tls(&mut slice).expect("server read");
            server.process_new_packets()?;
        }
        let mut buf = Vec::new();
        server.write_tls(&mut buf).expect("server write");
        let mut slice = &buf[..];
        while !slice.is_empty() {
            client.read_tls(&mut slice).expect("client read");
            client.process_new_packets()?;
        }
        if !client.is_handshaking() && !server.is_handshaking() {
            return Ok(client);
        }
    }
    panic!("the handshake did not finish");
}
