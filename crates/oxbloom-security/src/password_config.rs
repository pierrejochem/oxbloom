//! Readers for the password keys: `security.password`, `security.http-basic` and
//! `security.users`.
//!
//! Each reader reads only its own section, like the `security.jwt` reader, so the sections do
//! not need to know each other's key names. Inside a section unknown keys are rejected.

use std::collections::BTreeMap;

use argon2::Params;
use argon2::password_hash::phc::PasswordHash;
use oxbloom_config::{Config, ConfigError, Secret};
use serde::Deserialize;

use crate::{ARGON2_ID, Argon2PasswordEncoder, DelegatingPasswordEncoder, UserDetails};

const PASSWORD: &str = "security.password";
const ARGON2: &str = "security.password.argon2";
const DEFAULT_ENCODER: &str = "security.password.default-encoder";
const MEMORY_KIB: &str = "security.password.argon2.memory-kib";
const ITERATIONS: &str = "security.password.argon2.iterations";
const PARALLELISM: &str = "security.password.argon2.parallelism";
const HTTP_BASIC: &str = "security.http-basic";
const REALM: &str = "security.http-basic.realm";
const USERS: &str = "security.users";

const MEMORY_RANGE: (u32, u32) = (8, 1_048_576);
const ITERATIONS_RANGE: (u32, u32) = (1, 100);
const PARALLELISM_RANGE: (u32, u32) = (1, 64);

