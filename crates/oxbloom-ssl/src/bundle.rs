//! Loading the material of one bundle: PEM values and files, and PKCS#12 files.

use std::sync::{Arc, OnceLock};

use p12_keystore::Pkcs12Archive;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use rustls::{InconsistentKeys, RootCertStore};
use zeroize::{Zeroize, Zeroizing};

use crate::error::SslError;
use crate::settings::{BundleSource, Source};
use crate::validity::{self, CertValidity, Warnings};

/// The loaded material of a bundle: an optional identity and an optional trust store, at
/// least one of them present, with the validity of every certificate and the warnings given
/// about them. It has no `Debug`, so the key can never be printed, and dropping it wipes the
/// retained key DER.
pub(crate) struct Material {
    pub(crate) identity: Option<Arc<CertifiedKey>>,
    pub(crate) trust: Option<Arc<RootCertStore>>,
    /// The DER of the trust certificates, kept for the PEM accessors: a root store keeps only
    /// the parsed anchors.
    pub(crate) trust_certs: Vec<CertificateDer<'static>>,
    /// The private key of the identity, kept for the PEM accessors: a certified key keeps only
    /// a signing key.
    pub(crate) key: Option<PrivateKeyDer<'static>>,
    /// The identity chain, leaf first, then the trust certificates, each certificate once.
    pub(crate) validity: Vec<CertValidity>,
    pub(crate) warned: Warnings,
}

impl Material {
    /// The material of `identity` and `trust`, where `trust_certs` are the certificates of
    /// the trust store.
    fn new(
        identity: Option<(Arc<CertifiedKey>, PrivateKeyDer<'static>)>,
        trust: Option<Arc<RootCertStore>>,
        trust_certs: &[CertificateDer<'static>],
    ) -> Material {
        let (identity, key) = match identity {
            Some((identity, key)) => (Some(identity), Some(key)),
            None => (None, None),
        };
        let chain = identity.iter().flat_map(|key| key.cert.iter());
        let validity = validity::of_certificates(chain.chain(trust_certs));
        let warned = Warnings::new(validity.len());
        Material {
            identity,
            trust,
            trust_certs: trust_certs.to_vec(),
            key,
            validity,
            warned,
        }
    }
}

impl Drop for Material {
    /// Wipes the retained private key DER, so it does not linger in freed memory after a
    /// reload replaced the material or the bundle was dropped.
    fn drop(&mut self) {
        if let Some(key) = &mut self.key {
            key.zeroize();
        }
    }
}

/// The only crypto provider Oxbloom uses for TLS: `aws-lc-rs`.
pub(crate) fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .clone()
}

/// Loads the material `bundle` describes, reading every file it names.
pub(crate) fn load(bundle: &BundleSource) -> Result<Material, SslError> {
    let material = match &bundle.source {
        Source::Pem {
            certificate,
            private_key,
            trust,
        } => {
            let identity = match (certificate, private_key) {
                (Some(cert), Some(key)) => {
                    let chain = certificates(bundle, "certificate", cert)?;
                    // The PEM text of the key is wiped when it goes out of scope.
                    let key_bytes =
                        Zeroizing::new(value_bytes(bundle, "private-key", key.expose())?);
                    let key =
                        PrivateKeyDer::from_pem_slice(&key_bytes).map_err(|_| SslError::Pem {
                            bundle: bundle.name.clone(),
                            key: bundle.key("private-key"),
                        })?;
                    let kept = key.clone_key();
                    Some((certified_key(bundle, chain, key)?, kept))
                }
                // Settings rejects one without the other.
                _ => None,
            };
            let trust_certs = match trust {
                Some(value) => certificates(bundle, "trust", value)?,
                None => Vec::new(),
            };
            let trust = if trust_certs.is_empty() {
                None
            } else {
                Some(root_store(bundle, "trust", trust_certs.clone())?)
            };
            Material::new(identity, trust, &trust_certs)
        }
        Source::Pkcs12 { path, password } => {
            let data = read_file(bundle, "path", path)?;
            pkcs12(bundle, &data, password.expose())?
        }
    };
    if material.identity.is_none() && material.trust.is_none() {
        return Err(SslError::EmptyBundle {
            bundle: bundle.name.clone(),
        });
    }
    Ok(material)
}

/// The files `bundle` reads, each with the leaf of the key that names it, in a fixed order.
/// Inline PEM values are not files, so they are not listed.
pub(crate) fn watched_files(bundle: &BundleSource) -> Vec<(&'static str, String)> {
    let mut files = Vec::new();
    match &bundle.source {
        Source::Pem {
            certificate,
            private_key,
            trust,
        } => {
            let values = [
                ("certificate", certificate.as_deref()),
                (
                    "private-key",
                    private_key.as_ref().map(|key| key.expose().as_str()),
                ),
                ("trust", trust.as_deref()),
            ];
            for (leaf, value) in values {
                if let Some(value) = value.filter(|v| !is_inline_pem(v)) {
                    files.push((leaf, value.to_string()));
                }
            }
        }
        Source::Pkcs12 { path, .. } => files.push(("path", path.clone())),
    }
    files
}

/// Whether `value` is inline PEM rather than a file path.
fn is_inline_pem(value: &str) -> bool {
    value.trim_start().starts_with("-----BEGIN")
}

/// The PEM bytes of `value`: the value itself when it is inline PEM, else the file it names.
fn value_bytes(bundle: &BundleSource, leaf: &str, value: &str) -> Result<Vec<u8>, SslError> {
    if is_inline_pem(value) {
        Ok(value.as_bytes().to_vec())
    } else {
        read_file(bundle, leaf, value)
    }
}

/// Reads the file at `path`. The error names the key, never the path.
fn read_file(bundle: &BundleSource, leaf: &str, path: &str) -> Result<Vec<u8>, SslError> {
    std::fs::read(path).map_err(|e| SslError::Io {
        bundle: bundle.name.clone(),
        key: bundle.key(leaf),
        kind: e.kind(),
    })
}

/// Every certificate in the PEM `value`; at least one.
fn certificates(
    bundle: &BundleSource,
    leaf: &str,
    value: &str,
) -> Result<Vec<CertificateDer<'static>>, SslError> {
    let bytes = value_bytes(bundle, leaf, value)?;
    let pem_error = || SslError::Pem {
        bundle: bundle.name.clone(),
        key: bundle.key(leaf),
    };
    let certs = CertificateDer::pem_slice_iter(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| pem_error())?;
    if certs.is_empty() {
        return Err(pem_error());
    }
    Ok(certs)
}

