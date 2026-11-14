//! The `ssl.*` keys, read from a [`Config`] into intervals and bundle sources.

use std::collections::BTreeMap;
use std::time::Duration;

use oxbloom_config::{Config, ConfigError, Secret};
use serde::Deserialize;

use crate::error::SslError;

/// The configuration keys that [`SslBundles::from_config`](crate::SslBundles::from_config)
/// reads, as `(key, type name, default, description)`. `<name>` stands for any bundle name.
/// Boot turns these into its property metadata.
///
/// # Examples
///
/// ```
/// use oxbloom_ssl::PROPERTIES;
///
/// let (key, _, default, _) = PROPERTIES[0];
/// assert_eq!((key, default), ("ssl.reload-interval", Some("30s")));
/// assert!(PROPERTIES.iter().any(|p| p.0 == "ssl.bundle.pem.<name>.private-key"));
/// ```
pub const PROPERTIES: &[(&str, &str, Option<&str>, &str)] = &[
    (
        "ssl.reload-interval",
        "Duration",
        Some("30s"),
        "How often the files of every bundle are checked for changes; `0s` turns the check off.",
    ),
    (
        "ssl.expiry-warning",
        "Duration",
        Some("14d"),
        "A certificate that expires within this time produces a warning.",
    ),
    (
        "ssl.bundle.pem.<name>.certificate",
        "String",
        None,
        "The certificate chain of the bundle's identity, leaf first: a file path, or inline PEM \
         when the value starts with `-----BEGIN`. Requires `private-key`.",
    ),
    (
        "ssl.bundle.pem.<name>.private-key",
        "Secret<String>",
        None,
        "The unencrypted private key of the bundle's identity (PKCS#8, PKCS#1 or SEC1): a file \
         path, or inline PEM when the value starts with `-----BEGIN`. Requires `certificate`.",
    ),
    (
        "ssl.bundle.pem.<name>.trust",
        "String",
        None,
        "The CA certificates the bundle trusts: a file path, or inline PEM when the value starts \
         with `-----BEGIN`.",
    ),
    (
        "ssl.bundle.pkcs12.<name>.path",
        "String",
        None,
        "The PKCS#12 file of the bundle. Its key and certificate chain are the identity, and \
         every other certificate it holds is the trust store.",
    ),
    (
        "ssl.bundle.pkcs12.<name>.password",
        "Secret<String>",
        Some(""),
        "The password of the PKCS#12 file.",
    ),
];

const DEFAULT_RELOAD_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_EXPIRY_WARNING: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// The `ssl` subtree as written; every part is optional.
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawSsl {
    reload_interval: Duration,
    expiry_warning: Duration,
    bundle: RawBundles,
}

impl Default for RawSsl {
    fn default() -> Self {
        RawSsl {
            reload_interval: DEFAULT_RELOAD_INTERVAL,
            expiry_warning: DEFAULT_EXPIRY_WARNING,
            bundle: RawBundles::default(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawBundles {
    pem: BTreeMap<String, RawPem>,
    pkcs12: BTreeMap<String, RawPkcs12>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawPem {
    certificate: Option<String>,
    private_key: Option<Secret<String>>,
    trust: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPkcs12 {
    path: String,
    #[serde(default)]
    password: Option<Secret<String>>,
}

/// Where one bundle's material comes from.
#[derive(Clone, Debug)]
pub(crate) enum Source {
    /// `ssl.bundle.pem.<name>`. Certificate and key are both set or both unset.
    Pem {
        certificate: Option<String>,
        private_key: Option<Secret<String>>,
        trust: Option<String>,
    },
    /// `ssl.bundle.pkcs12.<name>`.
    Pkcs12 {
        path: String,
        password: Secret<String>,
    },
}

/// One configured bundle: its name as written and its source.
#[derive(Clone, Debug)]
pub(crate) struct BundleSource {
    pub(crate) name: String,
    pub(crate) source: Source,
}

impl BundleSource {
    /// The full configuration key of `leaf` in this bundle, such as
    /// `ssl.bundle.pem.web.certificate`.
    pub(crate) fn key(&self, leaf: &str) -> String {
        let kind = match self.source {
            Source::Pem { .. } => "pem",
            Source::Pkcs12 { .. } => "pkcs12",
        };
        format!("ssl.bundle.{kind}.{}.{leaf}", self.name)
    }
}

/// The SSL configuration: the intervals and every bundle, PEM bundles first, each kind in
/// name order.
#[derive(Debug)]
pub(crate) struct Settings {
    pub(crate) reload_interval: Duration,
    pub(crate) expiry_warning: Duration,
    pub(crate) bundles: Vec<BundleSource>,
}

impl Settings {
    /// Reads `ssl.*` from `config`. An absent `ssl` table gives the defaults and no bundles.
    pub(crate) fn from_config(config: &Config) -> Result<Settings, SslError> {
        let raw = match config.get::<RawSsl>("ssl") {
            Ok(raw) => raw,
            Err(ConfigError::Missing { key }) if key == "ssl" => RawSsl::default(),
            Err(e) => return Err(e.into()),
        };

        for name in raw.bundle.pem.keys() {
            if raw
                .bundle
                .pkcs12
                .keys()
                .any(|other| canonical(other) == canonical(name))
            {
                return Err(SslError::DuplicateName { name: name.clone() });
            }
        }

        let mut bundles = Vec::new();
        for (name, pem) in raw.bundle.pem {
            let missing = match (&pem.certificate, &pem.private_key) {
                (Some(_), None) => Some("private-key"),
                (None, Some(_)) => Some("certificate"),
                _ => None,
            };
            if let Some(leaf) = missing {
                return Err(ConfigError::Missing {
                    key: format!("ssl.bundle.pem.{name}.{leaf}"),
                }
                .into());
            }
            bundles.push(BundleSource {
                name,
                source: Source::Pem {
                    certificate: pem.certificate,
                    private_key: pem.private_key,
                    trust: pem.trust,
                },
            });
        }
        for (name, p12) in raw.bundle.pkcs12 {
            bundles.push(BundleSource {
                name,
                source: Source::Pkcs12 {
                    path: p12.path,
                    password: p12.password.unwrap_or_else(|| Secret::new(String::new())),
                },
            });
        }

        Ok(Settings {
            reload_interval: raw.reload_interval,
            expiry_warning: raw.expiry_warning,
            bundles,
        })
    }
}

/// The form in which Config matches keys: lowercase, without `-` and `_`.
pub(crate) fn canonical(key: &str) -> String {
    key.chars()
        .filter(|c| *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_ignores_case_dashes_and_underscores() {
        assert_eq!(canonical("My-Web_Bundle"), "mywebbundle");
    }

    #[test]
    fn pkcs12_password_defaults_to_empty() {
        let config = Config::from_str("[ssl.bundle.pkcs12.legacy]\npath = \"a.p12\"").unwrap();
        let settings = Settings::from_config(&config).unwrap();
        match &settings.bundles[0].source {
            Source::Pkcs12 { password, .. } => assert_eq!(password.expose(), ""),
            other => panic!("unexpected source {other:?}"),
        }
        assert_eq!(
            settings.bundles[0].key("path"),
            "ssl.bundle.pkcs12.legacy.path"
        );
    }

    #[test]
    fn every_property_key_starts_with_ssl() {
        assert!(PROPERTIES.iter().all(|p| p.0.starts_with("ssl.")));
        assert_eq!(PROPERTIES.len(), 7);
    }
}
