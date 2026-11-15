//! The stateless session backend: the session data travels in the cookie, encrypted and
//! authenticated with AES-256-GCM.
//!
//! # Format
//!
//! The cookie value is base64url without padding of `nonce || ciphertext || tag`. The nonce
//! is 12 fresh random bytes on every write, the tag is the 16-byte GCM tag, and the cookie
//! name is the associated data, so a value moved to a cookie with another name does not open.
//! The plaintext is the JSON object `{"v": 1, "c": created, "t": touched, "d": values}`, where
//! both times are whole nanoseconds since the Unix epoch.
//!
//! # Keys
//!
//! Every secret, the current one and each old one, must be at least 32 bytes. Each derives its
//! AES-256 key the same way: HKDF-SHA256 with an empty salt over the whole secret, expanded with
//! the fixed info string `oxbloom session cookie v1` to 32 bytes. Writes use the key of the
//! current secret; reads try the current key first and then each old key in order, so a secret
//! can be rotated without ending the sessions it sealed.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use aws_lc_rs::hkdf::{HKDF_SHA256, Salt};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::error::SessionError;
use super::settings::SessionSettings;
use super::store::SessionData;

/// The shortest secret, in bytes, the backend accepts.
const MIN_SECRET_BYTES: usize = 32;

/// The HKDF info string that binds a derived key to this use and this format version.
const KEY_INFO: &[u8] = b"oxbloom session cookie v1";

/// The payload format version.
const VERSION: u64 = 1;

/// The length of the AES-GCM tag in bytes.
const TAG_LEN: usize = 16;

/// The payload as it is written.
#[derive(Serialize)]
struct PayloadOut<'a> {
    v: u64,
    c: u64,
    t: u64,
    d: &'a Map<String, Value>,
}

/// The payload as it is read back.
#[derive(Deserialize)]
struct PayloadIn {
    c: u64,
    t: u64,
    d: Map<String, Value>,
}

/// Only the version, read before the rest so that an unknown version is told apart from a
/// malformed payload.
#[derive(Deserialize)]
struct VersionProbe {
    v: u64,
}

/// Session data kept in the cookie itself, encrypted and authenticated with AES-256-GCM.
///
/// Nothing is stored on the server, so sessions survive restarts and are shared by every
/// instance that holds the same secret. The price is that the cookie grows with the data
/// (a write over `max_cookie_bytes` fails with [`SessionError::TooLarge`]), and that a session
/// cannot be revoked: an invalidated session clears the cookie, but a copy of the old cookie
/// stays valid until its idle or absolute timeout passes.
///
/// The format and the key derivation are described in the module documentation of the
/// backend: HKDF-SHA256 over each whole secret with the info string
/// `oxbloom session cookie v1`, a fresh 96-bit nonce per write, and the cookie name as the
/// associated data. `Debug` prints `CookieSessionBackend { .. }` and never a key.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_config::Secret;
/// use oxbloom_core::SystemClock;
/// use oxbloom_security::{CookieSessionBackend, SessionSettings};
///
/// let secret = Secret::new("a-secret-of-at-least-thirty-two-bytes".to_owned());
/// let backend = CookieSessionBackend::new(
///     &secret,
///     &[],
///     &SessionSettings::default(),
///     Arc::new(SystemClock),
/// )
/// .unwrap();
/// assert_eq!(format!("{backend:?}"), "CookieSessionBackend { .. }");
/// ```
pub struct CookieSessionBackend {
    /// The current key first, then the old keys in order.
    keys: Vec<LessSafeKey>,
    cookie_name: String,
    max_cookie_bytes: usize,
    idle_timeout: Duration,
    absolute_timeout: Duration,
    clock: Arc<dyn Clock>,
}

/// Derives the AES-256-GCM key of one secret.
fn derive_key(secret: &Secret<String>) -> Result<LessSafeKey, SessionError> {
    let bytes = secret.expose().as_bytes();
    if bytes.len() < MIN_SECRET_BYTES {
        return Err(SessionError::InvalidSettings);
    }
    let prk = Salt::new(HKDF_SHA256, &[]).extract(bytes);
    let info = [KEY_INFO];
    let okm = prk
        .expand(&info, &AES_256_GCM)
        .map_err(|_| SessionError::Codec)?;
    Ok(LessSafeKey::new(UnboundKey::from(okm)))
}

