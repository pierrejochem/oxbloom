//! Password hashing: the [`PasswordEncoder`] trait, an Argon2id encoder and a delegating encoder
//! that prefixes each stored hash with the id of the algorithm that made it.
//!
//! Stored hashes look like `{argon2}$argon2id$v=19$m=19456,t=2,p=1$...`. The prefix lets an
//! application change algorithms or cost parameters later while old hashes keep verifying;
//! [`PasswordEncoder::upgrade_encoding`] reports which stored hashes should be re-hashed.
//!
//! No type here prints a password or a hash, in `Debug` output or in error text.
//!
//! # Examples
//!
//! ```
//! use oxbloom_security::{Argon2PasswordEncoder, DelegatingPasswordEncoder, PasswordEncoder};
//!
//! let encoder = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1)?);
//! let stored = encoder.encode("correct horse");
//! assert!(stored.starts_with("{argon2}"));
//! assert!(encoder.matches("correct horse", &stored));
//! assert!(!encoder.matches("wrong", &stored));
//! # Ok::<(), oxbloom_security::PasswordEncoderError>(())
//! ```

use std::fmt;
use std::sync::Arc;

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};

/// The id under which [`DelegatingPasswordEncoder::argon2`] registers the Argon2id encoder.
///
/// # Examples
///
/// ```
/// use oxbloom_security::ARGON2_ID;
///
/// assert_eq!(ARGON2_ID, "argon2");
/// ```
pub const ARGON2_ID: &str = "argon2";

const DEFAULT_MEMORY_KIB: u32 = 19456;
const DEFAULT_ITERATIONS: u32 = 2;
const DEFAULT_PARALLELISM: u32 = 1;

/// Hashes passwords and checks a password against a stored hash.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Argon2PasswordEncoder, PasswordEncoder};
///
/// let encoder = Argon2PasswordEncoder::new(8, 1, 1)?;
/// let hash = encoder.encode("secret");
/// assert!(encoder.matches("secret", &hash));
/// assert!(!encoder.upgrade_encoding(&hash));
/// # Ok::<(), oxbloom_security::PasswordEncoderError>(())
/// ```
pub trait PasswordEncoder: Send + Sync {
    /// Hashes `raw` with a fresh random salt.
    fn encode(&self, raw: &str) -> String;

    /// Returns true when `raw` produced `encoded`. A malformed `encoded` never matches.
    fn matches(&self, raw: &str, encoded: &str) -> bool;

    /// Returns true when `encoded` was made with weaker settings than this encoder uses, or
    /// cannot be read, so the caller should re-hash the password after a successful login.
    fn upgrade_encoding(&self, encoded: &str) -> bool;
}

/// A password encoder could not be built.
///
/// The messages never include a supplied value.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Argon2PasswordEncoder, PasswordEncoderError};
///
/// let err = Argon2PasswordEncoder::new(0, 1, 1).unwrap_err();
/// assert_eq!(err, PasswordEncoderError::InvalidParameters);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordEncoderError {
    /// The Argon2 memory, iteration or lane count is out of range.
    InvalidParameters,
    /// The default id is not among the registered encoders.
    UnknownDefaultId,
    /// Two encoders were registered under the same id.
    DuplicateId,
}

impl fmt::Display for PasswordEncoderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidParameters => "invalid Argon2 parameters",
            Self::UnknownDefaultId => "the default encoder id is not registered",
            Self::DuplicateId => "an encoder id is registered twice",
        })
    }
}

impl std::error::Error for PasswordEncoderError {}

/// An Argon2id [`PasswordEncoder`] that stores PHC strings (`$argon2id$v=19$...`).
///
/// The default is the OWASP minimum: 19 MiB of memory, 2 iterations and 1 lane.
///
/// # Examples
///
/// ```
/// use oxbloom_security::Argon2PasswordEncoder;
///
/// let default = Argon2PasswordEncoder::default();
/// assert_eq!((default.memory_kib(), default.iterations(), default.parallelism()), (19456, 2, 1));
///
/// let cheap = Argon2PasswordEncoder::new(8, 1, 1)?;
/// assert_eq!(cheap.memory_kib(), 8);
/// # Ok::<(), oxbloom_security::PasswordEncoderError>(())
/// ```
#[derive(Clone)]
pub struct Argon2PasswordEncoder {
    params: Params,
}

