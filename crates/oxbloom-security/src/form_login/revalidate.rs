//! Revalidation: the user of a session login is loaded again, at most once per interval, and the
//! session follows what the user service says now.
//!
//! The user is selected by the principal of the stored authentication, never by the principal
//! key, which application code could write. No username or session id reaches a log line.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::repository::{
    epoch_seconds, load_revalidated_at, refresh_authentication, store_revalidated_at,
};
use crate::dao::authentication_of;
use crate::{Authentication, Session, UserDetailsService};

/// The revalidation settings of a chain: the interval and the user service.
pub(crate) struct Revalidation {
    interval: Duration,
    users: Arc<dyn UserDetailsService>,
}

impl fmt::Debug for Revalidation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Revalidation")
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

impl Revalidation {
    /// `interval` is positive; `build` checks that.
    pub(crate) fn new(interval: Duration, users: Arc<dyn UserDetailsService>) -> Self {
        Self { interval, users }
    }

    /// Whether the last check, at `last` seconds since the epoch, is due again at `now`. A
    /// missing time is due, and so is a time after `now`, so a value from the future never
    /// postpones the check.
    fn due(&self, last: Option<u64>, now: u64) -> bool {
        match last {
            Some(last) if last <= now => now - last >= self.interval.as_secs(),
            _ => true,
        }
    }
}

/// What revalidation did to the session.
#[derive(Debug)]
pub(crate) enum Revalidated {
    /// The user was checked and nothing changed; the time of the check advanced.
    Unchanged,
    /// The authorities changed; the session holds this new authentication.
    Updated(Authentication),
    /// The user is missing, disabled or locked; the session was invalidated.
    Ended,
    /// No check was due, or the user service failed; the session is as it was.
    Skipped,
}

/// Checks the user of `authentication`, the authentication stored in `session`, when the last
/// check is due at `now`.
pub(crate) async fn revalidate(
    revalidation: &Revalidation,
    session: &Session,
    authentication: &Authentication,
    now: SystemTime,
) -> Revalidated {
    if !revalidation.due(load_revalidated_at(session), epoch_seconds(now)) {
        return Revalidated::Skipped;
    }
    let user = match revalidation
        .users
        .load_user_by_username(authentication.principal())
        .await
    {
        Ok(user) => user,
        Err(_) => {
            tracing::warn!(
                target: "oxbloom::security",
                "the user service failed during revalidation; the session is kept"
            );
            return Revalidated::Skipped;
        }
    };
    let Some(user) = user.filter(|u| u.is_enabled() && !u.is_locked()) else {
        return end(session);
    };
    let fresh = authentication_of(&user);
    let outcome = if fresh.authorities() == authentication.authorities() {
        Revalidated::Unchanged
    } else {
        // The principal and the details of the login stay; only the authorities change.
        let updated = Authentication::new(
            authentication.principal(),
            fresh.authorities().iter().cloned(),
        )
        .with_details(authentication.details().clone());
        if refresh_authentication(session, &updated).is_err() {
            tracing::error!(
                target: "oxbloom::security",
                "the revalidated authentication could not be stored; the session is ended"
            );
            return end(session);
        }
        Revalidated::Updated(updated)
    };
    if store_revalidated_at(session, now).is_err() {
        tracing::error!(
            target: "oxbloom::security",
            "the revalidation time could not be stored; the session is ended"
        );
        return end(session);
    }
    outcome
}

fn end(session: &Session) -> Revalidated {
    session.invalidate();
    tracing::debug!(
        target: "oxbloom::security",
        "the session ended at revalidation: the user is missing, disabled or locked"
    );
    Revalidated::Ended
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form_login::repository::{
        AUTHENTICATION_KEY, REVALIDATED_AT_KEY, store_authentication,
    };
    use crate::session::handle::SessionState;
    use crate::{GrantedAuthority, UserDetails, UserDetailsError};
    use async_trait::async_trait;

    struct One(Option<UserDetails>);

    #[async_trait]
    impl UserDetailsService for One {
        async fn load_user_by_username(
            &self,
            username: &str,
        ) -> Result<Option<UserDetails>, UserDetailsError> {
            Ok(self.0.clone().filter(|u| u.username() == username))
        }
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn setup(user: Option<UserDetails>) -> (Revalidation, Session, Authentication) {
        let session = Session::from_state(SessionState::fresh(at(0)));
        let auth = Authentication::new("alice", [GrantedAuthority::new("ROLE_USER")]);
        store_authentication(&session, &auth, at(1000)).unwrap();
        store_revalidated_at(&session, at(1000)).unwrap();
        let revalidation = Revalidation::new(Duration::from_secs(60), Arc::new(One(user)));
        (revalidation, session, auth)
    }

    #[tokio::test]
    async fn not_due_is_skipped_and_due_unchanged_advances_the_time() {
        let user = UserDetails::new("alice", "x", []).roles(&["USER"]);
        let (r, s, auth) = setup(Some(user));
        assert!(matches!(
            revalidate(&r, &s, &auth, at(1059)).await,
            Revalidated::Skipped
        ));
        assert_eq!(load_revalidated_at(&s), Some(1000));
        assert!(matches!(
            revalidate(&r, &s, &auth, at(1060)).await,
            Revalidated::Unchanged
        ));
        assert_eq!(load_revalidated_at(&s), Some(1060));
    }

    #[tokio::test]
    async fn a_time_from_the_future_is_due() {
        let (r, s, auth) = setup(None);
        s.set(REVALIDATED_AT_KEY, &5000u64).unwrap();
        assert!(matches!(
            revalidate(&r, &s, &auth, at(1001)).await,
            Revalidated::Ended
        ));
    }

    #[tokio::test]
    async fn a_missing_user_invalidates_the_session() {
        let (r, s, auth) = setup(None);
        assert!(matches!(
            revalidate(&r, &s, &auth, at(2000)).await,
            Revalidated::Ended
        ));
        assert!(s.take_state().invalidated);
    }

    #[tokio::test]
    async fn the_principal_key_does_not_select_the_user() {
        let user = UserDetails::new("alice", "x", []).roles(&["USER"]);
        let (r, s, auth) = setup(Some(user));
        s.set(crate::session::PRINCIPAL_KEY, "mallory").unwrap();
        assert!(matches!(
            revalidate(&r, &s, &auth, at(2000)).await,
            Revalidated::Unchanged
        ));
    }

    #[tokio::test]
    async fn changed_authorities_are_stored() {
        let user = UserDetails::new("alice", "x", []).roles(&["ADMIN"]);
        let (r, s, auth) = setup(Some(user));
        let Revalidated::Updated(updated) = revalidate(&r, &s, &auth, at(2000)).await else {
            panic!("expected an update");
        };
        assert!(updated.has_role("ADMIN") && !updated.has_role("USER"));
        let stored: serde_json::Value = s.get(AUTHENTICATION_KEY).unwrap().unwrap();
        assert_eq!(stored["login"], 1000);
        assert_eq!(load_revalidated_at(&s), Some(2000));
    }

    #[test]
    fn debug_shows_the_interval_only() {
        let (r, _, _) = setup(None);
        assert_eq!(format!("{r:?}"), "Revalidation { interval: 60s, .. }");
    }
}
