use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::header;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, Jwk, JwkSet, KeyAlgorithm,
    OctetKeyParameters, OctetKeyType, PublicKeyUse,
};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
use oxbloom_web::{App, HttpMethod, Request, Route};
use serde_json::json;
use tokio::time::{advance, sleep};

use super::*;
use crate::{
    AuthenticationError, FailureReason, HttpSecurity, JwtDecoder, JwtDecoderBuilder,
    RequestSecurityExt,
};

const RSA_PRIVATE: &str = include_str!("../../tests/fixtures/rsa_private.pem");
const RSA_PUBLIC: &str = include_str!("../../tests/fixtures/rsa_public.pem");
const EC_PUBLIC: &str = include_str!("../../tests/fixtures/ec_public.pem");
const URI: &str = "https://issuer.example/jwks.json";
const TTL: Duration = Duration::from_secs(300);
const INTERVAL: Duration = Duration::from_secs(30);
const RS: &[Algorithm] = &[Algorithm::RS256];

/// In-memory fetcher that counts calls, serves a swappable result and can be slowed down.
struct MemFetcher {
    result: Mutex<Result<JwkSet, FetchError>>,
    calls: AtomicUsize,
    delay: Mutex<Option<Duration>>,
    hang: Mutex<bool>,
    panic: Mutex<bool>,
}

