use std::fmt;

use serde::{Deserialize, Deserializer};

/// A value that never appears in `Debug` or `Display` output.
///
/// Use it for passwords, tokens and keys in config structs. It deserializes like `T`.
///
/// # Examples
///
/// ```
/// use oxbloom_config::{Config, Secret};
/// use serde::Deserialize;
///
/// #[derive(Deserialize, Debug)]
/// struct Db { user: String, password: Secret<String> }
///
/// let config = Config::from_str("[db]\nuser = \"app\"\npassword = \"hunter2\"")?;
/// let db: Db = config.get("db")?;
/// assert_eq!(format!("{db:?}"), "Db { user: \"app\", password: [REDACTED] }");
/// assert_eq!(db.password.expose(), "hunter2");
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wraps `value`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    ///
    /// let token = Secret::new("abc123".to_string());
    /// assert_eq!(token.to_string(), "[REDACTED]");
    /// ```
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Returns the wrapped value. Call it only where the secret is actually used.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    ///
    /// let token = Secret::new("abc123");
    /// assert_eq!(*token.expose(), "abc123");
    /// ```
    pub fn expose(&self) -> &T {
        &self.0
    }
}

/// Always prints `[REDACTED]`.
///
/// ```
/// use oxbloom_config::Secret;
///
/// assert_eq!(format!("{:?}", Secret::new("hunter2")), "[REDACTED]");
/// ```
impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Always prints `[REDACTED]`.
///
/// ```
/// use oxbloom_config::Secret;
///
/// assert_eq!(Secret::new("hunter2").to_string(), "[REDACTED]");
/// ```
impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Deserializes the inner value; the wrapper adds nothing to the format.
///
/// ```
/// use oxbloom_config::{Config, Secret};
///
/// let config = Config::from_str("key = \"abc\"")?;
/// let key: Secret<String> = config.get("key")?;
/// assert_eq!(key.expose(), "abc");
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_is_redacted() {
        assert_eq!(format!("{:?}", Secret::new("hunter2")), "[REDACTED]");
    }

    #[test]
    fn secret_display_is_redacted() {
        assert_eq!(Secret::new("hunter2").to_string(), "[REDACTED]");
    }

    #[test]
    fn secret_expose_returns_value() {
        assert_eq!(*Secret::new("hunter2").expose(), "hunter2");
    }

    #[test]
    fn secret_deserializes_from_string() {
        let s: Secret<String> = toml::Value::String("hunter2".into()).try_into().unwrap();
        assert_eq!(s.expose(), "hunter2");
    }
}