/// Whole nanoseconds since the Unix epoch, or `None` for a time before the epoch or after
/// the year 2554.
fn to_nanos(time: SystemTime) -> Option<u64> {
    let since = time.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    u64::try_from(since.as_nanos()).ok()
}

fn from_nanos(nanos: u64) -> Option<SystemTime> {
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_nanos(nanos))
}

/// Whether a deadline `start + timeout` has passed at `now`. A deadline that does not fit in a
/// `SystemTime` means no limit, as in the in-memory store.
fn has_passed(start: SystemTime, timeout: Duration, now: SystemTime) -> bool {
    start
        .checked_add(timeout)
        .is_some_and(|deadline| deadline <= now)
}

/// Logs a failed open at `debug` with a fixed reason word and nothing else.
fn rejected(reason: &'static str) -> Option<SessionData> {
    tracing::debug!(
        target: "oxbloom::security",
        reason,
        "the session cookie was rejected"
    );
    None
}

impl CookieSessionBackend {
    /// Creates the backend from the current secret, the old secrets that may still open
    /// cookies, the settings and the clock that decides the timeouts.
    ///
    /// # Errors
    ///
    /// [`SessionError::InvalidSettings`] when any secret, current or old, is shorter than 32
    /// bytes, or when the settings do not pass [`SessionSettings::validate`]. The error names
    /// no secret.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_config::Secret;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{CookieSessionBackend, SessionError, SessionSettings};
    ///
    /// let settings = SessionSettings::default();
    /// let current = Secret::new("the-new-secret-of-at-least-32-bytes".to_owned());
    /// let old = [Secret::new("the-old-secret-of-at-least-32-bytes".to_owned())];
    /// assert!(CookieSessionBackend::new(&current, &old, &settings, Arc::new(SystemClock)).is_ok());
    ///
    /// let short = Secret::new("too short".to_owned());
    /// assert_eq!(
    ///     CookieSessionBackend::new(&short, &[], &settings, Arc::new(SystemClock)).unwrap_err(),
    ///     SessionError::InvalidSettings,
    /// );
    /// ```
    pub fn new(
        secret: &Secret<String>,
        old_secrets: &[Secret<String>],
        settings: &SessionSettings,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, SessionError> {
        settings.validate()?;
        let keys = std::iter::once(secret)
            .chain(old_secrets)
            .map(derive_key)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            keys,
            cookie_name: settings.cookie_name.clone(),
            max_cookie_bytes: settings.max_cookie_bytes,
            idle_timeout: settings.idle_timeout,
            absolute_timeout: settings.absolute_timeout,
            clock,
        })
    }

    /// Seals `data` into a cookie value with the current key.
    ///
    /// # Errors
    ///
    /// [`SessionError::TooLarge`] when the encoded value is longer than `max_cookie_bytes`,
    /// [`SessionError::Serialization`] when a time lies before the Unix epoch or the values
    /// cannot be written as JSON, and [`SessionError::Codec`] when encryption fails.
    ///
    /// # Panics
    ///
    /// Panics when the system random number generator fails, because no safe nonce can be
    /// made without it.
    #[allow(dead_code, reason = "the session filter, added next, is the caller")]
    pub(crate) fn seal(&self, data: &SessionData) -> Result<String, SessionError> {
        let payload = PayloadOut {
            v: VERSION,
            c: to_nanos(data.created()).ok_or(SessionError::Serialization)?,
            t: to_nanos(data.touched()).ok_or(SessionError::Serialization)?,
            d: data.values(),
        };
        let plain = serde_json::to_vec(&payload).map_err(|_| SessionError::Serialization)?;
        let value = self.encrypt(plain)?;
        if value.len() > self.max_cookie_bytes {
            return Err(SessionError::TooLarge);
        }
        Ok(value)
    }

    /// Encrypts `plain` with the current key into `base64url(nonce || ciphertext || tag)`.
    fn encrypt(&self, mut plain: Vec<u8>) -> Result<String, SessionError> {
        let mut nonce = [0u8; NONCE_LEN];
        aws_lc_rs::rand::fill(&mut nonce).expect("the system random number generator failed");
        self.keys[0]
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.cookie_name.as_bytes()),
                &mut plain,
            )
            .map_err(|_| SessionError::Codec)?;
        let mut sealed = Vec::with_capacity(NONCE_LEN + plain.len());
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&plain);
        Ok(URL_SAFE_NO_PAD.encode(sealed))
    }

    /// Opens a cookie value, trying the current key and then each old key.
    ///
    /// Returns `None` for a value that is not base64url, is too short, fails authentication
    /// under every key, carries an unknown version or a malformed payload, or whose idle or
    /// absolute timeout has passed. The failure is logged at `debug` with a fixed reason word
    /// and without the value.
    #[allow(dead_code, reason = "the session filter, added next, is the caller")]
    pub(crate) fn open(&self, value: &str) -> Option<SessionData> {
        let Ok(sealed) = URL_SAFE_NO_PAD.decode(value) else {
            return rejected("encoding");
        };
        if sealed.len() < NONCE_LEN + TAG_LEN {
            return rejected("length");
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let Some(plain) = self.decrypt(nonce, ciphertext) else {
            return rejected("authentication");
        };
        match serde_json::from_slice::<VersionProbe>(&plain) {
            Ok(probe) if probe.v == VERSION => {}
            Ok(_) => return rejected("version"),
            Err(_) => return rejected("payload"),
        }
        let Ok(payload) = serde_json::from_slice::<PayloadIn>(&plain) else {
            return rejected("payload");
        };
        let (Some(created), Some(touched)) = (from_nanos(payload.c), from_nanos(payload.t)) else {
            return rejected("payload");
        };
        let now = self.clock.now();
        if has_passed(touched, self.idle_timeout, now)
            || has_passed(created, self.absolute_timeout, now)
        {
            return rejected("expired");
        }
        Some(SessionData::from_parts(payload.d, created, touched))
    }

    /// The plaintext under the first key that authenticates it, or `None`.
    fn decrypt(&self, nonce: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>> {
        self.keys.iter().find_map(|key| {
            let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
            let mut buffer = ciphertext.to_vec();
            let plain = key
                .open_in_place(nonce, Aad::from(self.cookie_name.as_bytes()), &mut buffer)
                .ok()?;
            Some(plain.to_vec())
        })
    }
}

impl fmt::Debug for CookieSessionBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookieSessionBackend")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210";

    /// A clock that only moves when a test moves it.
    struct ManualClock(Mutex<SystemTime>);

    impl ManualClock {
        fn new() -> Arc<Self> {
            Arc::new(Self(Mutex::new(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )))
        }

        fn advance(&self, by: Duration) {
            let mut now = self.0.lock().unwrap();
            *now += by;
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> SystemTime {
            *self.0.lock().unwrap()
        }
    }

    fn secret(s: &str) -> Secret<String> {
        Secret::new(s.to_owned())
    }

    fn backend_with(
        current: &str,
        old: &[&str],
        settings: &SessionSettings,
        clock: Arc<dyn Clock>,
    ) -> CookieSessionBackend {
        let old: Vec<_> = old.iter().map(|s| secret(s)).collect();
        CookieSessionBackend::new(&secret(current), &old, settings, clock).unwrap()
    }

    fn backend(current: &str) -> CookieSessionBackend {
        backend_with(
            current,
            &[],
            &SessionSettings::default(),
            ManualClock::new(),
        )
    }

    fn sample(clock: &dyn Clock) -> SessionData {
        let mut data = SessionData::new(clock.now());
        data.values_mut()
            .insert("user".to_owned(), serde_json::json!("alice-the-tester"));
        data.values_mut()
            .insert("visits".to_owned(), serde_json::json!(3));
        data
    }

    #[test]
    fn seal_open_round_trip() {
        let clock = ManualClock::new();
        let b = backend_with(SECRET, &[], &SessionSettings::default(), clock.clone());
        let mut data = sample(&*clock);
        // A time with a sub-second part survives the round trip exactly.
        data.touch(clock.now() + Duration::from_nanos(123_456_789));
        let sealed = b.seal(&data).unwrap();
        assert_eq!(b.open(&sealed), Some(data));
    }

    #[test]
    fn nonce_differs_per_write() {
        let clock = ManualClock::new();
        let b = backend(SECRET);
        let data = sample(&*clock);
        let first = b.seal(&data).unwrap();
        let second = b.seal(&data).unwrap();
        assert_ne!(first, second);
        let first = URL_SAFE_NO_PAD.decode(first).unwrap();
        let second = URL_SAFE_NO_PAD.decode(second).unwrap();
        assert_ne!(first[..NONCE_LEN], second[..NONCE_LEN]);
    }

    #[test]
    fn flipped_byte_gives_none() {
        let clock = ManualClock::new();
        let b = backend(SECRET);
        let sealed = URL_SAFE_NO_PAD
            .decode(b.seal(&SessionData::new(clock.now())).unwrap())
            .unwrap();
        for position in 0..sealed.len() {
            let mut bytes = sealed.clone();
            bytes[position] ^= 0x01;
            let value = URL_SAFE_NO_PAD.encode(&bytes);
            assert_eq!(b.open(&value), None, "position {position} opened");
        }
    }

    #[test]
    fn swapped_cookie_name_gives_none() {
        let clock = ManualClock::new();
        let a = backend_with(
            SECRET,
            &[],
            &SessionSettings::default().cookie_name("A"),
            clock.clone(),
        );
        let b = backend_with(
            SECRET,
            &[],
            &SessionSettings::default().cookie_name("B"),
            clock.clone(),
        );
        let sealed = a.seal(&sample(&*clock)).unwrap();
        assert!(a.open(&sealed).is_some());
        assert_eq!(b.open(&sealed), None);
    }

    #[test]
    fn wrong_key_gives_none() {
        let clock = ManualClock::new();
        let sealed = backend(SECRET).seal(&sample(&*clock)).unwrap();
        assert_eq!(backend(OTHER).open(&sealed), None);
    }

    #[test]
    fn old_key_still_opens_and_new_writes_use_the_new_key() {
        let clock = ManualClock::new();
        let settings = SessionSettings::default();
        let old = backend_with(OTHER, &[], &settings, clock.clone());
        let rotated = backend_with(SECRET, &[OTHER], &settings, clock.clone());
        let data = sample(&*clock);

        let from_old = old.seal(&data).unwrap();
        assert_eq!(rotated.open(&from_old), Some(data.clone()));

        let from_new = rotated.seal(&data).unwrap();
        assert_eq!(old.open(&from_new), None);
        assert_eq!(backend(SECRET).open(&from_new), Some(data));
    }

    #[test]
    fn too_large_is_rejected_with_toolarge() {
        let clock = ManualClock::new();
        let b = backend_with(
            SECRET,
            &[],
            &SessionSettings::default().max_cookie_bytes(256),
            clock.clone(),
        );
        let mut data = SessionData::new(clock.now());
        data.values_mut()
            .insert("big".to_owned(), serde_json::json!("x".repeat(300)));
        assert_eq!(b.seal(&data), Err(SessionError::TooLarge));
        assert!(b.seal(&SessionData::new(clock.now())).unwrap().len() <= 256);
    }

    #[test]
    fn short_secret_is_rejected() {
        let settings = SessionSettings::default();
        let short = secret(&SECRET[..31]);
        let ok = secret(SECRET);
        assert_eq!(
            CookieSessionBackend::new(&short, &[], &settings, ManualClock::new()).unwrap_err(),
            SessionError::InvalidSettings,
        );
        // An old secret is checked as strictly as the current one.
        assert_eq!(
            CookieSessionBackend::new(&ok, &[short], &settings, ManualClock::new()).unwrap_err(),
            SessionError::InvalidSettings,
        );
    }

    #[test]
    fn exactly_32_bytes_is_accepted() {
        assert_eq!(SECRET.len(), 32);
        let backend = CookieSessionBackend::new(
            &secret(SECRET),
            &[secret(OTHER)],
            &SessionSettings::default(),
            ManualClock::new(),
        );
        assert!(backend.is_ok());
    }

    #[test]
    fn invalid_settings_are_rejected() {
        assert_eq!(
            CookieSessionBackend::new(
                &secret(SECRET),
                &[],
                &SessionSettings::default().cookie_name(""),
                ManualClock::new(),
            )
            .unwrap_err(),
            SessionError::InvalidSettings,
        );
    }

    #[test]
    fn expired_idle_and_absolute_give_none() {
        let settings = SessionSettings::default()
            .idle_timeout(Duration::from_secs(60))
            .absolute_timeout(Duration::from_secs(300));

        // Idle: unused for the whole idle timeout.
        let clock = ManualClock::new();
        let b = backend_with(SECRET, &[], &settings, clock.clone());
        let sealed = b.seal(&sample(&*clock)).unwrap();
        clock.advance(Duration::from_secs(59));
        assert!(b.open(&sealed).is_some());
        clock.advance(Duration::from_secs(1));
        assert_eq!(b.open(&sealed), None);

        // Absolute: touched recently, but created too long ago.
        let clock = ManualClock::new();
        let b = backend_with(SECRET, &[], &settings, clock.clone());
        let mut data = sample(&*clock);
        clock.advance(Duration::from_secs(299));
        data.touch(clock.now());
        let sealed = b.seal(&data).unwrap();
        assert!(b.open(&sealed).is_some());
        clock.advance(Duration::from_secs(1));
        assert_eq!(b.open(&sealed), None);
    }

    #[test]
    fn overflowing_deadline_means_no_limit() {
        let settings = SessionSettings::default()
            .idle_timeout(Duration::MAX)
            .absolute_timeout(Duration::MAX);
        let clock = ManualClock::new();
        let b = backend_with(SECRET, &[], &settings, clock.clone());
        let sealed = b.seal(&sample(&*clock)).unwrap();
        clock.advance(Duration::from_secs(10 * 365 * 24 * 60 * 60));
        assert!(b.open(&sealed).is_some());
    }

    #[test]
    fn garbage_inputs_give_none_not_panic() {
        let b = backend(SECRET);
        let inputs = [
            String::new(),
            "!".to_owned(),
            "A".repeat(10_000),
            URL_SAFE_NO_PAD.encode([7u8; 11]),
            URL_SAFE_NO_PAD.encode([7u8; 12]),
            URL_SAFE_NO_PAD.encode([7u8; NONCE_LEN + TAG_LEN]),
        ];
        for input in &inputs {
            assert_eq!(b.open(input), None);
        }
    }

    #[test]
    fn unknown_version_and_malformed_payload_give_none() {
        // The clock stands at the epoch and every payload is created and touched 1 ns later,
        // so no case can be rejected as expired: each one fails only for the reason it names.
        let early = Arc::new(ManualClock(Mutex::new(SystemTime::UNIX_EPOCH)));
        let b = backend_with(SECRET, &[], &SessionSettings::default(), early);

        // The well-formed payload opens, which shows the clock does not mask the cases below.
        let good = br#"{"v":1,"c":1,"t":1,"d":{}}"#;
        assert!(b.open(&b.encrypt(good.to_vec()).unwrap()).is_some());

        let cases: [(&str, &[u8]); 6] = [
            ("unknown version", br#"{"v":2,"c":1,"t":1,"d":{}}"#),
            ("missing version", br#"{"c":1,"t":1,"d":{}}"#),
            ("not json", b"not json"),
            ("wrong type in c", br#"{"v":1,"c":"x","t":1,"d":{}}"#),
            ("wrong type in d", br#"{"v":1,"c":1,"t":1,"d":[]}"#),
            ("missing d", br#"{"v":1,"c":1,"t":1}"#),
        ];
        for (name, plain) in cases {
            let value = b.encrypt(plain.to_vec()).unwrap();
            assert_eq!(b.open(&value), None, "the {name} case opened");
        }
    }

    #[test]
    fn debug_shows_no_key_or_value() {
        let b = backend(SECRET);
        let text = format!("{b:?}");
        assert_eq!(text, "CookieSessionBackend { .. }");
        assert!(!text.contains(SECRET));
    }

    #[test]
    fn no_plaintext_in_the_cookie() {
        let clock = ManualClock::new();
        let b = backend(SECRET);
        let sealed = b.seal(&sample(&*clock)).unwrap();
        assert!(!sealed.contains("alice-the-tester"));
        let bytes = URL_SAFE_NO_PAD.decode(&sealed).unwrap();
        let needle = b"alice-the-tester";
        assert!(!bytes.windows(needle.len()).any(|w| w == needle));
        assert!(!bytes.windows(4).any(|w| w == b"user"));
    }
}