impl MemFetcher {
    fn new(result: Result<JwkSet, FetchError>) -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(result),
            calls: AtomicUsize::new(0),
            delay: Mutex::new(None),
            hang: Mutex::new(false),
            panic: Mutex::new(false),
        })
    }

    fn serve(&self, result: Result<JwkSet, FetchError>) {
        *self.result.lock().unwrap() = result;
    }

    fn slow(&self, delay: Duration) {
        *self.delay.lock().unwrap() = Some(delay);
    }

    fn panic(&self, on: bool) {
        *self.panic.lock().unwrap() = on;
    }

    fn hang(&self) {
        *self.hang.lock().unwrap() = true;
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl JwksFetcher for MemFetcher {
    async fn fetch(&self, uri: &str) -> Result<JwkSet, FetchError> {
        assert_eq!(uri, URI);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let delay = *self.delay.lock().unwrap();
        if let Some(delay) = delay {
            sleep(delay).await;
        }
        let panic = *self.panic.lock().unwrap();
        assert!(!panic, "test fetcher panics on purpose");
        let hang = *self.hang.lock().unwrap();
        if hang {
            std::future::pending::<()>().await;
        }
        self.result.lock().unwrap().clone()
    }
}

/// Yields until the fetcher has been called `n` times; panics instead of spinning forever.
async fn wait_for_calls(fetcher: &MemFetcher, n: usize) {
    for _ in 0..10_000 {
        if fetcher.calls() >= n {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("fetcher was called {} times, expected {n}", fetcher.calls());
}

fn rsa_jwk(kid: &str) -> Jwk {
    let key = DecodingKey::from_rsa_pem(RSA_PUBLIC.as_bytes()).unwrap();
    let mut jwk = Jwk::from_decoding_key(&key, Some(Algorithm::RS256)).unwrap();
    jwk.common.key_id = Some(kid.to_string());
    jwk.common.public_key_use = Some(PublicKeyUse::Signature);
    jwk
}

fn ec_jwk(kid: &str) -> Jwk {
    let key = DecodingKey::from_ec_pem(EC_PUBLIC.as_bytes()).unwrap();
    let mut jwk = Jwk::from_decoding_key(&key, Some(Algorithm::ES256)).unwrap();
    jwk.common.key_id = Some(kid.to_string());
    jwk
}

fn set(keys: impl IntoIterator<Item = Jwk>) -> JwkSet {
    JwkSet {
        keys: keys.into_iter().collect(),
    }
}

fn source(fetcher: &Arc<MemFetcher>) -> Arc<JwksSource> {
    JwksSource::builder(fetcher.clone(), URI).build().unwrap()
}

fn kid_of(result: Result<Jwk, KeyLookupError>) -> Result<String, KeyLookupError> {
    result.map(|jwk| jwk.common.key_id.unwrap())
}

#[tokio::test(start_paused = true)]
async fn cache_hit_does_not_refetch() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a"), rsa_jwk("b")])));
    let source = source(&fetcher);
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    advance(Duration::from_secs(120)).await;
    assert_eq!(kid_of(source.key_for("b", RS).await), Ok("b".into()));
    advance(TTL - Duration::from_secs(121)).await;
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    assert_eq!(fetcher.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn ttl_expiry_refetches() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    let source = source(&fetcher);
    source.key_for("a", RS).await.unwrap();
    advance(TTL - Duration::from_secs(1)).await;
    source.key_for("a", RS).await.unwrap();
    assert_eq!(fetcher.calls(), 1);
    advance(Duration::from_secs(1)).await;
    source.key_for("a", RS).await.unwrap();
    assert_eq!(fetcher.calls(), 2, "expired at exactly the TTL");
    source.key_for("a", RS).await.unwrap();
    assert_eq!(fetcher.calls(), 2, "fresh again");
}

#[tokio::test(start_paused = true)]
async fn ttl_is_configurable() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    let source = JwksSource::builder(fetcher.clone(), URI)
        .ttl(Duration::from_secs(60))
        .build()
        .unwrap();
    source.key_for("a", RS).await.unwrap();
    advance(Duration::from_secs(60)).await;
    source.key_for("a", RS).await.unwrap();
    assert_eq!(fetcher.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn unknown_kid_refreshes_once_and_finds_rotated_key() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("old")])));
    let source = source(&fetcher);
    source.key_for("old", RS).await.unwrap();
    fetcher.serve(Ok(set([rsa_jwk("old"), rsa_jwk("new")])));

    // Within the refresh interval of the first fetch the new key is not looked for yet.
    assert_eq!(
        source.key_for("new", RS).await.unwrap_err(),
        KeyLookupError::UnknownKid
    );
    assert_eq!(fetcher.calls(), 1);

    advance(INTERVAL).await;
    assert_eq!(kid_of(source.key_for("new", RS).await), Ok("new".into()));
    assert_eq!(fetcher.calls(), 2, "one refresh found the rotated key");
    assert_eq!(kid_of(source.key_for("new", RS).await), Ok("new".into()));
    assert_eq!(kid_of(source.key_for("old", RS).await), Ok("old".into()));
    assert_eq!(fetcher.calls(), 2);

    // A kid that is still unknown after the refresh does not fetch again right away.
    assert_eq!(
        source.key_for("missing", RS).await.unwrap_err(),
        KeyLookupError::UnknownKid
    );
    assert_eq!(fetcher.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn random_kids_cause_at_most_one_fetch_per_interval() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    let source = source(&fetcher);
    source.key_for("a", RS).await.unwrap();
    advance(INTERVAL + Duration::from_secs(1)).await;

    for i in 0..1_000 {
        let err = source
            .key_for(&format!("random-{i}"), RS)
            .await
            .unwrap_err();
        assert_eq!(err, KeyLookupError::UnknownKid);
        if i % 100 == 0 {
            advance(Duration::from_millis(2_990)).await; // 10 x 2.99 s = 29.9 s in total
        }
    }
    assert_eq!(
        fetcher.calls(),
        2,
        "1,000 random kids within 30 s: one fetch"
    );

    advance(Duration::from_millis(100)).await; // now exactly 30 s since the second fetch
    source.key_for("random-x", RS).await.unwrap_err();
    assert_eq!(fetcher.calls(), 3);
    source.key_for("a", RS).await.unwrap();
    assert_eq!(fetcher.calls(), 3);
}