/// A root store of `certs`; every one must be a usable trust anchor.
fn root_store(
    bundle: &BundleSource,
    leaf: &str,
    certs: Vec<CertificateDer<'static>>,
) -> Result<Arc<RootCertStore>, SslError> {
    let mut store = RootCertStore::empty();
    let (_, ignored) = store.add_parsable_certificates(certs);
    if ignored > 0 || store.is_empty() {
        return Err(SslError::Pem {
            bundle: bundle.name.clone(),
            key: bundle.key(leaf),
        });
    }
    Ok(Arc::new(store))
}

/// Pairs `chain` with `key` and checks that the key belongs to the first certificate.
fn certified_key(
    bundle: &BundleSource,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<CertifiedKey>, SslError> {
    let (cert_leaf, key_leaf) = match bundle.source {
        Source::Pem { .. } => ("certificate", "private-key"),
        Source::Pkcs12 { .. } => ("path", "path"),
    };
    match CertifiedKey::from_der(chain, key, &provider()) {
        Ok(certified) => Ok(Arc::new(certified)),
        Err(rustls::Error::InconsistentKeys(InconsistentKeys::KeyMismatch)) => {
            Err(SslError::KeyMismatch {
                bundle: bundle.name.clone(),
            })
        }
        Err(rustls::Error::InvalidCertificate(_)) => Err(SslError::Pem {
            bundle: bundle.name.clone(),
            key: bundle.key(cert_leaf),
        }),
        // The provider cannot load the key: an unsupported algorithm or a damaged key.
        Err(_) => Err(SslError::Pem {
            bundle: bundle.name.clone(),
            key: bundle.key(key_leaf),
        }),
    }
}

/// Decodes a PKCS#12 file: its first key with the matching certificate chain is the identity,
/// and every other certificate is the trust store.
fn pkcs12(bundle: &BundleSource, data: &[u8], password: &str) -> Result<Material, SslError> {
    let pkcs12_error = || SslError::Pkcs12 {
        bundle: bundle.name.clone(),
    };
    let archive = Pkcs12Archive::from_pkcs12(data, password).map_err(|_| pkcs12_error())?;
    let certs: Vec<CertificateDer<'static>> = archive
        .certs
        .iter()
        .map(|bag| CertificateDer::from(bag.cert.as_der().to_vec()))
        .collect();

    let mut leaf_index = None;
    let identity = match archive.keys.first() {
        None => None,
        Some(key_bag) => {
            let key =
                || PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_bag.key.as_der().to_vec()));
            // The certificate with the key's local key id, else the first one the key matches.
            let by_id = key_bag.local_key_id.as_ref().and_then(|id| {
                archive
                    .certs
                    .iter()
                    .position(|bag| bag.local_key_id.as_deref() == Some(id.0.as_slice()))
            });
            let index = match by_id {
                Some(index) => index,
                None => (0..certs.len())
                    .find(|&i| {
                        CertifiedKey::from_der(vec![certs[i].clone()], key(), &provider()).is_ok()
                    })
                    .ok_or_else(|| {
                        if certs.is_empty() {
                            pkcs12_error()
                        } else {
                            SslError::KeyMismatch {
                                bundle: bundle.name.clone(),
                            }
                        }
                    })?,
            };
            leaf_index = Some(index);
            let chain = chain_from(&archive, index)
                .into_iter()
                .map(|i| certs[i].clone())
                .collect();
            Some((certified_key(bundle, chain, key())?, key()))
        }
    };

    let others: Vec<CertificateDer<'static>> = certs
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != leaf_index)
        .map(|(_, cert)| cert.clone())
        .collect();
    // A certificate the PKCS#12 reader accepted but rustls cannot use as a trust anchor is a
    // problem of the file, not of PEM data.
    let trust = if others.is_empty() {
        None
    } else {
        Some(root_store(bundle, "path", others.clone()).map_err(|_| pkcs12_error())?)
    };
    Ok(Material::new(identity, trust, &others))
}

