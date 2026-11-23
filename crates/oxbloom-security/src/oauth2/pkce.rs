//! PKCE (RFC 7636) verifiers and the random tokens used for `state` and `nonce`.

use std::fmt;

use aws_lc_rs::digest::{SHA256, digest};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// A PKCE code verifier: 43 characters of the unreserved set, from 32 random bytes.
///
/// `Debug` does not show the value.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Verifier(String);

impl Verifier {
    /// Makes a verifier from the system random number generator.
    ///
    /// # Panics
    ///
    /// Panics when the system random number generator fails.
    pub(crate) fn generate() -> Verifier {
        Verifier(random_token(32))
    }

    /// Wraps a verifier that was stored earlier.
    pub(crate) fn from_string(value: String) -> Verifier {
        Verifier(value)
    }

    /// The verifier text, sent to the token endpoint.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The `S256` challenge: base64url without padding of the SHA-256 of the verifier.
    pub(crate) fn challenge_s256(&self) -> String {
        URL_SAFE_NO_PAD.encode(digest(&SHA256, self.0.as_bytes()))
    }
}

impl fmt::Debug for Verifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Verifier(<redacted>)")
    }
}

/// Returns `bytes` random bytes as base64url without padding, for `state` and `nonce`.
///
/// # Panics
///
/// Panics when the system random number generator fails.
pub(crate) fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    aws_lc_rs::rand::fill(&mut buffer).expect("the system random number generator failed");
    URL_SAFE_NO_PAD.encode(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_the_rfc_7636_test_vector() {
        let verifier = Verifier::from_string("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(
            verifier.challenge_s256(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_verifiers_have_valid_length_and_alphabet_and_differ() {
        let a = Verifier::generate();
        let b = Verifier::generate();
        assert_ne!(a, b);
        for v in [&a, &b] {
            let text = v.as_str();
            assert!((43..=128).contains(&text.len()));
            assert!(
                text.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c))
            );
        }
    }

    #[test]
    fn random_tokens_are_url_safe_and_unique() {
        let a = random_token(16);
        let b = random_token(16);
        assert_ne!(a, b);
        assert_eq!(a.len(), 22);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn debug_of_the_verifier_is_redacted() {
        let v = Verifier::from_string("secret-verifier".into());
        assert!(!format!("{v:?}").contains("secret-verifier"));
    }
}
