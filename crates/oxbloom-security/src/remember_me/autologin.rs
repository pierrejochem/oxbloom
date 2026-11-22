//! Auto login: the remember-me cookie of a request that has neither a header credential nor a
//! session login is checked, and a valid one names the user to log in.
//!
//! The checks run from the cheapest to the dearest: the format, then the expiry, both before the
//! user service is asked, so a forged or stale cookie costs no lookup; then the user, who must
//! exist, be enabled and not be locked; then the MAC over that user's stored password hash. The
//! client cannot tell which check failed. No cookie, token or username reaches a log line.

use oxbloom_web::Request;

use super::RememberMeRuntime;
use super::cookie::read_cookie;
use crate::{Authentication, UserDetails};

/// What auto login did for a request.
#[derive(Debug)]
pub(crate) enum AutoLogin {
    /// The cookie was valid; the session holds this authentication, marked remembered.
    LoggedIn(Authentication),
    /// The cookie was invalid; the response clears it and the request goes on anonymous.
    Invalid,
    /// The user service failed or the session could not be written; the cookie is kept and the
    /// request goes on anonymous.
    Unavailable,
    /// There was no cookie to check, or no session to log in to.
    Absent,
}

/// What the check of a cookie found.
pub(crate) enum Checked {
    /// The cookie is valid for this user.
    Valid(UserDetails),
    /// The cookie is malformed, expired, forged or no longer matches its user.
    Invalid,
    /// The user service failed.
    Unavailable,
}

/// The remember-me cookie of `req`: the first value of the configured name, `None` when there
/// is none, it is empty or the `Cookie` header is over 8 KiB.
pub(crate) fn remember_me_token(runtime: &RememberMeRuntime, req: &Request) -> Option<String> {
    read_cookie(req, &runtime.settings.cookie_name).filter(|value| !value.is_empty())
}

/// Checks `token`: format and expiry first, then the user, then the MAC with the user's
/// password hash.
pub(crate) async fn check_token(runtime: &RememberMeRuntime, token: &str) -> Checked {
    let Some(parsed) = runtime.codec.parse(token) else {
        return Checked::Invalid;
    };
    // Expired before any lookup, so a stale or forged expiry costs no call to the user service.
    if parsed.expires() <= runtime.settings.clock.now() {
        return Checked::Invalid;
    }
    let Some(users) = runtime.settings.user_details.as_ref() else {
        return Checked::Invalid;
    };
    let user = match users.load_user_by_username(parsed.username()).await {
        Ok(user) => user,
        Err(_) => {
            tracing::warn!(
                target: "oxbloom::security",
                "the user service failed during the remember-me login; the cookie is kept"
            );
            return Checked::Unavailable;
        }
    };
    match user.filter(|u| u.is_enabled() && !u.is_locked()) {
        Some(user) if runtime.codec.verify(&parsed, user.password()) => Checked::Valid(user),
        _ => Checked::Invalid,
    }
}

/// Logs that a remember-me cookie arrived but the request has no session, which means the
/// application has no session filter.
pub(crate) fn no_session_error() {
    tracing::error!(
        target: "oxbloom::security",
        "remember-me has no session: add a SessionFilter to the application"
    );
}

/// Logs that a cookie was refused. The reason stays out of the log as it stays out of the
/// answer.
pub(crate) fn invalid_cookie() {
    tracing::debug!(
        target: "oxbloom::security",
        "the remember-me cookie was refused and is cleared"
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    use async_trait::async_trait;
    use oxbloom_config::Secret;
    use oxbloom_core::Clock;

    use super::*;
    use crate::{RememberMe, UserDetailsError, UserDetailsService};

    struct Fixed(SystemTime);

    impl Clock for Fixed {
        fn now(&self) -> SystemTime {
            self.0
        }
    }

    struct Users(Mutex<Vec<String>>, Option<UserDetails>);

    #[async_trait]
    impl UserDetailsService for Users {
        async fn load_user_by_username(
            &self,
            username: &str,
        ) -> Result<Option<UserDetails>, UserDetailsError> {
            self.0.lock().unwrap().push(username.to_owned());
            Ok(self.1.clone().filter(|u| u.username() == username))
        }
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn runtime(user: Option<UserDetails>) -> (RememberMeRuntime, Arc<Users>) {
        let users = Arc::new(Users(Mutex::new(Vec::new()), user));
        let settings = RememberMe::new()
            .secret(Secret::new("a-remember-me-secret-of-32-bytes".to_owned()))
            .user_details(users.clone())
            .clock(Arc::new(Fixed(at(1000))));
        (RememberMeRuntime::new(settings).unwrap(), users)
    }

    #[tokio::test]
    async fn a_valid_token_names_its_user_and_an_expired_one_asks_no_one() {
        let alice = UserDetails::new("alice", "hash", []);
        let (rt, users) = runtime(Some(alice));
        let token = rt.codec.encode("alice", "hash", at(1001)).unwrap();
        assert!(
            matches!(check_token(&rt, &token).await, Checked::Valid(u) if u.username() == "alice")
        );
        let expired = rt.codec.encode("alice", "hash", at(1000)).unwrap();
        assert!(matches!(check_token(&rt, &expired).await, Checked::Invalid));
        assert_eq!(users.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_wrong_hash_is_invalid() {
        let (rt, _) = runtime(Some(UserDetails::new("alice", "other", [])));
        let token = rt.codec.encode("alice", "hash", at(2000)).unwrap();
        assert!(matches!(check_token(&rt, &token).await, Checked::Invalid));
    }
}