/// The realm of the Basic challenge when `security.http-basic.realm` is not set.
const DEFAULT_REALM: &str = "Oxbloom";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawPassword {
    #[serde(rename = "default-encoder")]
    default_encoder: Option<String>,
    argon2: RawArgon2,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawArgon2 {
    #[serde(rename = "memory-kib")]
    memory_kib: Option<u32>,
    iterations: Option<u32>,
    parallelism: Option<u32>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawHttpBasic {
    enabled: Option<bool>,
    realm: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUser {
    password: Secret<String>,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    locked: Option<bool>,
}

/// Reads the section `key` as `T`; a missing section gives `T::default()`.
fn section<T: Default + for<'de> Deserialize<'de>>(
    config: &Config,
    key: &str,
) -> Result<T, ConfigError> {
    match config.get::<T>(key) {
        Ok(raw) => Ok(raw),
        Err(ConfigError::Missing { key: missing }) if missing == key => Ok(T::default()),
        Err(e) => Err(e),
    }
}

/// `value` when it lies in `range`, else an error on `key` that gives the range, not the value.
fn bounded(
    key: &str,
    value: Option<u32>,
    default: u32,
    range: (u32, u32),
) -> Result<u32, ConfigError> {
    let value = value.unwrap_or(default);
    if value < range.0 || value > range.1 {
        return Err(ConfigError::invalid(
            key,
            format!("must be between {} and {}", range.0, range.1),
        ));
    }
    Ok(value)
}

/// The Argon2id cost settings of `security.password.argon2`.
///
/// The defaults are the OWASP minimum, the same as [`Argon2PasswordEncoder::default`]: 19 456
/// KiB of memory, 2 iterations and 1 lane.
///
/// # Examples
///
/// ```
/// use oxbloom_security::Argon2Settings;
///
/// let settings = Argon2Settings::default();
/// assert_eq!((settings.memory_kib, settings.iterations, settings.parallelism), (19456, 2, 1));
/// let encoder = settings.encoder()?;
/// assert_eq!(encoder.memory_kib(), 19456);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Argon2Settings {
    /// The memory cost in KiB, `security.password.argon2.memory-kib`.
    pub memory_kib: u32,
    /// The number of passes, `security.password.argon2.iterations`.
    pub iterations: u32,
    /// The number of lanes, `security.password.argon2.parallelism`.
    pub parallelism: u32,
}

impl Default for Argon2Settings {
    fn default() -> Self {
        let encoder = Argon2PasswordEncoder::default();
        Argon2Settings {
            memory_kib: encoder.memory_kib(),
            iterations: encoder.iterations(),
            parallelism: encoder.parallelism(),
        }
    }
}

impl Argon2Settings {
    /// Reads the `security.password.argon2` keys of `config`.
    ///
    /// The keys are `memory-kib` (8 to 1 048 576, default 19 456), `iterations` (1 to 100,
    /// default 2) and `parallelism` (1 to 64, default 1). Argon2 also needs at least 8 KiB of
    /// memory per lane. A missing section gives [`Argon2Settings::default`].
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] naming the key for an unknown key in the section, a value of the wrong
    /// type, a value outside its range (the message gives the range) and a `memory-kib` below
    /// 8 times `parallelism`. The error never shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_security::Argon2Settings;
    ///
    /// let config = Config::from_str("[security.password.argon2]\nmemory-kib = 65536\niterations = 3")?;
    /// let settings = Argon2Settings::from_config(&config)?;
    /// assert_eq!((settings.memory_kib, settings.iterations, settings.parallelism), (65536, 3, 1));
    ///
    /// let config = Config::from_str("[security.password.argon2]\niterations = 101")?;
    /// let err = Argon2Settings::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("security.password.argon2.iterations"));
    /// assert!(err.to_string().contains("between 1 and 100"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<Argon2Settings, ConfigError> {
        Argon2Settings::from_raw(section::<RawArgon2>(config, ARGON2)?)
    }

    fn from_raw(raw: RawArgon2) -> Result<Argon2Settings, ConfigError> {
        let defaults = Argon2Settings::default();
        let settings = Argon2Settings {
            memory_kib: bounded(
                MEMORY_KIB,
                raw.memory_kib,
                defaults.memory_kib,
                MEMORY_RANGE,
            )?,
            iterations: bounded(
                ITERATIONS,
                raw.iterations,
                defaults.iterations,
                ITERATIONS_RANGE,
            )?,
            parallelism: bounded(
                PARALLELISM,
                raw.parallelism,
                defaults.parallelism,
                PARALLELISM_RANGE,
            )?,
        };
        settings.encoder()?;
        Ok(settings)
    }

    /// Builds the [`Argon2PasswordEncoder`] of these settings.
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] on `security.password.argon2.memory-kib` when Argon2 rejects the
    /// values, for example less than 8 KiB of memory per lane.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Argon2Settings;
    ///
    /// let cheap = Argon2Settings { memory_kib: 8, iterations: 1, parallelism: 1 };
    /// assert_eq!(cheap.encoder()?.iterations(), 1);
    ///
    /// let starved = Argon2Settings { memory_kib: 8, iterations: 1, parallelism: 2 };
    /// assert!(starved.encoder().is_err());
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn encoder(&self) -> Result<Argon2PasswordEncoder, ConfigError> {
        Argon2PasswordEncoder::new(self.memory_kib, self.iterations, self.parallelism).map_err(
            |_| {
                ConfigError::invalid(
                    MEMORY_KIB,
                    "must be at least 8 times security.password.argon2.parallelism, within the \
                     Argon2 limits",
                )
            },
        )
    }
}

/// The `security.password` settings: the default encoder id and the Argon2 costs.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{PasswordEncoder, PasswordSettings};
///
/// let settings = PasswordSettings::default();
/// assert_eq!(settings.default_encoder, "argon2");
/// let encoder = settings.encoder()?;
/// assert!(encoder.encode("pw").starts_with("{argon2}$argon2id$"));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordSettings {
    /// The id new hashes are stored with, `security.password.default-encoder`. Only `argon2`
    /// exists in this version.
    pub default_encoder: String,
    /// The Argon2 costs, `security.password.argon2.*`.
    pub argon2: Argon2Settings,
}

impl Default for PasswordSettings {
    fn default() -> Self {
        PasswordSettings {
            default_encoder: ARGON2_ID.to_owned(),
            argon2: Argon2Settings::default(),
        }
    }
}