#[tokio::test(start_paused = true)]
async fn concurrent_lookups_share_one_fetch() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    fetcher.slow(Duration::from_secs(1));
    let source = source(&fetcher);

    let tasks: Vec<_> = (0..100)
        .map(|_| {
            let source = source.clone();
            tokio::spawn(async move { source.key_for("a", RS).await })
        })
        .collect();
    for task in tasks {
        assert_eq!(kid_of(task.await.unwrap()), Ok("a".into()));
    }
    assert_eq!(fetcher.calls(), 1, "empty cache: one shared fetch");

    // Unknown kids arriving together after the interval also share one refresh.
    advance(INTERVAL).await;
    let tasks: Vec<_> = (0..100)
        .map(|i| {
            let source = source.clone();
            tokio::spawn(async move { source.key_for(&format!("x-{i}"), RS).await })
        })
        .collect();
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap_err(), KeyLookupError::UnknownKid);
    }
    assert_eq!(fetcher.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn concurrent_lookups_share_one_fetch_even_without_rate_limit() {
    // With the rate limit off, waiters still reuse the fetch that finished while they waited.
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    fetcher.slow(Duration::from_secs(1));
    let source = JwksSource::builder(fetcher.clone(), URI)
        .min_refresh_interval(Duration::ZERO)
        .build()
        .unwrap();
    let tasks: Vec<_> = (0..50)
        .map(|i| {
            let source = source.clone();
            tokio::spawn(async move { source.key_for(&format!("x-{i}"), RS).await })
        })
        .collect();
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap_err(), KeyLookupError::UnknownKid);
    }
    assert_eq!(fetcher.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn slow_refresh_does_not_block_fresh_cache_readers() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    let source = source(&fetcher);
    source.key_for("a", RS).await.unwrap();
    advance(INTERVAL).await;

    fetcher.slow(Duration::from_secs(4));
    let refreshing = {
        let source = source.clone();
        tokio::spawn(async move { source.key_for("unknown", RS).await })
    };
    wait_for_calls(&fetcher, 2).await;
    let before = Instant::now();
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    assert_eq!(
        Instant::now(),
        before,
        "answered without waiting for the fetch"
    );
    assert!(!refreshing.is_finished());
    assert_eq!(
        refreshing.await.unwrap().unwrap_err(),
        KeyLookupError::UnknownKid
    );
    assert_eq!(fetcher.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn refresh_failure_serves_stale_within_twice_ttl() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    let source = source(&fetcher);
    source.key_for("a", RS).await.unwrap();
    fetcher.serve(Err(FetchError::Network));

    advance(TTL + Duration::from_secs(1)).await;
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    assert_eq!(fetcher.calls(), 2, "refresh attempted and failed");

    advance(Duration::from_secs(10)).await;
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    assert_eq!(fetcher.calls(), 2, "failed refresh is rate-limited");
    assert_eq!(
        source.key_for("other", RS).await.unwrap_err(),
        KeyLookupError::UnknownKid
    );

    // Exactly twice the TTL old is still served.
    advance(2 * TTL - TTL - Duration::from_secs(11)).await;
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    assert_eq!(fetcher.calls(), 3);

    // A refresh that succeeds again replaces the stale set.
    fetcher.serve(Ok(set([rsa_jwk("b")])));
    advance(INTERVAL).await;
    assert_eq!(kid_of(source.key_for("b", RS).await), Ok("b".into()));
    assert_eq!(fetcher.calls(), 4);
}

#[tokio::test(start_paused = true)]
async fn stale_older_than_twice_ttl_fails_closed_as_unavailable() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    let source = source(&fetcher);
    source.key_for("a", RS).await.unwrap();
    fetcher.serve(Err(FetchError::Timeout));

    advance(2 * TTL + Duration::from_secs(1)).await;
    assert_eq!(
        source.key_for("a", RS).await.unwrap_err(),
        KeyLookupError::Unavailable
    );
    assert_eq!(
        source.key_for("other", RS).await.unwrap_err(),
        KeyLookupError::Unavailable
    );
    assert_eq!(fetcher.calls(), 2);

    // The issuer recovers, but the next attempt waits for the rate limit.
    fetcher.serve(Ok(set([rsa_jwk("a")])));
    advance(INTERVAL - Duration::from_secs(1)).await;
    assert_eq!(
        source.key_for("a", RS).await.unwrap_err(),
        KeyLookupError::Unavailable
    );
    assert_eq!(fetcher.calls(), 2);
    advance(Duration::from_secs(1)).await;
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    assert_eq!(fetcher.calls(), 3);
}

