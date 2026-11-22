//! The remember-me token: `v1.<base64url(username)>.<expiry seconds>.<base64url(HMAC-SHA256)>`.
//!
//! The MAC covers an unambiguous encoding of the version tag `v1`, the username with its length
//! in front, the expiry in seconds since the Unix epoch and the stored password hash with its
//! length in front. The lengths and the expiry are 8 big-endian bytes each, so no username, `.`
//! and `:` included, can shift bytes between the fields. A changed password hash changes the MAC
//! input, so every earlier token fails.
//!
//! The HMAC key is derived from each secret with HKDF-SHA256 (empty salt) and the fixed info
//! string `oxbloom remember-me v1`, so it never equals the session cookie key even for the same
//! secret. Verification is constant time and tries the current key first, then each old key.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_lc_rs::hkdf::{HKDF_SHA256, Salt};
use aws_lc_rs::hmac;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oxbloom_config::Secret;
use oxbloom_core::Clock;

use super::settings::RememberMe;
use crate::http_security::SecurityBuildError;

/// The version tag, both the first token part and the first bytes of the MAC input.
const VERSION: &str = "v1";
/// The HKDF info string that binds a derived key to remember-me and this format version.
const KEY_INFO: &[u8] = b"oxbloom remember-me v1";
/// The longest token `parse` looks at, in bytes.
const MAX_TOKEN_BYTES: usize = 1024;
/// The length of an HMAC-SHA256 tag.
const MAC_BYTES: usize = 32;

/// A token whose format is valid. Its MAC is not checked yet; see [`TokenCodec::verify`].
#[derive(Clone)]
pub(crate) struct ParsedToken {
    username: String,
    expires: u64,
    mac: [u8; MAC_BYTES],
}

#[allow(dead_code, reason = "used by the auto login of the chain")]
impl ParsedToken {
    /// The username the token names. It is not authenticated until the MAC verifies.
    pub(crate) fn username(&self) -> &str {
        &self.username
    }

    /// The expiry the token names.
    pub(crate) fn expires(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.expires)
    }
}

impl fmt::Debug for ParsedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ParsedToken(..)")
    }
}

/// Encodes, parses and verifies remember-me tokens with the keys of one [`RememberMe`].
pub(crate) struct TokenCodec {
    /// The current key first, then the old keys in their configured order.
    keys: Vec<hmac::Key>,
    clock: Arc<dyn Clock>,
}

#[allow(
    dead_code,
    reason = "used by the cookie and the auto login of the chain"
)]
impl TokenCodec {
    /// Derives the keys of validated settings.
    ///
    /// # Errors
    ///
    /// [`SecurityBuildError::InvalidRememberMe`] when the settings have no secret.
    pub(crate) fn new(settings: &RememberMe) -> Result<Self, SecurityBuildError> {
        let current = settings
            .secret
            .as_ref()
            .ok_or(SecurityBuildError::InvalidRememberMe)?;
        let keys = std::iter::once(current)
            .chain(&settings.old_secrets)
            .map(derive_key)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            keys,
            clock: settings.clock.clone(),
        })
    }

    /// The token for this user and expiry, signed with the current key.
    pub(crate) fn encode(
        &self,
        username: &str,
        password_hash: &str,
        expires: SystemTime,
    ) -> String {
        let expires = epoch_seconds(expires);
        let tag = hmac::sign(&self.keys[0], &mac_input(username, expires, password_hash));
        format!(
            "{VERSION}.{}.{expires}.{}",
            URL_SAFE_NO_PAD.encode(username),
            URL_SAFE_NO_PAD.encode(tag.as_ref())
        )
    }

    /// Checks the format only: four parts separated by `.`, the version `v1`, a base64url
    /// username that is UTF-8, an expiry of decimal digits without a leading zero that fits a
    /// `u64`, and a base64url MAC of 32 bytes. A token over 1 KiB is refused unread. Never panics.
    pub(crate) fn parse(&self, token: &str) -> Option<ParsedToken> {
        if token.len() > MAX_TOKEN_BYTES {
            return None;
        }
        let mut parts = token.split('.');
        let (version, username, expires, mac) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || version != VERSION {
            return None;
        }
        let username = String::from_utf8(URL_SAFE_NO_PAD.decode(username).ok()?).ok()?;
        let canonical = expires.bytes().all(|b| b.is_ascii_digit())
            && (expires == "0" || !expires.starts_with('0'));
        if !canonical {
            return None;
        }
        let expires = expires.parse::<u64>().ok()?;
        let mac = <[u8; MAC_BYTES]>::try_from(URL_SAFE_NO_PAD.decode(mac).ok()?).ok()?;
        Some(ParsedToken {
            username,
            expires,
            mac,
        })
    }

    /// Whether the token is unexpired at the time of the clock and its MAC verifies, in constant
    /// time, for this password hash under the current key or one of the old keys. The answer
    /// never says which check failed.
    pub(crate) fn verify(&self, parsed: &ParsedToken, password_hash: &str) -> bool {
        let input = mac_input(&parsed.username, parsed.expires, password_hash);
        let mac_ok = self
            .keys
            .iter()
            .any(|key| hmac::verify(key, &input, &parsed.mac).is_ok());
        let unexpired = parsed.expires > epoch_seconds(self.clock.now());
        mac_ok && unexpired
    }
}