impl PasswordSettings {
    /// Reads the `security.password` keys of `config`.
    ///
    /// `default-encoder` accepts only `argon2` (the default). The `argon2` subsection is read
    /// as described in [`Argon2Settings::from_config`]. A missing section gives
    /// [`PasswordSettings::default`].
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] naming the key for an unknown key under `security.password` or
    /// `security.password.argon2`, any `default-encoder` other than `argon2`, and every error
    /// of [`Argon2Settings::from_config`]. The error never shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_security::PasswordSettings;
    ///
    /// let config = Config::from_str("[security.password]\ndefault-encoder = \"argon2\"\n[security.password.argon2]\nparallelism = 2")?;
    /// let settings = PasswordSettings::from_config(&config)?;
    /// assert_eq!(settings.argon2.parallelism, 2);
    ///
    /// let config = Config::from_str("[security.password]\ndefault-encoder = \"md5\"")?;
    /// let err = PasswordSettings::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("security.password.default-encoder"));
    /// assert!(!err.to_string().contains("md5"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<PasswordSettings, ConfigError> {
        let raw = section::<RawPassword>(config, PASSWORD)?;
        let default_encoder = match raw.default_encoder {
            None => ARGON2_ID.to_owned(),
            Some(id) if id == ARGON2_ID => id,
            Some(_) => return Err(ConfigError::invalid(DEFAULT_ENCODER, "must be `argon2`")),
        };
        Ok(PasswordSettings {
            default_encoder,
            argon2: Argon2Settings::from_raw(raw.argon2)?,
        })
    }

    /// Builds the [`DelegatingPasswordEncoder`] of these settings: new hashes are stored as
    /// `{argon2}` followed by the Argon2id PHC string.
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] on `security.password.default-encoder` for an id other than `argon2`,
    /// and the errors of [`Argon2Settings::encoder`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Argon2Settings, PasswordEncoder, PasswordSettings};
    ///
    /// let settings = PasswordSettings {
    ///     default_encoder: "argon2".to_owned(),
    ///     argon2: Argon2Settings { memory_kib: 8, iterations: 1, parallelism: 1 },
    /// };
    /// let encoder = settings.encoder()?;
    /// let stored = encoder.encode("pw");
    /// assert!(encoder.matches("pw", &stored));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn encoder(&self) -> Result<DelegatingPasswordEncoder, ConfigError> {
        if self.default_encoder != ARGON2_ID {
            return Err(ConfigError::invalid(DEFAULT_ENCODER, "must be `argon2`"));
        }
        Ok(DelegatingPasswordEncoder::argon2(self.argon2.encoder()?))
    }
}

/// The `security.http-basic` settings.
///
/// Basic is off by default (`enabled = false`): users in `security.users` do not open Basic by
/// themselves. The default realm is `Oxbloom`.
///
/// # Examples
///
/// ```
/// use oxbloom_security::HttpBasicSettings;
///
/// let settings = HttpBasicSettings::default();
/// assert!(!settings.enabled);
/// assert_eq!(settings.realm, "Oxbloom");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpBasicSettings {
    /// Whether the chain accepts `Authorization: Basic`, `security.http-basic.enabled`.
    pub enabled: bool,
    /// The realm of the `WWW-Authenticate` challenge, `security.http-basic.realm`.
    pub realm: String,
}

impl Default for HttpBasicSettings {
    fn default() -> Self {
        HttpBasicSettings {
            enabled: false,
            realm: DEFAULT_REALM.to_owned(),
        }
    }
}

impl HttpBasicSettings {
    /// Reads the `security.http-basic` keys of `config`: `enabled` (default `false`) and
    /// `realm` (default `Oxbloom`). A missing section gives [`HttpBasicSettings::default`].
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] naming the key for an unknown key in the section, a value of the
    /// wrong type and a realm with a double quote, a backslash or a control character. The
    /// error never shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_security::HttpBasicSettings;
    ///
    /// let config = Config::from_str("[security.http-basic]\nenabled = true\nrealm = \"Admin\"")?;
    /// let settings = HttpBasicSettings::from_config(&config)?;
    /// assert!(settings.enabled);
    /// assert_eq!(settings.realm, "Admin");
    ///
    /// let config = Config::from_str("[security.http-basic]\nenable = true")?;
    /// let err = HttpBasicSettings::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("security.http-basic"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<HttpBasicSettings, ConfigError> {
        let raw = section::<RawHttpBasic>(config, HTTP_BASIC)?;
        let realm = raw.realm.unwrap_or_else(|| DEFAULT_REALM.to_owned());
        if realm
            .chars()
            .any(|c| c == '"' || c == '\\' || c.is_control())
        {
            return Err(ConfigError::invalid(
                REALM,
                "must not contain a double quote, a backslash or a control character",
            ));
        }
        Ok(HttpBasicSettings {
            enabled: raw.enabled.unwrap_or(false),
            realm,
        })
    }
}