impl Argon2PasswordEncoder {
    /// Builds an encoder with the given memory (in KiB), iteration and lane counts.
    ///
    /// # Errors
    ///
    /// [`PasswordEncoderError::InvalidParameters`] when Argon2 rejects the values, for example
    /// zero iterations or less than 8 KiB per lane.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Argon2PasswordEncoder, PasswordEncoderError};
    ///
    /// assert!(Argon2PasswordEncoder::new(19456, 2, 1).is_ok());
    /// assert_eq!(
    ///     Argon2PasswordEncoder::new(8, 0, 1).unwrap_err(),
    ///     PasswordEncoderError::InvalidParameters
    /// );
    /// ```
    pub fn new(
        memory_kib: u32,
        iterations: u32,
        parallelism: u32,
    ) -> Result<Self, PasswordEncoderError> {
        let params = Params::new(memory_kib, iterations, parallelism, None)
            .map_err(|_| PasswordEncoderError::InvalidParameters)?;
        Ok(Self { params })
    }

    /// The memory cost in KiB.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Argon2PasswordEncoder;
    ///
    /// assert_eq!(Argon2PasswordEncoder::new(16, 1, 1)?.memory_kib(), 16);
    /// # Ok::<(), oxbloom_security::PasswordEncoderError>(())
    /// ```
    pub fn memory_kib(&self) -> u32 {
        self.params.m_cost()
    }

    /// The iteration (time) cost.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Argon2PasswordEncoder;
    ///
    /// assert_eq!(Argon2PasswordEncoder::new(8, 3, 1)?.iterations(), 3);
    /// # Ok::<(), oxbloom_security::PasswordEncoderError>(())
    /// ```
    pub fn iterations(&self) -> u32 {
        self.params.t_cost()
    }

    /// The number of lanes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Argon2PasswordEncoder;
    ///
    /// assert_eq!(Argon2PasswordEncoder::new(16, 1, 2)?.parallelism(), 2);
    /// # Ok::<(), oxbloom_security::PasswordEncoderError>(())
    /// ```
    pub fn parallelism(&self) -> u32 {
        self.params.p_cost()
    }

    fn hasher(&self) -> Argon2<'_> {
        Argon2::new(Algorithm::Argon2id, Version::V0x13, self.params.clone())
    }
}

impl Default for Argon2PasswordEncoder {
    fn default() -> Self {
        Self::new(DEFAULT_MEMORY_KIB, DEFAULT_ITERATIONS, DEFAULT_PARALLELISM)
            .expect("the OWASP Argon2 defaults are valid")
    }
}

impl fmt::Debug for Argon2PasswordEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Argon2PasswordEncoder")
            .field("memory_kib", &self.memory_kib())
            .field("iterations", &self.iterations())
            .field("parallelism", &self.parallelism())
            .finish()
    }
}

impl PasswordEncoder for Argon2PasswordEncoder {
    fn encode(&self, raw: &str) -> String {
        self.hasher()
            .hash_password(raw.as_bytes())
            .expect("Argon2 hashing with validated parameters and the OS RNG cannot fail")
            .to_string()
    }

    fn matches(&self, raw: &str, encoded: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(encoded) else {
            return false;
        };
        self.hasher()
            .verify_password(raw.as_bytes(), &parsed)
            .is_ok()
    }

    fn upgrade_encoding(&self, encoded: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(encoded) else {
            return true;
        };
        let Ok(stored) = Params::try_from(&parsed) else {
            return true;
        };
        stored.m_cost() < self.memory_kib()
            || stored.t_cost() < self.iterations()
            || stored.p_cost() < self.parallelism()
    }
}

