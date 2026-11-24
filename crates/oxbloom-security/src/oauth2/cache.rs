//! The in-memory cache of access tokens, one slot per registration.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use oxbloom_core::Clock;

use super::client::{AccessToken, TokenError};

/// The access token of a slot and the generation it was stored under.
struct Cached {
    token: AccessToken,
    generation: u64,
}

/// What a slot remembers: the usable token, and the outcome of the last request together with
/// the generation that request started under, for the callers that waited for it.
#[derive(Default)]
struct SlotState {
    token: Option<Cached>,
    last: Option<(Result<AccessToken, TokenError>, u64)>,
}

/// One slot per registration. Its async lock is held while the token request is in flight,
/// so concurrent callers wait for one request instead of starting their own. `attempts` counts
/// the finished requests; a caller that sees it change while it waited takes the outcome of
/// that request. `generation` is bumped by `invalidate`, which makes every token stored under
/// an older generation unusable.
#[derive(Default)]
struct Slot {
    attempts: AtomicU64,
    generation: AtomicU64,
    state: tokio::sync::Mutex<SlotState>,
}

/// Holds one access token per registration until it is within the skew of its expiry.
///
/// The cache never creates a slot for a registration the caller has not checked, so the number
/// of slots is bounded by the number of configured registrations.
#[derive(Default)]
pub(crate) struct TokenCache {
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

/// Whether `token` can still be used at `now`: it is used until `skew` before its expiry.
fn is_fresh(token: &AccessToken, now: SystemTime, skew: Duration) -> bool {
    token
        .expires_at
        .checked_sub(skew)
        .is_some_and(|limit| now < limit)
}

impl TokenCache {
    /// Returns the cached token of `registration_id` while it is fresh, otherwise runs `fetch`
    /// and caches the token.
    ///
    /// Callers that arrive while a request is in flight wait for it and take its outcome, a
    /// token or a failure, so there is one request at a time per registration. A failure is
    /// not kept: the next caller that was not waiting starts a new request at once. A token
    /// that is within the skew of its expiry when it arrives (including one that has expired
    /// already, `expires_in = 0`) is returned but not stored, so the next call asks again
    /// and one call never makes more than one request.
    pub(crate) async fn get_or_fetch<F, Fut>(
        &self,
        registration_id: &str,
        clock: &dyn Clock,
        skew: Duration,
        fetch: F,
    ) -> Result<AccessToken, TokenError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<AccessToken, TokenError>>,
    {
        let slot = self
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(registration_id.to_owned())
            .or_default()
            .clone();
        let seen = slot.attempts.load(Ordering::Acquire);
        let arrived_in = slot.generation.load(Ordering::Acquire);
        let mut state = slot.state.lock().await;
        // A request finished while this caller waited. Its failure is shared. Its token is
        // shared unless the token was invalidated after this caller arrived.
        if slot.attempts.load(Ordering::Acquire) != seen
            && let Some((outcome, generation)) = &state.last
            && (outcome.is_err() || *generation >= arrived_in)
        {
            return outcome.clone();
        }
        let generation = slot.generation.load(Ordering::Acquire);
        if let Some(cached) = &state.token
            && cached.generation == generation
            && is_fresh(&cached.token, clock.now(), skew)
        {
            return Ok(cached.token.clone());
        }
        // A stale token is dropped now, so a failed request does not leave it in memory.
        state.token = None;
        let outcome = fetch().await;
        if let Ok(token) = &outcome
            && slot.generation.load(Ordering::Acquire) == generation
            && is_fresh(token, clock.now(), skew)
        {
            state.token = Some(Cached {
                token: token.clone(),
                generation,
            });
        }
        state.last = Some((outcome.clone(), generation));
        // Counted while the lock is still held, so every waiter sees it.
        slot.attempts.fetch_add(1, Ordering::Release);
        outcome
    }

    /// Drops the token of `registration_id`, so the next call requests a new one. A request
    /// that is in flight still answers its callers, but its token is not kept.
    pub(crate) fn invalidate(&self, registration_id: &str) {
        let slot = self
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(registration_id)
            .cloned();
        if let Some(slot) = slot {
            slot.generation.fetch_add(1, Ordering::AcqRel);
            // Free the token now when no request holds the lock; otherwise the generation
            // already makes it unusable.
            if let Ok(mut state) = slot.state.try_lock() {
                state.token = None;
                state.last = None;
            }
        }
    }
}

impl fmt::Debug for TokenCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenCache").finish_non_exhaustive()
    }
}
