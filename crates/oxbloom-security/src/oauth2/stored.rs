//! The token set of an OAuth2 login, kept in the session per registration.
//!
//! The value under `oxbloom.security.oauth2.tokens.<registration>` is the JSON object
//! `{ "v": 1, "access_token", "token_type", "expires_at", "refresh_token", "id_token", "scope" }`,
//! with `expires_at` in seconds since the epoch. The OAuth2 client reads it for the user's
//! access token. `Debug` never shows a token.

use std::fmt;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::TokenResponse;
use crate::form_login::{OAUTH2_TOKENS_PREFIX as TOKENS_PREFIX, epoch_seconds};
use crate::{Session, SessionError};

/// The version of the stored value.
const VERSION: u32 = 1;

/// The tokens of one login, as stored in the session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredTokens {
    /// The version of the value, always 1.
    pub(crate) v: u32,
    /// The access token.
    pub(crate) access_token: String,
    /// The token type, `Bearer`.
    pub(crate) token_type: String,
    /// When the access token expires, in seconds since the epoch, when the provider said.
    pub(crate) expires_at: Option<u64>,
    /// The refresh token, when the provider sent one.
    pub(crate) refresh_token: Option<String>,
    /// The id token of an OpenID Connect login.
    pub(crate) id_token: Option<String>,
    /// The granted scopes, space-separated.
    pub(crate) scope: Option<String>,
}

impl fmt::Debug for StoredTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredTokens")
            .field("v", &self.v)
            .field("access_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_at", &self.expires_at)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .field("scope", &self.scope)
            .finish()
    }
}

fn key_for(registration: &str) -> String {
    format!("{TOKENS_PREFIX}{registration}")
}

impl StoredTokens {
    /// The value for `tokens`, received at `now`, with the scopes that were granted (already
    /// checked); an empty list stores no `scope`.
    pub(crate) fn new(tokens: &TokenResponse, granted_scopes: &[String], now: SystemTime) -> Self {
        Self {
            v: VERSION,
            access_token: tokens.access_token.expose().clone(),
            token_type: tokens.token_type.clone(),
            expires_at: tokens.expires_at(now).map(epoch_seconds),
            refresh_token: tokens.refresh_token.as_ref().map(|t| t.expose().clone()),
            id_token: tokens.id_token.as_ref().map(|t| t.expose().clone()),
            scope: (!granted_scopes.is_empty()).then(|| granted_scopes.join(" ")),
        }
    }

    /// Stores the tokens of `registration`, replacing earlier ones.
    ///
    /// # Errors
    ///
    /// [`SessionError::Serialization`] when the value cannot be written.
    pub(crate) fn store(&self, session: &Session, registration: &str) -> Result<(), SessionError> {
        session.set_reserved(&key_for(registration), self)
    }

    /// The tokens of `registration`. A value that does not deserialize or has another version
    /// is removed and gives `None`.
    pub(crate) fn load(session: &Session, registration: &str) -> Option<Self> {
        let key = key_for(registration);
        match session.get::<Self>(&key) {
            Ok(Some(tokens)) if tokens.v == VERSION => Some(tokens),
            Ok(None) => None,
            _ => {
                session.remove_reserved(&key);
                None
            }
        }
    }

    /// Removes the tokens of `registration`.
    pub(crate) fn remove(session: &Session, registration: &str) {
        session.remove_reserved(&key_for(registration));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oxbloom_config::Secret;

    use super::*;
    use crate::session::handle::SessionState;

    fn session() -> Session {
        Session::from_state(SessionState::fresh(SystemTime::UNIX_EPOCH))
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn response(expires_in: Option<u64>) -> TokenResponse {
        TokenResponse {
            access_token: Secret::new("the-access-token".into()),
            token_type: "Bearer".into(),
            expires_in,
            refresh_token: Some(Secret::new("the-refresh-token".into())),
            id_token: Some(Secret::new("the-id-token".into())),
            scope: Some("ignored here".into()),
        }
    }

    #[test]
    fn the_json_shape_is_fixed() {
        let s = session();
        let scopes = vec!["openid".to_owned(), "email".to_owned()];
        StoredTokens::new(&response(Some(3600)), &scopes, now())
            .store(&s, "idp")
            .unwrap();
        let value: serde_json::Value = s
            .get("oxbloom.security.oauth2.tokens.idp")
            .unwrap()
            .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "v": 1,
                "access_token": "the-access-token",
                "token_type": "Bearer",
                "expires_at": 1_800_003_600u64,
                "refresh_token": "the-refresh-token",
                "id_token": "the-id-token",
                "scope": "openid email",
            })
        );
    }

    #[test]
    fn store_load_and_remove_round_trip_per_registration() {
        let s = session();
        let tokens = StoredTokens::new(&response(None), &[], now());
        assert_eq!(tokens.expires_at, None);
        assert_eq!(tokens.scope, None);
        tokens.store(&s, "a").unwrap();
        assert_eq!(StoredTokens::load(&s, "a"), Some(tokens.clone()));
        assert_eq!(StoredTokens::load(&s, "b"), None);
        StoredTokens::remove(&s, "a");
        assert_eq!(StoredTokens::load(&s, "a"), None);
    }

    #[test]
    fn a_huge_lifetime_never_overflows() {
        let tokens = StoredTokens::new(&response(Some(u64::MAX)), &[], now());
        assert!(tokens.expires_at.is_some());
    }

    #[test]
    fn a_bad_or_other_version_value_is_removed() {
        let s = session();
        let key = "oxbloom.security.oauth2.tokens.idp";
        s.set_reserved(key, &serde_json::json!({"v": 1})).unwrap();
        assert_eq!(StoredTokens::load(&s, "idp"), None);
        assert_eq!(s.get::<serde_json::Value>(key).unwrap(), None);
        let mut tokens = StoredTokens::new(&response(None), &[], now());
        tokens.v = 2;
        tokens.store(&s, "idp").unwrap();
        assert_eq!(StoredTokens::load(&s, "idp"), None);
        assert_eq!(s.get::<serde_json::Value>(key).unwrap(), None);
    }

    #[test]
    fn debug_hides_every_token() {
        let tokens = StoredTokens::new(&response(Some(1)), &[], now());
        let text = format!("{tokens:?}");
        for secret in ["the-access-token", "the-refresh-token", "the-id-token"] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}
