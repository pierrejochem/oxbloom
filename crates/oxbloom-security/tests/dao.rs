use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use oxbloom_config::Secret;
use oxbloom_security::{
    Argon2PasswordEncoder, AuthenticationEntryPoint, AuthenticationError, AuthenticationProvider,
    AuthenticationRequest, DaoAuthenticationProvider, DefaultEntryPoint,
    InMemoryUserDetailsService, PasswordEncoder, UserDetails, UserDetailsError, UserDetailsService,
    UserStoreError,
};
use oxbloom_web::{HttpMethod, Request};

#[derive(Default)]
struct Counts {
    encodes: usize,
    encoded: Vec<String>,
    matches: usize,
    matched_against: Vec<String>,
}

struct Counting {
    inner: Argon2PasswordEncoder,
    counts: Arc<Mutex<Counts>>,
}

impl PasswordEncoder for Counting {
    fn encode(&self, raw: &str) -> String {
        let hash = self.inner.encode(raw);
        let mut c = self.counts.lock().unwrap();
        c.encodes += 1;
        c.encoded.push(hash.clone());
        hash
    }

    fn matches(&self, raw: &str, encoded: &str) -> bool {
        {
            let mut c = self.counts.lock().unwrap();
            c.matches += 1;
            c.matched_against.push(encoded.to_owned());
        }
        self.inner.matches(raw, encoded)
    }

    fn upgrade_encoding(&self, encoded: &str) -> bool {
        self.inner.upgrade_encoding(encoded)
    }
}

fn request(username: &str, password: &str) -> AuthenticationRequest {
    AuthenticationRequest::UsernamePassword {
        username: username.to_string(),
        password: Secret::new(password.to_string()),
    }
}

struct Fixture {
    provider: DaoAuthenticationProvider,
    counts: Arc<Mutex<Counts>>,
}

/// Builds users with real hashes made by the plain encoder, then the provider on a counting one.
fn fixture(extra: Vec<(&str, &str, bool, bool)>) -> Fixture {
    let plain = Argon2PasswordEncoder::new(8, 1, 1).unwrap();
    let mut users = vec![
        UserDetails::new("alice", plain.encode("s3cret"), []).roles(&["USER", "ADMIN"]),
        UserDetails::new("empty-pw", plain.encode("x"), []),
        UserDetails::new("broken", "not-a-hash", []),
    ];
    for (name, pw, enabled, locked) in extra {
        users.push(
            UserDetails::new(name, plain.encode(pw), [])
                .enabled(enabled)
                .locked(locked),
        );
    }
    let counts = Arc::new(Mutex::new(Counts::default()));
    let encoder = Counting {
        inner: Argon2PasswordEncoder::new(8, 1, 1).unwrap(),
        counts: Arc::clone(&counts),
    };
    let provider = DaoAuthenticationProvider::new(
        Arc::new(InMemoryUserDetailsService::new(users).unwrap()),
        Arc::new(encoder),
    );
    Fixture { provider, counts }
}

#[tokio::test]
async fn valid_credentials_authenticate() {
    let f = fixture(vec![]);
    let auth = f
        .provider
        .authenticate(&request("alice", "s3cret"))
        .await
        .unwrap();
    assert_eq!(auth.principal(), "alice");
    assert!(auth.has_authority("ROLE_USER") && auth.has_role("ADMIN"));
}

#[tokio::test]
async fn wrong_password_and_unknown_user_give_the_same_error() {
    let f = fixture(vec![]);
    let wrong = f
        .provider
        .authenticate(&request("alice", "nope"))
        .await
        .unwrap_err();
    let unknown = f
        .provider
        .authenticate(&request("ghost", "nope"))
        .await
        .unwrap_err();
    assert_eq!(wrong, AuthenticationError::BadCredentials);
    assert_eq!(unknown, AuthenticationError::BadCredentials);
}

#[tokio::test]
async fn unknown_user_still_runs_the_encoder_once() {
    let f = fixture(vec![]);
    f.counts.lock().unwrap().matches = 0;
    let _ = f.provider.authenticate(&request("ghost", "pw")).await;
    assert_eq!(f.counts.lock().unwrap().matches, 1);
    f.counts.lock().unwrap().matches = 0;
    let _ = f.provider.authenticate(&request("alice", "pw")).await;
    assert_eq!(f.counts.lock().unwrap().matches, 1);
}

#[tokio::test]
async fn dummy_hash_uses_the_encoders_parameters() {
    let f = fixture(vec![]);
    {
        let c = f.counts.lock().unwrap();
        assert_eq!(c.encodes, 1, "construction encodes the dummy hash once");
    }
    let _ = f.provider.authenticate(&request("ghost", "pw")).await;
    let c = f.counts.lock().unwrap();
    assert_eq!(c.matches, 1);
    assert_eq!(c.matched_against[0], c.encoded[0]);
    assert_eq!(c.encodes, 1, "authenticating does not encode again");
}

#[tokio::test]
async fn disabled_user_fails_only_after_a_correct_password() {
    let f = fixture(vec![("dis", "pw", false, false)]);
    let wrong = f
        .provider
        .authenticate(&request("dis", "bad"))
        .await
        .unwrap_err();
    assert_eq!(wrong, AuthenticationError::BadCredentials);
    let right = f
        .provider
        .authenticate(&request("dis", "pw"))
        .await
        .unwrap_err();
    assert_eq!(right, AuthenticationError::AccountDisabled);
}