/// The indices of the chain that starts at certificate `leaf`, each next certificate the
/// issuer of the previous one, ending at a self-signed certificate or a missing issuer.
fn chain_from(archive: &Pkcs12Archive, leaf: usize) -> Vec<usize> {
    let mut chain = vec![leaf];
    let mut current = leaf;
    loop {
        let cert = &archive.certs[current].cert;
        if cert.issuer() == cert.subject() {
            break;
        }
        let next = archive
            .certs
            .iter()
            .enumerate()
            .find(|(i, bag)| !chain.contains(i) && bag.cert.subject() == cert.issuer());
        match next {
            Some((i, _)) => {
                chain.push(i);
                current = i;
            }
            None => break,
        }
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_pem_is_recognized_after_leading_whitespace() {
        assert!(is_inline_pem("-----BEGIN CERTIFICATE-----"));
        assert!(is_inline_pem("\n  -----BEGIN CERTIFICATE-----"));
        assert!(!is_inline_pem("config/web.crt"));
    }

    #[test]
    fn inline_pem_values_are_not_watched() {
        use oxbloom_config::Secret;

        let bundle = BundleSource {
            name: "web".into(),
            source: Source::Pem {
                certificate: Some("web.crt".into()),
                private_key: Some(Secret::new("-----BEGIN PRIVATE KEY-----".into())),
                trust: Some("ca.pem".into()),
            },
        };
        let files = watched_files(&bundle);
        assert_eq!(
            files,
            [
                ("certificate", "web.crt".to_string()),
                ("trust", "ca.pem".to_string())
            ]
        );
    }
}