#[tokio::test(start_paused = true)]
async fn failed_refresh_is_rate_limited() {
    // Empty cache and a failing issuer: one attempt per interval, Unavailable in between.
    let fetcher = MemFetcher::new(Err(FetchError::BadStatus));
    let source = source(&fetcher);
    for _ in 0..50 {
        assert_eq!(
            source.key_for("a", RS).await.unwrap_err(),
            KeyLookupError::Unavailable
        );
    }
    assert_eq!(fetcher.calls(), 1);
    advance(INTERVAL - Duration::from_millis(1)).await;
    source.key_for("a", RS).await.unwrap_err();
    assert_eq!(fetcher.calls(), 1);
    advance(Duration::from_millis(1)).await;
    source.key_for("a", RS).await.unwrap_err();
    assert_eq!(fetcher.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn hanging_fetch_times_out_and_is_rate_limited() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    fetcher.hang();
    let source = JwksSource::builder(fetcher.clone(), URI)
        .fetch_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let start = Instant::now();
    assert_eq!(
        source.key_for("a", RS).await.unwrap_err(),
        KeyLookupError::Unavailable
    );
    assert_eq!(start.elapsed(), Duration::from_secs(2));
    // The timed-out attempt counts for the rate limit, so callers do not pile up on the issuer.
    for _ in 0..10 {
        source.key_for("a", RS).await.unwrap_err();
    }
    assert_eq!(fetcher.calls(), 1);
    assert_eq!(
        start.elapsed(),
        Duration::from_secs(2),
        "no further waiting"
    );
}

#[tokio::test(start_paused = true)]
async fn aborted_caller_does_not_abort_fetch() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    fetcher.slow(Duration::from_secs(1));
    let source = source(&fetcher);
    let lookup = {
        let source = source.clone();
        tokio::spawn(async move { source.key_for("a", RS).await })
    };
    wait_for_calls(&fetcher, 1).await;
    lookup.abort();
    assert!(lookup.await.unwrap_err().is_cancelled());

    // The fetch keeps running without its caller and fills the cache.
    sleep(Duration::from_secs(2)).await;
    fetcher.slow(Duration::from_secs(100)); // a new fetch now would be visible as a delay
    let start = Instant::now();
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    assert_eq!(Instant::now(), start, "cache hit");
    assert_eq!(fetcher.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn aborted_caller_does_not_burn_rate_limit_slot_without_a_fetch_completing() {
    // Cold start: an attacker starts the first lookup and disconnects at once. A later
    // lookup joins the still-running fetch instead of waiting out the 30 s rate limit.
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    fetcher.slow(Duration::from_secs(1));
    let source = source(&fetcher);
    let start = Instant::now();
    for _ in 0..5 {
        let lookup = {
            let source = source.clone();
            tokio::spawn(async move { source.key_for("random", RS).await })
        };
        // The first lookup starts the fetch; later ones join it. Each is aborted only after
        // it has run, i.e. while it waits on the fetch.
        wait_for_calls(&fetcher, 1).await;
        tokio::task::yield_now().await;
        assert!(!lookup.is_finished());
        lookup.abort();
        assert!(lookup.await.unwrap_err().is_cancelled());
    }
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    assert_eq!(fetcher.calls(), 1);
    assert!(
        start.elapsed() <= Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );

    // Warm cache: aborted refresh callers still let the refresh land (rotation is found).
    fetcher.serve(Ok(set([rsa_jwk("a"), rsa_jwk("b")])));
    advance(INTERVAL).await;
    let lookup = {
        let source = source.clone();
        tokio::spawn(async move { source.key_for("b", RS).await })
    };
    wait_for_calls(&fetcher, 2).await;
    lookup.abort();
    assert!(lookup.await.unwrap_err().is_cancelled());
    assert_eq!(kid_of(source.key_for("b", RS).await), Ok("b".into()));
    assert_eq!(fetcher.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn panicking_fetcher_is_a_failed_attempt_not_a_hang() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    fetcher.panic(true);
    let source = source(&fetcher);
    let start = Instant::now();
    assert_eq!(
        source.key_for("a", RS).await.unwrap_err(),
        KeyLookupError::Unavailable
    );
    // Later lookups within the rate-limit interval start no new attempt and get the same
    // defined answer.
    let tasks: Vec<_> = (0..10)
        .map(|_| {
            let source = source.clone();
            tokio::spawn(async move { source.key_for("a", RS).await })
        })
        .collect();
    for task in tasks {
        assert_eq!(
            task.await.unwrap().unwrap_err(),
            KeyLookupError::Unavailable
        );
    }
    assert_eq!(
        fetcher.calls(),
        1,
        "a panicked attempt still counts for the rate limit"
    );
    assert_eq!(start.elapsed(), Duration::ZERO, "no hang");

    // Nothing is stuck or poisoned: the next attempt after the interval works.
    fetcher.panic(false);
    advance(INTERVAL).await;
    assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
    assert_eq!(fetcher.calls(), 2);
}

#[test]
fn non_https_rejected_unless_allowed() {
    let fetcher = MemFetcher::new(Ok(set([])));
    let build = |uri: &str, insecure: bool| {
        let builder = JwksSource::builder(fetcher.clone(), uri);
        let builder = if insecure {
            builder.allow_insecure_http()
        } else {
            builder
        };
        builder.build().map(|_| ())
    };
    assert_eq!(build(URI, false), Ok(()));
    assert_eq!(
        build("http://127.0.0.1:8080/jwks", false),
        Err(JwksError::InsecureUri)
    );
    assert_eq!(
        build("HTTP://issuer.example/jwks", false),
        Err(JwksError::InsecureUri)
    );
    assert_eq!(build("http://127.0.0.1:8080/jwks", true), Ok(()));
    assert_eq!(build("http://localhost/jwks", true), Ok(()));
    for bad in [
        "ftp://issuer.example/jwks",
        "issuer.example/jwks",
        "",
        "https://",
        "not a uri",
    ] {
        assert_eq!(build(bad, false), Err(JwksError::InvalidUri), "{bad:?}");
        assert_eq!(build(bad, true), Err(JwksError::InvalidUri), "{bad:?}");
    }
    let zero_ttl = JwksSource::builder(fetcher.clone(), URI)
        .ttl(Duration::ZERO)
        .build()
        .map(|_| ());
    assert_eq!(zero_ttl, Err(JwksError::ZeroTtl));
    let zero_timeout = JwksSource::builder(fetcher.clone(), URI)
        .fetch_timeout(Duration::ZERO)
        .build()
        .map(|_| ());
    assert_eq!(zero_timeout, Err(JwksError::ZeroTimeout));
}

#[tokio::test(start_paused = true)]
async fn key_with_disallowed_alg_or_kty_is_ignored() {
    let mut rs384 = rsa_jwk("rs384");
    rs384.common.key_algorithm = Some(KeyAlgorithm::RS384);
    let mut ps256 = rsa_jwk("ps256");
    ps256.common.key_algorithm = Some(KeyAlgorithm::PS256);
    let mut unknown_alg = rsa_jwk("unknown-alg");
    unknown_alg.common.key_algorithm = Some(KeyAlgorithm::UNKNOWN_ALGORITHM);
    let mut rsa_no_alg = rsa_jwk("rsa-no-alg");
    rsa_no_alg.common.key_algorithm = None;
    // An RSA key that claims an EC algorithm.
    let mut rsa_claims_es = rsa_jwk("rsa-claims-es");
    rsa_claims_es.common.key_algorithm = Some(KeyAlgorithm::ES256);
    // A P-256 key that claims ES384.
    let mut ec_wrong_curve = ec_jwk("ec-wrong-curve");
    ec_wrong_curve.common.key_algorithm = Some(KeyAlgorithm::ES384);
    let mut ec_p384 = ec_jwk("ec-p384");
    ec_p384.common.key_algorithm = None;
    if let AlgorithmParameters::EllipticCurve(ec) = &mut ec_p384.algorithm {
        ec.curve = EllipticCurve::P384;
    }
    let hmac = Jwk {
        common: CommonParameters {
            key_id: Some("hmac".into()),
            key_algorithm: Some(KeyAlgorithm::HS256),
            ..Default::default()
        },
        algorithm: AlgorithmParameters::OctetKey(OctetKeyParameters {
            key_type: OctetKeyType::Octet,
            value: "c2VjcmV0LXNlY3JldC1zZWNyZXQtc2VjcmV0LXNlY3JldA".into(),
        }),
    };
    let mut broken = rsa_jwk("broken");
    if let AlgorithmParameters::RSA(rsa) = &mut broken.algorithm {
        rsa.n = "!!! not base64 !!!".into();
    }

    let fetcher = MemFetcher::new(Ok(set([
        rs384,
        ps256,
        unknown_alg,
        rsa_no_alg,
        rsa_claims_es,
        ec_wrong_curve,
        ec_p384,
        hmac,
        broken,
        ec_jwk("ec"),
        rsa_jwk("rsa"),
    ])));
    let source = source(&fetcher);
    let rs_es = [Algorithm::RS256, Algorithm::ES256];
    for ignored in [
        "rs384",
        "ps256",
        "unknown-alg",
        "rsa-claims-es",
        "ec-wrong-curve",
        "ec-p384",
        "hmac",
        "broken",
    ] {
        assert_eq!(
            source.key_for(ignored, &rs_es).await.unwrap_err(),
            KeyLookupError::UnknownKid,
            "{ignored}"
        );
    }
    assert_eq!(
        kid_of(source.key_for("rsa", &rs_es).await),
        Ok("rsa".into())
    );
    assert_eq!(kid_of(source.key_for("ec", &rs_es).await), Ok("ec".into()));
    assert_eq!(
        kid_of(source.key_for("rsa-no-alg", &rs_es).await),
        Ok("rsa-no-alg".into())
    );
    // The same keys outside the allow-list.
    assert_eq!(
        source.key_for("ec", RS).await.unwrap_err(),
        KeyLookupError::UnknownKid
    );
    assert_eq!(
        source
            .key_for("rsa", &[Algorithm::ES256])
            .await
            .unwrap_err(),
        KeyLookupError::UnknownKid
    );
    assert_eq!(
        kid_of(source.key_for("rs384", &[Algorithm::RS384]).await),
        Ok("rs384".into())
    );
    // HMAC keys never fit, not even when HMAC is requested.
    assert_eq!(
        source
            .key_for("hmac", &[Algorithm::HS256])
            .await
            .unwrap_err(),
        KeyLookupError::UnknownKid
    );
    assert_eq!(fetcher.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn key_with_use_enc_is_ignored() {
    let mut enc = rsa_jwk("enc");
    enc.common.public_key_use = Some(PublicKeyUse::Encryption);
    let mut other = rsa_jwk("other");
    other.common.public_key_use = Some(PublicKeyUse::Other("wrap".into()));
    let mut no_use = rsa_jwk("no-use");
    no_use.common.public_key_use = None;
    // Two keys share a kid: the encryption one is skipped, the signing one is used.
    let mut shared_enc = rsa_jwk("shared");
    shared_enc.common.public_key_use = Some(PublicKeyUse::Encryption);
    let fetcher = MemFetcher::new(Ok(set([enc, other, no_use, shared_enc, rsa_jwk("shared")])));
    let source = source(&fetcher);
    for ignored in ["enc", "other"] {
        assert_eq!(
            source.key_for(ignored, RS).await.unwrap_err(),
            KeyLookupError::UnknownKid
        );
    }
    assert!(source.key_for("no-use", RS).await.is_ok());
    let shared = source.key_for("shared", RS).await.unwrap();
    assert_eq!(shared.common.public_key_use, Some(PublicKeyUse::Signature));
}

#[test]
fn errors_have_fixed_text() {
    let texts = [
        (FetchError::Network.to_string(), "JWKS request failed"),
        (FetchError::Timeout.to_string(), "JWKS request timed out"),
        (FetchError::TooLarge.to_string(), "JWKS response too large"),
        (
            FetchError::BadStatus.to_string(),
            "JWKS endpoint returned an error status",
        ),
        (
            FetchError::Malformed.to_string(),
            "JWKS response is not a valid key set",
        ),
        (
            KeyLookupError::UnknownKid.to_string(),
            "no usable key for the key id",
        ),
        (
            KeyLookupError::Unavailable.to_string(),
            "JWKS keys unavailable",
        ),
        (JwksError::InvalidUri.to_string(), "invalid JWKS URI"),
        (
            JwksError::InsecureUri.to_string(),
            "JWKS URI must use https",
        ),
        (
            JwksError::ZeroTtl.to_string(),
            "JWKS TTL must be greater than zero",
        ),
        (
            JwksError::ZeroTimeout.to_string(),
            "JWKS fetch timeout must be greater than zero",
        ),
    ];
    for (actual, expected) in texts {
        assert_eq!(actual, expected);
    }
}

#[tokio::test(start_paused = true)]
async fn debug_does_not_print_keys() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    let source = source(&fetcher);
    source.key_for("a", RS).await.unwrap();
    let modulus = match &rsa_jwk("a").algorithm {
        AlgorithmParameters::RSA(rsa) => rsa.n.clone(),
        _ => unreachable!(),
    };
    let text = format!("{source:?} {:?}", JwksSource::builder(fetcher.clone(), URI));
    assert!(!text.contains(&modulus[..20]), "{text}");
    assert!(text.contains(URI));
}

#[test]
fn debug_does_not_print_uri_userinfo_or_query() {
    let fetcher = MemFetcher::new(Ok(set([])));
    let uri = "https://user:hunter2@idp.example:8443/jwks?apikey=SECRET-XYZ";
    let builder = JwksSource::builder(fetcher.clone(), uri);
    let source = JwksSource::builder(fetcher.clone(), uri).build().unwrap();
    for text in [format!("{builder:?}"), format!("{source:?}")] {
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("user:"), "{text}");
        assert!(!text.contains("SECRET-XYZ"), "{text}");
        assert!(!text.contains("apikey"), "{text}");
        assert!(text.contains("https://idp.example:8443/jwks"), "{text}");
    }
    let bad = format!("{:?}", JwksSource::builder(fetcher, "not a uri hunter2"));
    assert!(!bad.contains("hunter2"), "{bad}");
}

// ---- decoder integration ----

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn sign(kid: Option<&str>) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = kid.map(str::to_string);
    let key = EncodingKey::from_rsa_pem(RSA_PRIVATE.as_bytes()).unwrap();
    encode(
        &header,
        &json!({ "sub": "alice", "exp": now() + 300 }),
        &key,
    )
    .unwrap()
}

fn recording_decoder(
    source: Arc<JwksSource>,
) -> (Arc<dyn JwtDecoder>, Arc<Mutex<Vec<FailureReason>>>) {
    let reasons: Arc<Mutex<Vec<FailureReason>>> = Arc::default();
    let sink = reasons.clone();
    let decoder = JwtDecoderBuilder::with_jwks(source, vec![Algorithm::RS256])
        .on_failure(Arc::new(move |r| sink.lock().unwrap().push(r)))
        .build()
        .unwrap();
    (Arc::new(decoder), reasons)
}

#[tokio::test(start_paused = true)]
async fn jwks_decoder_verifies_rotated_key_end_to_end() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("old")])));
    let (decoder, reasons) = recording_decoder(source(&fetcher));

    let jwt = decoder.decode(&sign(Some("old"))).await.unwrap();
    assert_eq!(jwt.subject(), Some("alice"));

    // The issuer rotates to a new kid.
    fetcher.serve(Ok(set([rsa_jwk("new")])));
    advance(INTERVAL).await;
    let jwt = decoder.decode(&sign(Some("new"))).await.unwrap();
    assert_eq!(jwt.subject(), Some("alice"));
    assert_eq!(fetcher.calls(), 2);
    assert!(reasons.lock().unwrap().is_empty());

    // Without a kid, or with an unknown one, the token is invalid.
    for token in [sign(None), sign(Some("never"))] {
        assert_eq!(
            decoder.decode(&token).await.unwrap_err(),
            AuthenticationError::InvalidToken
        );
    }
    assert_eq!(
        *reasons.lock().unwrap(),
        [FailureReason::UnknownKid, FailureReason::UnknownKid]
    );
    // A token whose alg is outside the allow-list never reaches the key source.
    let es = {
        let key = EncodingKey::from_ec_pem(
            include_str!("../../tests/fixtures/ec_private.pem").as_bytes(),
        )
        .unwrap();
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("other".into());
        encode(
            &header,
            &json!({ "sub": "alice", "exp": now() + 300 }),
            &key,
        )
        .unwrap()
    };
    advance(INTERVAL).await;
    let calls = fetcher.calls();
    assert_eq!(
        decoder.decode(&es).await.unwrap_err(),
        AuthenticationError::InvalidToken
    );
    assert_eq!(fetcher.calls(), calls);
}