/// A [`PasswordEncoder`] that stores `{id}hash` and verifies with the encoder the id names.
///
/// New hashes use the default id. Hashes with an unknown or missing prefix never match and
/// always report that they need upgrading.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_security::{Argon2PasswordEncoder, DelegatingPasswordEncoder, PasswordEncoder};
///
/// let argon2 = Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?);
/// let encoder = DelegatingPasswordEncoder::new("argon2", vec![("argon2".to_owned(), argon2)])?;
/// let stored = encoder.encode("pw");
/// assert!(encoder.matches("pw", &stored));
/// assert!(!encoder.matches("pw", "{unknown}whatever"));
/// # Ok::<(), oxbloom_security::PasswordEncoderError>(())
/// ```
pub struct DelegatingPasswordEncoder {
    default_id: String,
    encoders: Vec<(String, Arc<dyn PasswordEncoder>)>,
}

impl DelegatingPasswordEncoder {
    /// Builds an encoder from `(id, encoder)` pairs; new hashes use `default_id`.
    ///
    /// # Errors
    ///
    /// [`PasswordEncoderError::DuplicateId`] when an id repeats, and
    /// [`PasswordEncoderError::UnknownDefaultId`] when `default_id` is not registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{DelegatingPasswordEncoder, PasswordEncoderError};
    ///
    /// let err = DelegatingPasswordEncoder::new("missing", vec![]).unwrap_err();
    /// assert_eq!(err, PasswordEncoderError::UnknownDefaultId);
    /// ```
    pub fn new(
        default_id: &str,
        encoders: Vec<(String, Arc<dyn PasswordEncoder>)>,
    ) -> Result<Self, PasswordEncoderError> {
        for (i, (id, _)) in encoders.iter().enumerate() {
            if encoders[..i].iter().any(|(other, _)| other == id) {
                return Err(PasswordEncoderError::DuplicateId);
            }
        }
        if !encoders.iter().any(|(id, _)| id == default_id) {
            return Err(PasswordEncoderError::UnknownDefaultId);
        }
        Ok(Self {
            default_id: default_id.to_owned(),
            encoders,
        })
    }

    /// Builds an encoder with `encoder` registered as [`ARGON2_ID`] and as the default.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Argon2PasswordEncoder, DelegatingPasswordEncoder, PasswordEncoder};
    ///
    /// let encoder = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1)?);
    /// assert!(encoder.encode("pw").starts_with("{argon2}"));
    /// # Ok::<(), oxbloom_security::PasswordEncoderError>(())
    /// ```
    pub fn argon2(encoder: Argon2PasswordEncoder) -> Self {
        Self {
            default_id: ARGON2_ID.to_owned(),
            encoders: vec![(ARGON2_ID.to_owned(), Arc::new(encoder))],
        }
    }

    fn find(&self, id: &str) -> Option<&Arc<dyn PasswordEncoder>> {
        self.encoders
            .iter()
            .find(|(known, _)| known == id)
            .map(|(_, e)| e)
    }

    /// Splits `{id}rest` into its id and the rest.
    fn split(encoded: &str) -> Option<(&str, &str)> {
        let inner = encoded.strip_prefix('{')?;
        inner.split_once('}')
    }
}

impl fmt::Debug for DelegatingPasswordEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ids: Vec<&str> = self.encoders.iter().map(|(id, _)| id.as_str()).collect();
        f.debug_struct("DelegatingPasswordEncoder")
            .field("default_id", &self.default_id)
            .field("ids", &ids)
            .finish()
    }
}

impl PasswordEncoder for DelegatingPasswordEncoder {
    fn encode(&self, raw: &str) -> String {
        let encoder = self
            .find(&self.default_id)
            .expect("the default id is registered");
        format!("{{{}}}{}", self.default_id, encoder.encode(raw))
    }

    fn matches(&self, raw: &str, encoded: &str) -> bool {
        match Self::split(encoded).and_then(|(id, rest)| Some((self.find(id)?, rest))) {
            Some((encoder, rest)) => encoder.matches(raw, rest),
            None => false,
        }
    }

    fn upgrade_encoding(&self, encoded: &str) -> bool {
        match Self::split(encoded) {
            Some((id, rest)) if id == self.default_id => self
                .find(id)
                .is_none_or(|encoder| encoder.upgrade_encoding(rest)),
            _ => true,
        }
    }
}