/// Reads the users of `security.users.<name>`.
///
/// Each user has `password` (required), `roles` (a list of role names without the `ROLE_`
/// prefix, default empty), `enabled` (default `true`) and `locked` (default `false`). The
/// password is the stored hash: `{argon2}` followed by an Argon2 PHC string, as
/// `DelegatingPasswordEncoder::encode` returns it. Only its form is checked here; nothing is
/// hashed. The user name is the key as written and is compared exactly at login. The
/// configuration matches keys ignoring case, `-` and `_`, so two names that differ only in
/// those are the same key and fail to load as a duplicate key. A name with dots needs quotes
/// in TOML (`[security.users."ops.bot"]`). Environment variables give lowercase names. A
/// missing section gives no users.
///
/// Users alone do not open HTTP Basic: that needs `security.http-basic.enabled = true`.
///
/// # Errors
///
/// A [`ConfigError`] naming the key for an unknown key under a user, a missing or mistyped
/// value, an empty user name, an empty role, and a password without the `{argon2}` prefix or
/// with a bad PHC string (on `security.users.<name>.password`). The error never shows the
/// password, the hash or any other configured value.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::{Argon2PasswordEncoder, DelegatingPasswordEncoder, PasswordEncoder, users_from_config};
///
/// let encoder = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1)?);
/// let hash = encoder.encode("s3cret");
/// let toml = format!("[security.users.alice]\npassword = \"{hash}\"\nroles = [\"ADMIN\"]");
/// let users = users_from_config(&Config::from_str(&toml)?)?;
/// assert_eq!(users[0].username(), "alice");
/// assert!(encoder.matches("s3cret", users[0].password()));
/// assert_eq!(users[0].authorities()[0].as_str(), "ROLE_ADMIN");
///
/// // A plain-text password is refused, naming the key and not the value.
/// let config = Config::from_str("[security.users.bob]\npassword = \"hunter2\"")?;
/// let err = users_from_config(&config).unwrap_err().to_string();
/// assert!(err.contains("security.users.bob.password"));
/// assert!(!err.contains("hunter2"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn users_from_config(config: &Config) -> Result<Vec<UserDetails>, ConfigError> {
    let raw = section::<BTreeMap<String, RawUser>>(config, USERS)?;
    let mut users = Vec::with_capacity(raw.len());
    for (name, user) in raw {
        if name.is_empty() {
            return Err(ConfigError::invalid(USERS, "user names must not be empty"));
        }
        check_hash(&name, user.password.expose())?;
        if user.roles.iter().any(|r| r.trim().is_empty()) {
            return Err(ConfigError::invalid(
                format!("{USERS}.{name}.roles"),
                "roles must not be empty",
            ));
        }
        let roles: Vec<&str> = user.roles.iter().map(String::as_str).collect();
        users.push(
            UserDetails::new(name.as_str(), user.password.expose().as_str(), [])
                .roles(&roles)
                .enabled(user.enabled.unwrap_or(true))
                .locked(user.locked.unwrap_or(false)),
        );
    }
    Ok(users)
}

/// Checks that `stored` is `{argon2}` followed by an Argon2 PHC string (`argon2id`, `argon2i` or
/// `argon2d`) whose cost lies in the configured ranges, without hashing and without echoing it.
fn check_hash(name: &str, stored: &str) -> Result<(), ConfigError> {
    let valid = stored
        .strip_prefix("{argon2}")
        .and_then(|phc| PasswordHash::new(phc).ok())
        .is_some_and(|parsed| {
            matches!(
                parsed.algorithm.as_str(),
                "argon2id" | "argon2i" | "argon2d"
            ) && Params::try_from(&parsed).is_ok_and(|p| {
                in_range(p.m_cost(), MEMORY_RANGE)
                    && in_range(p.t_cost(), ITERATIONS_RANGE)
                    && in_range(p.p_cost(), PARALLELISM_RANGE)
            })
        });
    if valid {
        Ok(())
    } else {
        Err(ConfigError::invalid(
            format!("{USERS}.{name}.password"),
            "must be `{argon2}` followed by an Argon2 PHC string (argon2id, argon2i or argon2d, \
             with memory, iterations and lanes in the configured ranges), as the password \
             encoder stores it; plain-text passwords are not accepted",
        ))
    }
}

