//! The CSRF token type.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The length of a token: 32 bytes in base64url without padding.
const TOKEN_LEN: usize = 43;

/// A CSRF token: 32 random bytes in base64url without padding (43 characters).
///
/// There is no `Display`, and `Debug` prints a placeholder, so a token cannot reach a log line
/// by accident. Read the value with [`as_str`](Self::as_str) only to put it in a page or a
/// response body.
///
/// # Examples
///
/// ```
/// use oxbloom_security::CsrfToken;
///
/// fn show(token: &CsrfToken) -> String {
///     format!("{token:?}")
/// }
/// // Display is not implemented, so `format!("{token}")` does not compile.
/// # let _ = show;
/// ```
#[derive(Clone)]
pub struct CsrfToken(String);

impl CsrfToken {
    /// Mints a token from the system random number generator.
    ///
    /// # Panics
    ///
    /// Panics when the system random number generator fails, because no safe token can be made
    /// without it.
    pub(crate) fn generate() -> CsrfToken {
        let mut bytes = [0u8; 32];
        aws_lc_rs::rand::fill(&mut bytes).expect("the system random number generator failed");
        CsrfToken(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Wraps a stored value when it has the shape of a token: 43 base64url characters.
    pub(crate) fn from_stored(value: String) -> Option<CsrfToken> {
        let ok = value.len() == TOKEN_LEN
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        ok.then_some(CsrfToken(value))
    }

    /// The token value, for a page or a response body.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::CsrfToken;
    ///
    /// fn len(token: &CsrfToken) -> usize {
    ///     token.as_str().len()
    /// }
    /// # let _ = len;
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `submitted` equals this token. The comparison takes the same time whatever the
    /// first differing byte and whatever the length difference. An empty value never matches.
    pub(crate) fn matches(&self, submitted: &str) -> bool {
        let a = self.0.as_bytes();
        let b = submitted.as_bytes();
        let mut diff = usize::from(a.len() != b.len());
        for i in 0..a.len().max(b.len()) {
            let x = a.get(i).copied().unwrap_or(0);
            let y = b.get(i).copied().unwrap_or(0);
            diff |= usize::from(std::hint::black_box(x ^ y));
        }
        diff == 0 && !a.is_empty()
    }
}

impl fmt::Debug for CsrfToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CsrfToken(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_have_43_url_safe_characters_and_differ() {
        let a = CsrfToken::generate();
        let b = CsrfToken::generate();
        for t in [&a, &b] {
            assert_eq!(t.as_str().len(), 43);
            assert!(
                t.as_str()
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            );
        }
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn matches_is_true_only_for_the_same_value() {
        let t = CsrfToken::generate();
        let v = t.as_str().to_owned();
        assert!(t.matches(&v));
        let mut off = v.clone().into_bytes();
        off[10] = if off[10] == b'A' { b'B' } else { b'A' };
        assert!(!t.matches(std::str::from_utf8(&off).unwrap()));
        assert!(!t.matches(&v[..42]));
        assert!(!t.matches(&format!("{v}x")));
        assert!(!t.matches(""));
        let empty = CsrfToken(String::new());
        assert!(!empty.matches(""));
        assert!(!empty.matches("x"));
    }

    #[test]
    fn debug_hides_the_token_and_there_is_no_display() {
        let t = CsrfToken::generate();
        let shown = format!("{t:?}");
        assert!(!shown.contains(t.as_str()), "{shown}");
        assert!(shown.contains("CsrfToken"));
    }

    #[test]
    fn from_stored_rejects_wrong_length_and_alphabet() {
        let good = "A".repeat(43);
        assert_eq!(
            CsrfToken::from_stored(good.clone()).map(|t| t.0),
            Some(good.clone())
        );
        assert!(CsrfToken::from_stored("A".repeat(42)).is_none());
        assert!(CsrfToken::from_stored("A".repeat(44)).is_none());
        assert!(CsrfToken::from_stored(String::new()).is_none());
        assert!(CsrfToken::from_stored(format!("{}+", "A".repeat(42))).is_none());
        assert!(CsrfToken::from_stored(format!("{}=", "A".repeat(42))).is_none());
        assert!(CsrfToken::from_stored(format!("{}é", "A".repeat(41))).is_none());
    }
}