impl fmt::Debug for TokenCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenCodec")
            .field("keys", &self.keys.len())
            .finish_non_exhaustive()
    }
}

/// The HMAC-SHA256 key for one secret.
fn derive_key(secret: &Secret<String>) -> Result<hmac::Key, SecurityBuildError> {
    let prk = Salt::new(HKDF_SHA256, &[]).extract(secret.expose().as_bytes());
    let info = [KEY_INFO];
    let okm = prk
        .expand(&info, hmac::HMAC_SHA256)
        .map_err(|_| SecurityBuildError::InvalidRememberMe)?;
    Ok(hmac::Key::from(okm))
}

/// Whole seconds since the Unix epoch; a time before the epoch counts as zero.
fn epoch_seconds(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The bytes the MAC covers: `v1`, the username length and bytes, the expiry, the password hash
/// length and bytes. Lengths and the expiry are 8 big-endian bytes.
fn mac_input(username: &str, expires: u64, password_hash: &str) -> Vec<u8> {
    let mut input = Vec::with_capacity(VERSION.len() + 24 + username.len() + password_hash.len());
    input.extend_from_slice(VERSION.as_bytes());
    input.extend_from_slice(&(username.len() as u64).to_be_bytes());
    input.extend_from_slice(username.as_bytes());
    input.extend_from_slice(&expires.to_be_bytes());
    input.extend_from_slice(&(password_hash.len() as u64).to_be_bytes());
    input.extend_from_slice(password_hash.as_bytes());
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "a-remember-me-secret-of-32-bytes";
    const OTHER: &str = "another-remember-me-secret-32-by";
    const HASH: &str = "$argon2id$v=19$m=8,t=1,p=1$c2FsdA$aGFzaA";
    const NOW: u64 = 1_800_000_000;

    struct Fixed(SystemTime);

    impl Clock for Fixed {
        fn now(&self) -> SystemTime {
            self.0
        }
    }

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn codec_with(secret: &str, old: &[&str]) -> TokenCodec {
        let settings = RememberMe::new()
            .secret(Secret::new(secret.to_owned()))
            .old_secrets(old.iter().map(|s| Secret::new((*s).to_owned())).collect())
            .clock(Arc::new(Fixed(at(NOW))));
        TokenCodec::new(&settings).unwrap()
    }

    fn codec() -> TokenCodec {
        codec_with(SECRET, &[])
    }

    fn later() -> SystemTime {
        at(NOW + 3600)
    }

    fn accepted(codec: &TokenCodec, token: &str, hash: &str) -> bool {
        codec.parse(token).is_some_and(|p| codec.verify(&p, hash))
    }

    fn parts(token: &str) -> Vec<String> {
        token.split('.').map(str::to_owned).collect()
    }

    #[test]
    fn a_token_round_trips() {
        let c = codec();
        let token = c.encode("alice", HASH, later());
        let p = parts(&token);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], "v1");
        assert_eq!(p[1], URL_SAFE_NO_PAD.encode("alice"));
        assert_eq!(p[2], (NOW + 3600).to_string());
        assert_eq!(URL_SAFE_NO_PAD.decode(&p[3]).unwrap().len(), 32);
        let parsed = c.parse(&token).unwrap();
        assert_eq!(parsed.username(), "alice");
        assert_eq!(parsed.expires(), later());
        assert!(c.verify(&parsed, HASH));
        assert!(!token.contains(SECRET));
    }

    #[test]
    fn each_tampered_part_is_rejected() {
        let c = codec();
        let token = c.encode("alice", HASH, later());
        let p = parts(&token);
        let with = |i: usize, v: &str| {
            let mut q = p.clone();
            q[i] = v.to_owned();
            q.join(".")
        };
        let mut mac = URL_SAFE_NO_PAD.decode(&p[3]).unwrap();
        mac[0] ^= 1;
        let tampered = [
            with(0, "v2"),
            with(1, &URL_SAFE_NO_PAD.encode("alicf")),
            with(1, &URL_SAFE_NO_PAD.encode("mallory")),
            with(2, &(NOW + 3601).to_string()),
            with(2, &(NOW + 999_999).to_string()),
            with(3, &URL_SAFE_NO_PAD.encode(&mac)),
        ];
        assert!(accepted(&c, &token, HASH));
        for t in tampered {
            assert!(!accepted(&c, &t, HASH), "{t}");
        }
    }

    #[test]
    fn a_tampered_username_or_expiry_still_parses_but_fails_verification() {
        let c = codec();
        let p = parts(&c.encode("alice", HASH, later()));
        let forged = format!("v1.{}.{}.{}", URL_SAFE_NO_PAD.encode("root"), p[2], p[3]);
        let parsed = c.parse(&forged).expect("the format is valid");
        assert!(!c.verify(&parsed, HASH));
        let forged = format!("v1.{}.{}.{}", p[1], NOW + 7200, p[3]);
        let parsed = c.parse(&forged).expect("the format is valid");
        assert!(!c.verify(&parsed, HASH));
    }

    #[test]
    fn an_expired_token_is_rejected() {
        let c = codec();
        assert!(!accepted(&c, &c.encode("alice", HASH, at(NOW - 1)), HASH));
        assert!(!accepted(&c, &c.encode("alice", HASH, at(NOW)), HASH));
        assert!(accepted(&c, &c.encode("alice", HASH, at(NOW + 1)), HASH));
        assert!(!accepted(
            &c,
            &c.encode("alice", HASH, SystemTime::UNIX_EPOCH),
            HASH
        ));
    }

    #[test]
    fn a_token_of_another_key_is_rejected() {
        let token = codec_with(OTHER, &[]).encode("alice", HASH, later());
        assert!(!accepted(&codec(), &token, HASH));
    }

    #[test]
    fn the_same_secret_gives_a_key_other_than_the_session_key() {
        // The session cookie key uses another info string; the derived keys must differ.
        let session = Salt::new(HKDF_SHA256, &[]).extract(SECRET.as_bytes());
        let info = [&b"oxbloom session cookie v1"[..]];
        let session_key = hmac::Key::from(session.expand(&info, hmac::HMAC_SHA256).unwrap());
        let c = codec();
        let input = mac_input("alice", NOW + 3600, HASH);
        let ours = hmac::sign(&c.keys[0], &input);
        assert!(hmac::verify(&session_key, &input, ours.as_ref()).is_err());
    }

    #[test]
    fn an_old_key_is_accepted_after_a_rotation() {
        let before = codec_with(SECRET, &[]).encode("alice", HASH, later());
        let rotated = codec_with(OTHER, &["a-third-remember-me-secret-32-byt", SECRET]);
        assert!(accepted(&rotated, &before, HASH));
        let after = rotated.encode("alice", HASH, later());
        assert_ne!(after, before, "new tokens use the current key");
        assert!(accepted(&rotated, &after, HASH));
        assert!(!accepted(&codec(), &after, HASH));
        // Once the old secret is dropped, its tokens fail.
        assert!(!accepted(&codec_with(OTHER, &[]), &before, HASH));
    }

    #[test]
    fn a_changed_password_hash_is_rejected() {
        let c = codec();
        let token = c.encode("alice", HASH, later());
        assert!(!accepted(
            &c,
            &token,
            "$argon2id$v=19$m=8,t=1,p=1$c2FsdA$bmV3"
        ));
        assert!(!accepted(&c, &token, ""));
    }

    #[test]
    fn any_username_round_trips() {
        let c = codec();
        for name in [
            "a.b",
            "a:b",
            "a b",
            "dot.and:colon",
            "üñî",
            "🦀 crab",
            "",
            "v1.x.1.y",
        ] {
            let token = c.encode(name, HASH, later());
            assert_eq!(parts(&token).len(), 4, "{name}");
            let parsed = c.parse(&token).expect(name);
            assert_eq!(parsed.username(), name);
            assert!(c.verify(&parsed, HASH), "{name}");
        }
    }

    #[test]
    fn bytes_cannot_shift_between_the_fields() {
        let c = codec();
        let splice = |a: &str, b: &str| {
            let (pa, pb) = (parts(a), parts(b));
            format!("{}.{}.{}.{}", pa[0], pa[1], pa[2], pb[3])
        };
        // Username against password hash.
        let a = c.encode("a", "bc", later());
        let b = c.encode("ab", "c", later());
        assert_ne!(parts(&a)[3], parts(&b)[3]);
        assert!(!accepted(&c, &splice(&a, &b), "bc"));
        // Username against expiry.
        let a = c.encode("a1", HASH, at(NOW + 23));
        let b = c.encode("a", HASH, at(NOW + 123));
        assert_ne!(parts(&a)[3], parts(&b)[3]);
        assert!(!accepted(&c, &splice(&a, &b), HASH));
    }

    #[test]
    fn the_mac_input_is_length_prefixed() {
        let mut expected = b"v1".to_vec();
        expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 2]);
        expected.extend_from_slice(b"ab");
        expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 1, 0]);
        expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        expected.extend_from_slice(b"h");
        assert_eq!(mac_input("ab", 256, "h"), expected);
    }

    #[test]
    fn a_token_over_one_kib_is_refused() {
        let c = codec();
        // A username that keeps the token at exactly 1024 bytes parses; one byte more does not.
        let fixed = "v1...".len() + (NOW + 3600).to_string().len() + 43;
        let fits = (MAX_TOKEN_BYTES - fixed) / 4 * 3;
        let token = c.encode(&"u".repeat(fits), HASH, later());
        assert!(token.len() <= MAX_TOKEN_BYTES);
        assert!(accepted(&c, &token, HASH));
        let token = c.encode(&"u".repeat(fits + 3), HASH, later());
        assert!(token.len() > MAX_TOKEN_BYTES);
        assert!(c.parse(&token).is_none());
        assert!(c.parse(&"v".repeat(MAX_TOKEN_BYTES + 1)).is_none());
        assert!(c.parse(&"x".repeat(1 << 20)).is_none());
    }

    #[test]
    fn a_malformed_token_never_parses_and_never_panics() {
        let c = codec();
        let good = parts(&c.encode("alice", HASH, later()));
        let mac = &good[3];
        let user = &good[1];
        let inputs = [
            String::new(),
            ".".to_owned(),
            "...".to_owned(),
            "v1...".to_owned(),
            "v1....".to_owned(),
            "v1".to_owned(),
            "ü.ö.ä.ß".to_owned(),
            "🦀".repeat(300),
            format!("V1.{user}.1.{mac}"),
            format!("v1.{user}.1.{mac}.extra"),
            format!("v1.{user}..{mac}"),
            format!("v1.{user}.+1.{mac}"),
            format!("v1.{user}.-1.{mac}"),
            format!("v1.{user}.01.{mac}"),
            format!("v1.{user}.1 .{mac}"),
            format!("v1.{user}.18446744073709551616.{mac}"),
            format!("v1.{user}.१२.{mac}"),
            format!("v1.{user}=.1.{mac}"),
            format!("v1.a+b/.1.{mac}"),
            format!("v1.{}.1.{mac}", URL_SAFE_NO_PAD.encode([0xff, 0xfe])),
            format!("v1.{user}.1.{}", URL_SAFE_NO_PAD.encode([0u8; 31])),
            format!("v1.{user}.1.{}", URL_SAFE_NO_PAD.encode([0u8; 33])),
            format!("v1.{user}.1.{mac}="),
            format!("v1.{user}.1."),
        ];
        for input in &inputs {
            assert!(c.parse(input).is_none(), "{input}");
        }
        // The largest expiry still parses.
        assert!(
            c.parse(&format!("v1.{user}.18446744073709551615.{mac}"))
                .is_some()
        );
        assert!(c.parse(&format!("v1.{user}.0.{mac}")).is_some());
    }

    #[test]
    fn debug_shows_no_key_or_username() {
        let c = codec();
        let parsed = c.parse(&c.encode("alice", HASH, later())).unwrap();
        assert_eq!(format!("{parsed:?}"), "ParsedToken(..)");
        let dbg = format!("{c:?}");
        assert!(dbg.contains("keys: 1") && !dbg.contains(SECRET), "{dbg}");
    }
}