fn in_range(value: u32, range: (u32, u32)) -> bool {
    value >= range.0 && value <= range.1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PasswordEncoder;

    fn cfg(toml: &str) -> Config {
        Config::from_str(toml).unwrap()
    }

    fn invalid_key(err: ConfigError) -> (String, String) {
        match err {
            ConfigError::Invalid { key, message, .. } => (key, message),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    fn hash(raw: &str) -> String {
        DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1).unwrap()).encode(raw)
    }

    #[test]
    fn defaults_without_sections() {
        let config = cfg("");
        assert_eq!(
            PasswordSettings::from_config(&config).unwrap(),
            PasswordSettings::default()
        );
        assert_eq!(
            Argon2Settings::from_config(&config).unwrap(),
            Argon2Settings {
                memory_kib: 19456,
                iterations: 2,
                parallelism: 1
            }
        );
        let basic = HttpBasicSettings::from_config(&config).unwrap();
        assert!(!basic.enabled);
        assert_eq!(basic.realm, "Oxbloom");
        assert!(users_from_config(&config).unwrap().is_empty());
        assert_eq!(PasswordSettings::default().default_encoder, "argon2");
    }

    #[test]
    fn every_password_key_is_read() {
        let config = cfg("[security.password]\ndefault-encoder = \"argon2\"\n\
             [security.password.argon2]\nmemory-kib = 4096\niterations = 3\nparallelism = 4\n");
        let settings = PasswordSettings::from_config(&config).unwrap();
        assert_eq!(settings.default_encoder, "argon2");
        assert_eq!(
            settings.argon2,
            Argon2Settings {
                memory_kib: 4096,
                iterations: 3,
                parallelism: 4
            }
        );
        assert_eq!(
            Argon2Settings::from_config(&config).unwrap(),
            settings.argon2
        );
        let encoder = settings.argon2.encoder().unwrap();
        assert_eq!(
            (
                encoder.memory_kib(),
                encoder.iterations(),
                encoder.parallelism()
            ),
            (4096, 3, 4)
        );
    }

    #[test]
    fn every_http_basic_key_is_read() {
        let config = cfg("[security.http-basic]\nenabled = true\nrealm = \"Admin area\"\n");
        assert_eq!(
            HttpBasicSettings::from_config(&config).unwrap(),
            HttpBasicSettings {
                enabled: true,
                realm: "Admin area".to_owned()
            }
        );
    }

    fn argon2_with(key: &str, value: u32) -> Result<Argon2Settings, ConfigError> {
        // Enough memory for 64 lanes, so only the bound of `key` is tested.
        let base = if key == "memory-kib" {
            String::new()
        } else {
            "memory-kib = 1024\n".to_owned()
        };
        Argon2Settings::from_config(&cfg(&format!(
            "[security.password.argon2]\n{base}{key} = {value}\n"
        )))
    }

    #[test]
    fn bound_edges() {
        let cases = [
            (
                "memory-kib",
                8,
                1_048_576,
                MEMORY_KIB,
                "between 8 and 1048576",
            ),
            ("iterations", 1, 100, ITERATIONS, "between 1 and 100"),
            ("parallelism", 1, 64, PARALLELISM, "between 1 and 64"),
        ];
        for (field, low, high, key, range) in cases {
            assert!(argon2_with(field, low).is_ok(), "{field} = {low}");
            assert!(argon2_with(field, high).is_ok(), "{field} = {high}");
            for outside in [low - 1, high + 1] {
                let (k, message) = invalid_key(argon2_with(field, outside).unwrap_err());
                assert_eq!(k, key);
                assert!(message.contains(range), "{message}");
            }
            // The value is not echoed (the numbers are chosen to be absent from the range).
            let (_, message) = invalid_key(argon2_with(field, high + 1).unwrap_err());
            assert!(!message.contains(&(high + 1).to_string()), "{message}");
        }
    }

    #[test]
    fn memory_below_eight_per_lane_is_invalid() {
        let config = cfg("[security.password.argon2]\nmemory-kib = 8\nparallelism = 2\n");
        let (key, _) = invalid_key(Argon2Settings::from_config(&config).unwrap_err());
        assert_eq!(key, MEMORY_KIB);
    }

    #[test]
    fn default_encoder_other_than_argon2_is_invalid_without_echo() {
        for value in ["bcrypt", "ARGON2", "noop"] {
            let config = cfg(&format!(
                "[security.password]\ndefault-encoder = \"{value}\"\n"
            ));
            let err = PasswordSettings::from_config(&config).unwrap_err();
            let text = format!("{err} {err:?}");
            assert!(!text.contains(value), "{text}");
            assert_eq!(invalid_key(err).0, DEFAULT_ENCODER);
        }
    }

    #[test]
    fn unknown_keys_are_rejected_in_each_section() {
        let cases = [
            (
                "[security.password]\ndefault-encodr = \"argon2\"\n",
                PASSWORD,
            ),
            ("[security.password.argon2]\nmemory = 8\n", ARGON2),
            ("[security.http-basic]\nenable = true\n", HTTP_BASIC),
        ];
        for (toml, key) in cases {
            let config = cfg(toml);
            let err = match key {
                HTTP_BASIC => HttpBasicSettings::from_config(&config).unwrap_err(),
                _ => PasswordSettings::from_config(&config).unwrap_err(),
            };
            assert_eq!(invalid_key(err).0, key, "{toml}");
        }
        let toml = format!(
            "[security.users.alice]\npassword = \"{}\"\nrole = [\"A\"]\n",
            hash("pw")
        );
        let (key, message) = invalid_key(users_from_config(&cfg(&toml)).unwrap_err());
        assert_eq!(key, "security.users.alice");
        assert!(message.contains("role"), "{message}");
    }

    #[test]
    fn readers_ignore_the_other_security_sections() {
        let toml = format!(
            "[security.jwt]\nalgorithms = [\"HS256\"]\n\
             [security.password]\ndefault-encoder = \"argon2\"\n\
             [security.http-basic]\nenabled = true\n\
             [security.users.alice]\npassword = \"{}\"\n",
            hash("pw")
        );
        let config = cfg(&toml);
        assert!(PasswordSettings::from_config(&config).is_ok());
        assert!(HttpBasicSettings::from_config(&config).unwrap().enabled);
        assert_eq!(users_from_config(&config).unwrap().len(), 1);
    }

    #[test]
    fn every_user_key_is_read() {
        let stored = hash("pw");
        let toml = format!(
            "[security.users.alice]\npassword = \"{stored}\"\nroles = [\"ADMIN\", \"USER\"]\n\
             enabled = false\nlocked = true\n\
             [security.users.bob]\npassword = \"{stored}\"\n"
        );
        let users = users_from_config(&cfg(&toml)).unwrap();
        assert_eq!(users.len(), 2);
        let alice = &users[0];
        assert_eq!(alice.username(), "alice");
        assert_eq!(alice.password(), stored);
        let roles: Vec<_> = alice.authorities().iter().map(|a| a.as_str()).collect();
        assert_eq!(roles, ["ROLE_ADMIN", "ROLE_USER"]);
        assert!(!alice.is_enabled());
        assert!(alice.is_locked());
        let bob = &users[1];
        assert!(bob.is_enabled() && !bob.is_locked());
        assert!(bob.authorities().is_empty());
    }

    #[test]
    fn other_algorithms_and_hostile_costs_are_rejected_without_hashing() {
        let salt = "c2FsdHNhbHRzYWx0";
        let out = "aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g";
        let cases = [
            format!("{{argon2}}$pbkdf2-sha256$i=600000,l=32${salt}${out}"),
            format!("{{argon2}}$scrypt$ln=17,r=8,p=1${salt}${out}"),
            format!("{{argon2}}$argon2id$v=19$m=19456,t=4294967295,p=1${salt}${out}"),
            format!("{{argon2}}$argon2id$v=19$m=4294967295,t=2,p=1${salt}${out}"),
            format!("{{argon2}}$argon2id$v=19$m=19456,t=2,p=4294967295${salt}${out}"),
        ];
        for value in cases {
            let toml = format!("[security.users.dave]\npassword = \"{value}\"\n");
            let err = users_from_config(&cfg(&toml)).unwrap_err();
            let text = format!("{err} {err:?}");
            assert!(!text.contains(out), "{text}");
            assert!(!text.contains(salt), "{text}");
            assert_eq!(invalid_key(err).0, "security.users.dave.password");
        }
    }

    #[test]
    fn a_default_cost_hash_is_accepted() {
        let encoder = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::default());
        let stored = encoder.encode("pw");
        let toml = format!("[security.users.erin]\npassword = \"{stored}\"\n");
        assert_eq!(users_from_config(&cfg(&toml)).unwrap().len(), 1);
    }

    #[test]
    fn bad_passwords_are_rejected_without_echo() {
        let phc = hash("pw").strip_prefix("{argon2}").unwrap().to_owned();
        let cases = [
            "plain-text-hunter2".to_owned(),
            format!("{{bcrypt}}{phc}"),
            phc.clone(),
            "{argon2}not-a-phc-string".to_owned(),
            "{argon2}".to_owned(),
        ];
        for value in cases {
            let toml = format!("[security.users.carol]\npassword = \"{value}\"\n");
            let err = users_from_config(&cfg(&toml)).unwrap_err();
            let text = format!("{err} {err:?}");
            // The bare prefix is part of the rule the message states.
            if value != "{argon2}" {
                assert!(!text.contains(&value), "{text}");
            }
            assert!(!text.contains("hunter2"), "{text}");
            assert!(!text.contains(&phc), "{text}");
            assert_eq!(invalid_key(err).0, "security.users.carol.password");
        }
    }

    #[test]
    fn a_missing_password_names_the_key() {
        let err = users_from_config(&cfg("[security.users.dave]\nroles = [\"A\"]\n")).unwrap_err();
        assert!(
            err.to_string().contains("security.users.dave.password"),
            "{err}"
        );
    }

    #[test]
    fn empty_role_is_invalid() {
        let toml = format!(
            "[security.users.erin]\npassword = \"{}\"\nroles = [\"\"]\n",
            hash("pw")
        );
        let (key, _) = invalid_key(users_from_config(&cfg(&toml)).unwrap_err());
        assert_eq!(key, "security.users.erin.roles");
    }

    #[test]
    fn user_names_keep_their_spelling_and_dots() {
        let stored = hash("pw");
        let toml = format!(
            "[security.users.\"ops.bot\"]\npassword = \"{stored}\"\n\
             [security.users.Alice]\npassword = \"{stored}\"\n"
        );
        let users = users_from_config(&cfg(&toml)).unwrap();
        let mut names: Vec<_> = users.iter().map(UserDetails::username).collect();
        names.sort_unstable();
        assert_eq!(names, ["Alice", "ops.bot"]);
    }

    #[test]
    fn names_that_differ_only_in_case_are_one_configuration_key() {
        // The configuration matches keys ignoring case, so the pair cannot load as two users;
        // it is a duplicate key, never a silent merge.
        let stored = hash("pw");
        let toml = format!(
            "[security.users.Alice]\npassword = \"{stored}\"\n\
             [security.users.alice]\npassword = \"{stored}\"\n"
        );
        assert!(matches!(
            Config::from_str(&toml),
            Err(ConfigError::DuplicateKey { .. })
        ));
    }

    #[test]
    fn realm_with_a_quote_is_invalid() {
        let config = cfg("[security.http-basic]\nrealm = 'a\"b'\n");
        let (key, _) = invalid_key(HttpBasicSettings::from_config(&config).unwrap_err());
        assert_eq!(key, REALM);
    }

    #[test]
    fn wrong_types_name_the_key_without_echo() {
        let config = cfg("[security.password.argon2]\nmemory-kib = \"lots-of-memory\"\n");
        let err = PasswordSettings::from_config(&config).unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(text.contains(MEMORY_KIB), "{text}");
        assert!(!text.contains("lots-of-memory"), "{text}");
        let config = cfg("[security.http-basic]\nenabled = \"sometimes\"\n");
        let err = HttpBasicSettings::from_config(&config).unwrap_err();
        assert!(!err.to_string().contains("sometimes"), "{err}");
    }
}