#[tokio::test]
async fn locked_user_fails_only_after_a_correct_password() {
    let f = fixture(vec![("lck", "pw", true, true)]);
    let wrong = f
        .provider
        .authenticate(&request("lck", "bad"))
        .await
        .unwrap_err();
    assert_eq!(wrong, AuthenticationError::BadCredentials);
    let right = f
        .provider
        .authenticate(&request("lck", "pw"))
        .await
        .unwrap_err();
    assert_eq!(right, AuthenticationError::AccountLocked);
}

#[tokio::test]
async fn malformed_stored_hash_counts_as_bad_credentials() {
    let f = fixture(vec![]);
    let err = f
        .provider
        .authenticate(&request("broken", "pw"))
        .await
        .unwrap_err();
    assert_eq!(err, AuthenticationError::BadCredentials);
}

struct Panicking;

#[async_trait]
impl UserDetailsService for Panicking {
    async fn load_user_by_username(
        &self,
        _username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        panic!("the service must not be called");
    }
}

#[tokio::test]
async fn empty_username_and_empty_password() {
    let plain = Argon2PasswordEncoder::new(8, 1, 1).unwrap();
    let provider = DaoAuthenticationProvider::new(Arc::new(Panicking), Arc::new(plain));
    let err = provider.authenticate(&request("", "pw")).await.unwrap_err();
    assert_eq!(err, AuthenticationError::BadCredentials);

    let f = fixture(vec![]);
    let err = f
        .provider
        .authenticate(&request("empty-pw", ""))
        .await
        .unwrap_err();
    assert_eq!(err, AuthenticationError::BadCredentials);
}

#[tokio::test]
async fn oversized_credentials_fail_before_hashing() {
    let f = fixture(vec![]);
    let before = f.counts.lock().unwrap().matches;
    let long = "a".repeat(1025);
    let err = f
        .provider
        .authenticate(&request(&long, "pw"))
        .await
        .unwrap_err();
    assert_eq!(err, AuthenticationError::BadCredentials);
    let err = f
        .provider
        .authenticate(&request("alice", &long))
        .await
        .unwrap_err();
    assert_eq!(err, AuthenticationError::BadCredentials);
    assert_eq!(f.counts.lock().unwrap().matches, before);
    // Exactly 1024 bytes is still checked.
    let ok = "b".repeat(1024);
    let _ = f.provider.authenticate(&request("alice", &ok)).await;
    assert_eq!(f.counts.lock().unwrap().matches, before + 1);
}

#[tokio::test]
async fn supports_only_username_password() {
    let f = fixture(vec![]);
    assert!(f.provider.supports(&request("a", "b")));
    assert!(
        !f.provider
            .supports(&AuthenticationRequest::BearerToken(Secret::new("t".into())))
    );
}

struct Failing;

#[async_trait]
impl UserDetailsService for Failing {
    async fn load_user_by_username(
        &self,
        _username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        Err(UserDetailsError::new("backend down"))
    }
}

#[tokio::test]
async fn service_error_is_provider_unavailable() {
    let provider = DaoAuthenticationProvider::new(
        Arc::new(Failing),
        Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap()),
    );
    let err = provider
        .authenticate(&request("alice", "pw"))
        .await
        .unwrap_err();
    assert_eq!(err, AuthenticationError::ProviderUnavailable);
}

#[test]
fn debug_does_not_leak() {
    let req = format!("{:?}", request("carol-the-user", "hunter2-password"));
    assert!(req.contains("[REDACTED]") && req.contains("14"));
    assert!(!req.contains("carol") && !req.contains("hunter2"));

    let hash = Argon2PasswordEncoder::new(8, 1, 1).unwrap().encode("pw");
    let user = UserDetails::new("carol", hash.clone(), []).roles(&["USER"]);
    let dbg = format!("{user:?}");
    assert!(dbg.contains("carol") && dbg.contains("ROLE_USER") && dbg.contains("[REDACTED]"));
    assert!(!dbg.contains(&hash) && !dbg.contains("argon2"));

    let f = fixture(vec![]);
    let dbg = format!("{:?}", f.provider);
    let c = f.counts.lock().unwrap();
    assert!(!dbg.contains(&c.encoded[0]) && !dbg.contains("argon2") && !dbg.contains("alice"));
}

#[test]
fn in_memory_rejects_duplicates_and_empty_names() {
    let dup = vec![
        UserDetails::new("a", "h", []),
        UserDetails::new("a", "h2", []),
    ];
    assert_eq!(
        InMemoryUserDetailsService::new(dup).err(),
        Some(UserStoreError::DuplicateUsername)
    );
    assert_eq!(
        InMemoryUserDetailsService::new(vec![UserDetails::new("", "h", [])]).err(),
        Some(UserStoreError::EmptyUsername)
    );
    assert!(InMemoryUserDetailsService::new(vec![]).is_ok());
}

#[tokio::test]
async fn in_memory_lookup_is_case_sensitive() {
    let store = InMemoryUserDetailsService::new(vec![UserDetails::new("Alice", "h", [])]).unwrap();
    assert!(
        store
            .load_user_by_username("Alice")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .load_user_by_username("alice")
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn account_errors_answer_401() {
    let req = Request::new(HttpMethod::Get, "/");
    for error in [
        AuthenticationError::AccountDisabled,
        AuthenticationError::AccountLocked,
    ] {
        assert_eq!(DefaultEntryPoint.commence(&req, Some(&error)).status(), 401);
    }
    assert_eq!(
        DefaultEntryPoint
            .commence(&req, Some(&AuthenticationError::ProviderUnavailable))
            .status(),
        503
    );
}