#[tokio::test(start_paused = true)]
async fn jwks_outage_gives_provider_unavailable_and_503() {
    let fetcher = MemFetcher::new(Err(FetchError::Network));
    let (decoder, _) = recording_decoder(source(&fetcher));
    let token = sign(Some("a"));
    assert_eq!(
        decoder.decode(&token).await.unwrap_err(),
        AuthenticationError::ProviderUnavailable
    );

    let chain = HttpSecurity::new()
        .oauth2_resource_server(|r| r.jwt(decoder))
        .authorize(|a| a.any_request().authenticated())
        .build()
        .unwrap();
    let me = |req: Request| async move { req.authentication().unwrap().principal().to_string() };
    let app = App::new()
        .filter(chain)
        .route(Route::new(HttpMethod::Get, "/me", me))
        .unwrap();
    let mut req = Request::new(HttpMethod::Get, "/me");
    req.headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let res = app.handle(req).await;
    assert_eq!(res.status(), 503);
    assert!(res.headers().get(header::WWW_AUTHENTICATE).is_none());
    // Rendered by the app's error pipeline.
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );

    // Once the issuer is back (after the rate limit), the same token is accepted.
    fetcher.serve(Ok(set([rsa_jwk("a")])));
    advance(INTERVAL).await;
    let mut req = Request::new(HttpMethod::Get, "/me");
    req.headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let res = app.handle(req).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body().as_ref(), b"alice");
}

