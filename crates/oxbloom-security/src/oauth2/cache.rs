//! The in-memory cache of access tokens, one slot per registration.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use oxbloom_core::Clock;

use super::client::{AccessToken, TokenError};
use super::constant_time_eq;

/// The access token of a slot and the generation it was stored under.
struct Cached {
    token: AccessToken,
    generation: u64,
}

/// The outcome of the last request, with the generation that request started under, for the
/// callers that waited for it.
type Outcome = (Result<AccessToken, TokenError>, u64);

/// One slot per registration. Its async lock `state` is held while the token request is in
/// flight, so concurrent callers wait for one request instead of starting their own.
/// `attempts` counts the finished requests; a caller that sees it change while it waited takes
/// the outcome of that request. `generation` is bumped by the invalidations, which makes every
/// token stored under an older generation unusable.
///
/// The usable token lives in `current` behind a plain mutex that is never held across an
/// await, so an invalidation can look at it and change it at any time, also while a request
/// holds `state`. The generation is only changed with `current` locked, so storing a token
/// and invalidating are atomic with respect to each other.
#[derive(Default)]
struct Slot {
    attempts: AtomicU64,
    generation: AtomicU64,
    waiting: AtomicUsize,
    current: Mutex<Option<Cached>>,
    state: tokio::sync::Mutex<Option<Outcome>>,
}

/// Counts a caller as waiting for a lock until it has the lock or is dropped.
pub(super) struct Waiting<'a>(&'a AtomicUsize);

impl<'a> Waiting<'a> {
    pub(super) fn new(counter: &'a AtomicUsize) -> Waiting<'a> {
        counter.fetch_add(1, Ordering::AcqRel);
        Waiting(counter)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TokenCache {
    fn slot(&self, registration_id: &str) -> Option<Arc<Slot>> {
        lock(&self.slots).get(registration_id).cloned()
    }

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
        let slot = lock(&self.slots)
            .entry(registration_id.to_owned())
            .or_default()
            .clone();
        let seen = slot.attempts.load(Ordering::Acquire);
        let arrived_in = slot.generation.load(Ordering::Acquire);
        let mut last = {
            let _waiting = Waiting::new(&slot.waiting);
            slot.state.lock().await
        };
        // A request finished while this caller waited. Its failure is shared. Its token is
        // shared unless it was invalidated before this caller arrived: the request started
        // under an older generation than the one this caller saw.
        if slot.attempts.load(Ordering::Acquire) != seen
            && let Some((outcome, generation)) = &*last
            && (outcome.is_err() || *generation >= arrived_in)
        {
            return outcome.clone();
        }
        let generation;
        {
            let mut current = lock(&slot.current);
            // Read with `current` locked, as the invalidations change it, so the generation
            // and the token this caller looks at belong together.
            generation = slot.generation.load(Ordering::Acquire);
            if let Some(cached) = &*current
                && cached.generation == generation
                && is_fresh(&cached.token, clock.now(), skew)
            {
                return Ok(cached.token.clone());
            }
            // A stale token is dropped now, so a failed request does not leave it in memory.
            *current = None;
        }
        let outcome = fetch().await;
        if let Ok(token) = &outcome
            && is_fresh(token, clock.now(), skew)
        {
            let mut current = lock(&slot.current);
            // Still the generation the request started under: no invalidation happened.
            if slot.generation.load(Ordering::Acquire) == generation {
                *current = Some(Cached {
                    token: token.clone(),
                    generation,
                });
            }
        }
        *last = Some((outcome.clone(), generation));
        // Counted while the lock is still held, so every waiter sees it.
        slot.attempts.fetch_add(1, Ordering::Release);
        outcome
    }

    /// Drops the token of `registration_id`, so the next call requests a new one. A request
    /// that is in flight still answers its callers, but its token is not kept.
    pub(crate) fn invalidate(&self, registration_id: &str) {
        if let Some(slot) = self.slot(registration_id) {
            let mut current = lock(&slot.current);
            slot.generation.fetch_add(1, Ordering::AcqRel);
            *current = None;
            drop(current);
            forget_last(&slot);
        }
    }

    /// Drops the token of `registration_id` only if it is `refused`: the same expiry and the
    /// same value (compared in constant time). Any other state, no token, a newer token or
    /// a request in flight, is left alone, so a late refusal of an old token cannot discard a
    /// fresh one.
    pub(crate) fn invalidate_token(&self, registration_id: &str, refused: &AccessToken) {
        if let Some(slot) = self.slot(registration_id) {
            let mut current = lock(&slot.current);
            let matches = current.as_ref().is_some_and(|cached| {
                cached.token.expires_at == refused.expires_at
                    && constant_time_eq(cached.token.value.expose(), refused.value.expose())
            });
            if matches {
                slot.generation.fetch_add(1, Ordering::AcqRel);
                *current = None;
                drop(current);
                forget_last(&slot);
            }
        }
    }

    /// The number of callers of `registration_id` that are waiting for a request in flight
    /// (or for the lock of the slot). For tests.
    pub(crate) fn waiting(&self, registration_id: &str) -> usize {
        self.slot(registration_id)
            .map_or(0, |slot| slot.waiting.load(Ordering::Acquire))
    }
}

/// Frees the outcome of the last request when no request holds the slot; otherwise the
/// generation already keeps its token from being shared or stored.
fn forget_last(slot: &Slot) {
    if let Ok(mut last) = slot.state.try_lock() {
        *last = None;
    }
}

impl fmt::Debug for TokenCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenCache").finish_non_exhaustive()
    }
}