fn live_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn lookup_outside_runtime_fails_closed_without_wedging_the_cache() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    let source = source(&fetcher);
    let mut lookup = Box::pin(source.key_for("a", RS));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let first = lookup.as_mut().poll(&mut cx);
    assert!(
        matches!(
            first,
            std::task::Poll::Ready(Err(KeyLookupError::Unavailable))
        ),
        "{first:?}"
    );
    drop(lookup);
    assert_eq!(fetcher.calls(), 0);

    // No fetch ran, so the rate-limit slot is not used up: a lookup on a runtime fetches now.
    let found = live_runtime().block_on(source.key_for("a", RS));
    assert_eq!(kid_of(found), Ok("a".into()));
    assert_eq!(fetcher.calls(), 1);
}

#[test]
fn fetch_task_dropped_unpolled_on_dead_runtime_does_not_wedge_the_cache() {
    let fetcher = MemFetcher::new(Ok(set([rsa_jwk("a")])));
    let source = source(&fetcher);
    let dead = live_runtime();
    let dead_handle = dead.handle().clone();
    drop(dead);

    let rt = live_runtime();
    rt.block_on(async {
        {
            // `tokio::spawn` now targets the shut-down runtime, which drops the task unpolled.
            let _enter = dead_handle.enter();
            assert_eq!(
                source.key_for("a", RS).await.unwrap_err(),
                KeyLookupError::Unavailable
            );
        }
        assert_eq!(fetcher.calls(), 0);
        assert_eq!(kid_of(source.key_for("a", RS).await), Ok("a".into()));
        assert_eq!(fetcher.calls(), 1);
    });
}
