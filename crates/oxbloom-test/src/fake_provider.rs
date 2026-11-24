//! An OpenID Connect provider over [`MockHttpClient`], for tests of an OAuth2 login.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use aws_lc_rs::digest::{SHA256, digest};
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use http::{HeaderMap, HeaderValue, Method, StatusCode};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use oxbloom_core::{Clock, SystemClock};
use oxbloom_http_client::{Body, Request, Response};
use serde_json::{Map, Value, json};

use crate::MockHttpClient;

/// How long the access token and the id token of the fake are valid, in seconds.
const LIFETIME: u64 = 3600;
/// The `error_description` the fake sends with [`Fault::TokenEndpointError`]. Code under test
/// must never copy it into an error or a log.
const ERROR_DESCRIPTION: &str = "FAKE-PROVIDER-ERROR-DESCRIPTION";
/// An audience and an issuer that belong to someone else.
const OTHER: &str = "https://other.example.test";

/// A way for [`FakeProvider`] to misbehave, set with [`FakeProvider::misbehave`]. Each one
/// drives a negative test of the code under test.
///
/// # Examples
///
/// ```
/// use oxbloom_test::{FakeProvider, Fault};
///
/// let fake = FakeProvider::new("https://idp.example.test");
/// fake.misbehave(Fault::WrongNonce)
///     .misbehave(Fault::TokenEndpointStatus(503));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Fault {
    /// The id token's `iss` is another issuer.
    WrongIssuer,
    /// The id token's `aud` is another client.
    WrongAudience,
    /// The id token has two audiences, and its `azp` names the other client.
    WrongAzp,
    /// The id token expired an hour ago.
    Expired,
    /// The id token is issued, and valid from, an hour in the future.
    NotYetValid,
    /// The id token's `nonce` is not the expected one.
    WrongNonce,
    /// The id token is unsigned, with the header `alg: none`.
    AlgNone,
    /// The id token is signed with HS256, using the public key bytes as the HMAC secret.
    Hs256WithPublicKey,
    /// The id token has no `sub`.
    MissingSub,
    /// The token endpoint answers 400 with this `error` code and an `error_description`.
    TokenEndpointError(&'static str),
    /// The token endpoint answers this status with a text body.
    TokenEndpointStatus(u16),
    /// The token endpoint answers 200 with a JSON body larger than 1 MiB.
    OversizedBody,
    /// The discovery document names another issuer.
    DiscoveryIssuerMismatch,
    /// The user-info endpoint answers 500.
    UserinfoError,
}

/// A user of [`FakeProvider`]: the subject and the attributes that appear in the id token and
/// the user info.
///
/// `Debug` shows the subject and the attribute names, not their values.
///
/// # Examples
///
/// ```
/// use oxbloom_test::FakeUser;
///
/// let user = FakeUser::new("alice")
///     .email("alice@example.test")
///     .name("Alice")
///     .attr("login", "alice-gh")
///     .scope("openid email");
/// # let _ = user;
/// ```
#[derive(Clone)]
pub struct FakeUser {
    sub: String,
    attributes: Map<String, Value>,
    scope: Option<String>,
}

impl FakeUser {
    /// A user with the subject `sub` and no other attribute. Without
    /// [`scope`](Self::scope) the token response grants `openid`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeUser;
    ///
    /// let user = FakeUser::new("alice");
    /// assert_eq!(user.sub(), "alice");
    /// ```
    pub fn new(sub: &str) -> FakeUser {
        FakeUser {
            sub: sub.to_owned(),
            attributes: Map::new(),
            scope: None,
        }
    }

    /// The subject.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeUser;
    ///
    /// assert_eq!(FakeUser::new("bob").sub(), "bob");
    /// ```
    pub fn sub(&self) -> &str {
        &self.sub
    }

    /// Sets the `email` attribute.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeUser;
    ///
    /// let user = FakeUser::new("alice").email("alice@example.test");
    /// # let _ = user;
    /// ```
    #[must_use]
    pub fn email(self, email: &str) -> FakeUser {
        self.attr("email", email)
    }

    /// Sets the `name` attribute.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeUser;
    ///
    /// let user = FakeUser::new("alice").name("Alice");
    /// # let _ = user;
    /// ```
    #[must_use]
    pub fn name(self, name: &str) -> FakeUser {
        self.attr("name", name)
    }

    /// Sets the attribute `key` to `value`. A key the fake sets itself in the id token
    /// (`iss`, `sub`, `aud`, `exp`, `iat`, `nonce`) is overwritten there, but kept in the user
    /// info.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeUser;
    ///
    /// let user = FakeUser::new("alice")
    ///     .attr("login", "alice-gh")
    ///     .attr("groups", serde_json::json!(["dev", "ops"]));
    /// # let _ = user;
    /// ```
    #[must_use]
    pub fn attr(mut self, key: &str, value: impl Into<Value>) -> FakeUser {
        self.attributes.insert(key.to_owned(), value.into());
        self
    }

    /// Sets the `scope` of the token response, a space-separated list of granted scopes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeUser;
    ///
    /// let user = FakeUser::new("alice").scope("openid profile");
    /// # let _ = user;
    /// ```
    #[must_use]
    pub fn scope(mut self, scope: &str) -> FakeUser {
        self.scope = Some(scope.to_owned());
        self
    }

    /// The user info: the attributes plus `sub`.
    fn user_info(&self) -> Map<String, Value> {
        let mut info = self.attributes.clone();
        info.insert("sub".into(), Value::String(self.sub.clone()));
        info
    }
}

impl fmt::Debug for FakeUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeUser")
            .field("sub", &self.sub)
            .field("attributes", &self.attributes.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// A request the token endpoint of [`FakeProvider`] received: its form fields and its
/// `Authorization` header.
///
/// # Examples
///
/// ```
/// use oxbloom_http_client::{Body, HttpClient, Request};
/// use oxbloom_test::{FakeProvider, FakeUser};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let fake = FakeProvider::new("https://idp.example.test");
/// let code = fake.issue_code(FakeUser::new("alice"));
/// let request = Request::post("https://idp.example.test/token").body(Body::Form(vec![
///     ("grant_type".into(), "authorization_code".into()),
///     ("code".into(), code),
///     ("client_id".into(), "app".into()),
/// ]));
/// fake.client().execute(request).await?;
///
/// let seen = &fake.token_requests()[0];
/// assert_eq!(seen.field("client_id"), Some("app"));
/// assert_eq!(seen.authorization(), None);
/// # Ok::<(), oxbloom_http_client::HttpClientError>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct TokenRequest {
    fields: Vec<(String, String)>,
    authorization: Option<String>,
}

impl TokenRequest {
    /// The value of the first form field named `name`.
    ///
    /// # Examples
    ///
    /// See [`TokenRequest`].
    ///
    /// ```
    /// # use oxbloom_http_client::{Body, HttpClient, Request};
    /// # use oxbloom_test::{FakeProvider, FakeUser};
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// # let fake = FakeProvider::new("https://idp.example.test");
    /// # let code = fake.issue_code(FakeUser::new("alice"));
    /// # let request = Request::post("https://idp.example.test/token").body(Body::Form(vec![
    /// #     ("grant_type".into(), "authorization_code".into()), ("code".into(), code),
    /// # ]));
    /// # fake.client().execute(request).await?;
    /// assert_eq!(fake.token_requests()[0].field("grant_type"), Some("authorization_code"));
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Every form field, in the order sent.
    ///
    /// # Examples
    ///
    /// ```
    /// # use oxbloom_http_client::{Body, HttpClient, Request};
    /// # use oxbloom_test::{FakeProvider, FakeUser};
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// # let fake = FakeProvider::new("https://idp.example.test");
    /// # let code = fake.issue_code(FakeUser::new("alice"));
    /// # let request = Request::post("https://idp.example.test/token").body(Body::Form(vec![
    /// #     ("grant_type".into(), "authorization_code".into()), ("code".into(), code),
    /// # ]));
    /// # fake.client().execute(request).await?;
    /// let seen = fake.token_requests();
    /// let names: Vec<&str> = seen[0]
    ///     .fields()
    ///     .iter()
    ///     .map(|(name, _)| name.as_str())
    ///     .collect();
    /// assert_eq!(names, ["grant_type", "code"]);
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn fields(&self) -> &[(String, String)] {
        &self.fields
    }

    /// The `Authorization` header, when one was sent.
    ///
    /// # Examples
    ///
    /// ```
    /// # use oxbloom_http_client::{Body, HttpClient, Request};
    /// # use oxbloom_test::{FakeProvider, FakeUser};
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// # let fake = FakeProvider::new("https://idp.example.test");
    /// # let code = fake.issue_code(FakeUser::new("alice"));
    /// let request = Request::post("https://idp.example.test/token")
    ///     .header("authorization", "Basic YXBwOnNlY3JldA==")
    ///     .body(Body::Form(vec![("code".into(), code)]));
    /// fake.client().execute(request).await?;
    /// assert_eq!(fake.token_requests()[0].authorization(), Some("Basic YXBwOnNlY3JldA=="));
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn authorization(&self) -> Option<&str> {
        self.authorization.as_deref()
    }
}

impl fmt::Debug for TokenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenRequest")
            .field(
                "fields",
                &self.fields.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .field("authorization", &self.authorization.as_ref().map(|_| ".."))
            .finish()
    }
}

/// A code that has been issued and not yet exchanged.
struct Issued {
    user: FakeUser,
    nonce: Option<String>,
}

struct State {
    codes: HashMap<String, Issued>,
    access_tokens: HashMap<String, FakeUser>,
    faults: Vec<Fault>,
    nonce: Option<String>,
    token_requests: Vec<TokenRequest>,
    clock: Arc<dyn Clock>,
    expected_challenge: Option<String>,
    expected_redirect_uri: Option<String>,
    expected_secret: Option<String>,
}

/// The parts of the fake that the HTTP handler needs.
struct Shared {
    issuer: String,
    base: String,
    kid: String,
    signing_key: EncodingKey,
    public_point: Vec<u8>,
    state: Mutex<State>,
}

/// An OpenID Connect provider for tests, served through a [`MockHttpClient`].
///
/// It generates an ES256 key pair at construction and answers, under the issuer:
/// `/.well-known/openid-configuration` (the discovery document), `/jwks` (the public key),
/// `POST /token` (the authorization code grant) and `/userinfo` (the user info for a bearer
/// access token). `/authorize` is not served: the browser goes there, not the client.
///
/// [`issue_code`](Self::issue_code) registers a code for a [`FakeUser`]; the next token request
/// with that code gets an access token, a refresh token and an id token whose `aud` is the
/// client id of the token request (from HTTP Basic or the `client_id` field) and whose `nonce`
/// is the one set with [`expect_nonce`](Self::expect_nonce) before the code was issued, or,
/// when none was set then, the one set when the code is exchanged. A code works once; an
/// unknown code gets `invalid_grant`. [`misbehave`](Self::misbehave) adds a [`Fault`]. The key
/// id, the codes and the tokens are random per instance.
///
/// By default the token endpoint does not check the PKCE verifier, the redirect URI or the
/// client secret. [`expect_pkce`](Self::expect_pkce),
/// [`expect_redirect_uri`](Self::expect_redirect_uri) and
/// [`expect_client_secret`](Self::expect_client_secret) turn those checks on, so an end-to-end
/// test notices a dropped `code_verifier` or a wrong secret.
///
/// Available with the cargo feature `oauth2`. `Debug` shows the issuer only.
///
/// # Examples
///
/// ```
/// use oxbloom_http_client::{Body, HttpClient, Request};
/// use oxbloom_test::{FakeProvider, FakeUser};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let fake = FakeProvider::new("https://idp.example.test");
/// fake.expect_nonce("n-123");
/// let code = fake.issue_code(FakeUser::new("alice").email("alice@example.test"));
///
/// let request = Request::post("https://idp.example.test/token")
///     .header("authorization", "Basic YXBwOnNlY3JldA==") // app:secret
///     .body(Body::Form(vec![
///         ("grant_type".into(), "authorization_code".into()),
///         ("code".into(), code),
///     ]));
/// let response = fake.client().execute(request).await?;
/// let body: serde_json::Value = response.json()?;
/// assert_eq!(body["token_type"], "Bearer");
/// assert!(body["id_token"].is_string());
/// # Ok::<(), oxbloom_http_client::HttpClientError>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct FakeProvider {
    shared: Arc<Shared>,
    client: Arc<MockHttpClient>,
}

impl FakeProvider {
    /// A provider for `issuer` with a new ES256 key pair, using the system time.
    ///
    /// The endpoints are the issuer, without a trailing slash, followed by the path; the
    /// discovery document names `issuer` exactly as given.
    ///
    /// # Panics
    ///
    /// Panics when the key pair cannot be generated, which means the crypto library is broken.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeProvider;
    ///
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// assert_eq!(fake.issuer(), "https://idp.example.test");
    /// ```
    pub fn new(issuer: &str) -> FakeProvider {
        let pkcs8 = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING)
            .and_then(|pair| pair.to_pkcs8v1())
            .expect("generating a P-256 key pair");
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref())
            .expect("reading the generated P-256 key pair");
        let shared = Arc::new(Shared {
            issuer: issuer.to_owned(),
            base: issuer.trim_end_matches('/').to_owned(),
            kid: random_token(),
            signing_key: EncodingKey::from_ec_der(pkcs8.as_ref()),
            public_point: pair.public_key().as_ref().to_vec(),
            state: Mutex::new(State {
                codes: HashMap::new(),
                access_tokens: HashMap::new(),
                faults: Vec::new(),
                nonce: None,
                token_requests: Vec::new(),
                clock: Arc::new(SystemClock),
                expected_challenge: None,
                expected_redirect_uri: None,
                expected_secret: None,
            }),
        });
        let client = Arc::new(MockHttpClient::new());
        let handler = shared.clone();
        client.on_fn(move |request| handler.answer(request));
        FakeProvider { shared, client }
    }

    /// Takes the times of the id tokens from `clock` instead of the system time.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::SystemTime;
    /// use oxbloom_test::{FakeProvider, ManualClock};
    ///
    /// let clock = ManualClock::at(SystemTime::now());
    /// let fake = FakeProvider::new("https://idp.example.test").clock(Arc::new(clock));
    /// # let _ = fake;
    /// ```
    #[must_use]
    pub fn clock(self, clock: Arc<dyn Clock>) -> FakeProvider {
        self.shared.state().clock = clock;
        self
    }

    /// The issuer, as given to [`new`](Self::new).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeProvider;
    ///
    /// assert_eq!(FakeProvider::new("https://idp.example.test/").issuer(), "https://idp.example.test/");
    /// ```
    pub fn issuer(&self) -> &str {
        &self.shared.issuer
    }

    /// The client that serves the provider's endpoints. Every call returns the same client, so
    /// its recorded requests cover every call.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_http_client::HttpClient;
    /// use oxbloom_test::FakeProvider;
    ///
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// let client: Arc<dyn HttpClient> = fake.client();
    /// assert_eq!(fake.client().request_count(), 0);
    /// # let _ = client;
    /// ```
    pub fn client(&self) -> Arc<MockHttpClient> {
        self.client.clone()
    }

    /// Registers a new random code for `user` and returns it. The nonce set with
    /// [`expect_nonce`](Self::expect_nonce) so far goes into the id token of this code; when
    /// none is set yet, the one set by the time the code is exchanged is used.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::{FakeProvider, FakeUser};
    ///
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// let first = fake.issue_code(FakeUser::new("alice"));
    /// let second = fake.issue_code(FakeUser::new("alice"));
    /// assert_ne!(first, second);
    /// ```
    pub fn issue_code(&self, user: FakeUser) -> String {
        let code = random_token();
        let mut state = self.shared.state();
        let nonce = state.nonce.clone();
        state.codes.insert(code.clone(), Issued { user, nonce });
        code
    }

    /// Sets the nonce that the id tokens of codes issued from now on carry, normally the one
    /// the login under test sent in the authorization request. A code issued before any
    /// nonce was set takes the nonce current when it is exchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::{FakeProvider, FakeUser};
    ///
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// fake.expect_nonce("nonce-from-the-authorization-url");
    /// let code = fake.issue_code(FakeUser::new("alice"));
    /// # let _ = code;
    /// ```
    pub fn expect_nonce(&self, nonce: &str) -> &Self {
        self.shared.state().nonce = Some(nonce.to_owned());
        self
    }

    /// Makes the token endpoint check the PKCE verifier: the base64url (no padding) SHA-256 of
    /// the `code_verifier` field must equal `challenge`, else the answer is `invalid_grant`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeProvider;
    ///
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// // The RFC 7636 test vector: the challenge of the verifier
    /// // "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".
    /// fake.expect_pkce("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    /// ```
    pub fn expect_pkce(&self, challenge: &str) -> &Self {
        self.shared.state().expected_challenge = Some(challenge.to_owned());
        self
    }

    /// Makes the token endpoint check that the `redirect_uri` field equals `uri` exactly, else
    /// the answer is `invalid_grant`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeProvider;
    ///
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// fake.expect_redirect_uri("https://app.example/login/oauth2/code/idp");
    /// ```
    pub fn expect_redirect_uri(&self, uri: &str) -> &Self {
        self.shared.state().expected_redirect_uri = Some(uri.to_owned());
        self
    }

    /// Makes the token endpoint check the client secret, from HTTP Basic (form-URL-decoded) or
    /// from the `client_secret` field; a wrong or missing secret gets `invalid_client`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeProvider;
    ///
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// fake.expect_client_secret("app-secret");
    /// ```
    pub fn expect_client_secret(&self, secret: &str) -> &Self {
        self.shared.state().expected_secret = Some(secret.to_owned());
        self
    }

    /// Adds a fault; every fault added stays in force.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::{FakeProvider, Fault};
    ///
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// fake.misbehave(Fault::Expired);
    /// ```
    pub fn misbehave(&self, fault: Fault) -> &Self {
        self.shared.state().faults.push(fault);
        self
    }

    /// The requests the token endpoint received, in order.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::FakeProvider;
    ///
    /// assert!(FakeProvider::new("https://idp.example.test").token_requests().is_empty());
    /// ```
    pub fn token_requests(&self) -> Vec<TokenRequest> {
        self.shared.state().token_requests.clone()
    }
}

impl fmt::Debug for FakeProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeProvider")
            .field("issuer", &self.shared.issuer)
            .finish_non_exhaustive()
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn has(&self, fault: Fault) -> bool {
        self.state().faults.contains(&fault)
    }

    fn answer(&self, request: &Request) -> Option<Response> {
        let path = request.url().strip_prefix(&self.base)?;
        match (request.method(), path) {
            (Method::GET, "/.well-known/openid-configuration") => Some(self.discovery()),
            (Method::GET, "/jwks") => Some(self.jwks()),
            (Method::POST, "/token") => Some(self.token(request)),
            (Method::GET, "/userinfo") => Some(self.user_info(request)),
            _ => None,
        }
    }

    fn discovery(&self) -> Response {
        let issuer = if self.has(Fault::DiscoveryIssuerMismatch) {
            OTHER.to_owned()
        } else {
            self.issuer.clone()
        };
        let base = &self.base;
        json_response(
            StatusCode::OK,
            &json!({
                "issuer": issuer,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "userinfo_endpoint": format!("{base}/userinfo"),
                "jwks_uri": format!("{base}/jwks"),
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["ES256"],
                "code_challenge_methods_supported": ["S256"],
            }),
        )
    }

    fn jwks(&self) -> Response {
        // An uncompressed P-256 point: 0x04, then 32 bytes of x and 32 bytes of y.
        let (x, y) = self.public_point[1..].split_at(32);
        json_response(
            StatusCode::OK,
            &json!({"keys": [{
                "kty": "EC",
                "crv": "P-256",
                "x": URL_SAFE_NO_PAD.encode(x),
                "y": URL_SAFE_NO_PAD.encode(y),
                "kid": self.kid,
                "use": "sig",
                "alg": "ES256",
            }]}),
        )
    }

    fn token(&self, request: &Request) -> Response {
        let fields = match request.body_ref() {
            Body::Form(fields) => fields.clone(),
            _ => Vec::new(),
        };
        let authorization = request
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let recorded = TokenRequest {
            fields,
            authorization,
        };
        let mut state = self.state();
        state.token_requests.push(recorded.clone());
        for fault in &state.faults {
            match *fault {
                Fault::TokenEndpointError(code) => {
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        &json!({"error": code, "error_description": ERROR_DESCRIPTION}),
                    );
                }
                Fault::TokenEndpointStatus(status) => {
                    let status =
                        StatusCode::from_u16(status).expect("a valid HTTP status code in a fault");
                    return Response::new(
                        status,
                        HeaderMap::new(),
                        b"the token endpoint failed".to_vec(),
                    );
                }
                _ => {}
            }
        }
        if recorded.field("grant_type") != Some("authorization_code") {
            return oauth_error("unsupported_grant_type");
        }
        if let Some(expected) = &state.expected_secret
            && client_secret(&recorded).as_deref() != Some(expected.as_str())
        {
            return oauth_error("invalid_client");
        }
        let Some(issued) = recorded
            .field("code")
            .and_then(|code| state.codes.remove(code))
        else {
            return oauth_error("invalid_grant");
        };
        if let Some(expected) = &state.expected_challenge {
            let challenge = recorded
                .field("code_verifier")
                .map(|verifier| URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes())));
            if challenge.as_deref() != Some(expected.as_str()) {
                return oauth_error("invalid_grant");
            }
        }
        if let Some(expected) = &state.expected_redirect_uri
            && recorded.field("redirect_uri") != Some(expected.as_str())
        {
            return oauth_error("invalid_grant");
        }
        let client_id = client_id(&recorded).unwrap_or_default();
        let nonce = issued.nonce.or_else(|| state.nonce.clone());
        let now = state
            .clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let faults = state.faults.clone();
        let access_token = random_token();
        state
            .access_tokens
            .insert(access_token.clone(), issued.user.clone());
        drop(state);

        let id_token = self.id_token(&issued.user, &client_id, nonce.as_deref(), now, &faults);
        let mut body = json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": LIFETIME,
            "refresh_token": random_token(),
            "id_token": id_token,
            "scope": issued.user.scope.as_deref().unwrap_or("openid"),
        });
        if faults.contains(&Fault::OversizedBody) {
            body["padding"] = Value::String("x".repeat(1024 * 1024 + 1024));
        }
        json_response(StatusCode::OK, &body)
    }

    fn id_token(
        &self,
        user: &FakeUser,
        client_id: &str,
        nonce: Option<&str>,
        now: u64,
        faults: &[Fault],
    ) -> String {
        let has = |fault| faults.contains(&fault);
        let mut claims = user.attributes.clone();
        let issuer = if has(Fault::WrongIssuer) {
            OTHER
        } else {
            &self.issuer
        };
        claims.insert("iss".into(), json!(issuer));
        if has(Fault::MissingSub) {
            claims.remove("sub");
        } else {
            claims.insert("sub".into(), json!(user.sub));
        }
        if has(Fault::WrongAudience) {
            claims.insert("aud".into(), json!(OTHER));
        } else if has(Fault::WrongAzp) {
            claims.insert("aud".into(), json!([client_id, OTHER]));
            claims.insert("azp".into(), json!(OTHER));
        } else {
            claims.insert("aud".into(), json!(client_id));
        }
        let (iat, exp) = if has(Fault::Expired) {
            (
                now.saturating_sub(2 * LIFETIME),
                now.saturating_sub(LIFETIME),
            )
        } else if has(Fault::NotYetValid) {
            claims.insert("nbf".into(), json!(now + LIFETIME));
            (now + LIFETIME, now + 2 * LIFETIME)
        } else {
            (now, now + LIFETIME)
        };
        claims.insert("iat".into(), json!(iat));
        claims.insert("exp".into(), json!(exp));
        if has(Fault::WrongNonce) {
            claims.insert("nonce".into(), json!("not-the-expected-nonce"));
        } else if let Some(nonce) = nonce {
            claims.insert("nonce".into(), json!(nonce));
        } else {
            claims.remove("nonce");
        }

        if has(Fault::AlgNone) {
            let header = json!({"alg": "none", "typ": "JWT", "kid": self.kid});
            return format!(
                "{}.{}.",
                URL_SAFE_NO_PAD.encode(header.to_string()),
                URL_SAFE_NO_PAD.encode(Value::Object(claims).to_string())
            );
        }
        if has(Fault::Hs256WithPublicKey) {
            let mut header = Header::new(Algorithm::HS256);
            header.kid = Some(self.kid.clone());
            let key = EncodingKey::from_secret(&self.public_point)
                .expect("an HMAC key from the public key bytes");
            return jsonwebtoken::encode(&header, &claims, &key).expect("signing an id token");
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        jsonwebtoken::encode(&header, &claims, &self.signing_key).expect("signing an id token")
    }

    fn user_info(&self, request: &Request) -> Response {
        if self.has(Fault::UserinfoError) {
            return Response::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                HeaderMap::new(),
                Vec::new(),
            );
        }
        let token = request
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let user = token.and_then(|t| self.state().access_tokens.get(t).cloned());
        match user {
            Some(user) => json_response(StatusCode::OK, &Value::Object(user.user_info())),
            None => Response::new(StatusCode::UNAUTHORIZED, HeaderMap::new(), Vec::new()),
        }
    }
}

/// The client id of a token request: from HTTP Basic (form-URL-decoded, RFC 6749 section
/// 2.3.1), else from the `client_id` field.
fn client_id(request: &TokenRequest) -> Option<String> {
    if let Some(encoded) = request
        .authorization()
        .and_then(|v| v.strip_prefix("Basic "))
    {
        let decoded = STANDARD.decode(encoded).ok()?;
        let text = String::from_utf8(decoded).ok()?;
        let (id, _) = text.split_once(':')?;
        return Some(form_decode(id));
    }
    request.field("client_id").map(str::to_owned)
}

/// The client secret of a token request: from HTTP Basic (form-URL-decoded), else from the
/// `client_secret` field.
fn client_secret(request: &TokenRequest) -> Option<String> {
    if let Some(encoded) = request
        .authorization()
        .and_then(|v| v.strip_prefix("Basic "))
    {
        let decoded = STANDARD.decode(encoded).ok()?;
        let text = String::from_utf8(decoded).ok()?;
        let (_, secret) = text.split_once(':')?;
        return Some(form_decode(secret));
    }
    request.field("client_secret").map(str::to_owned)
}

/// Decodes `application/x-www-form-urlencoded` text: `+` is a space and `%XX` a byte.
fn form_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = |b: u8| char::from(b).to_digit(16);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(high), Some(low)) => {
                        // Two hex digits make one byte, so the cast cannot truncate.
                        #[allow(clippy::cast_possible_truncation)]
                        out.push((high * 16 + low) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Response::new(status, headers, body.to_string().into_bytes())
}

fn oauth_error(code: &str) -> Response {
    json_response(StatusCode::BAD_REQUEST, &json!({"error": code}))
}

/// 24 random bytes as base64url without padding.
fn random_token() -> String {
    let mut bytes = [0u8; 24];
    aws_lc_rs::rand::fill(&mut bytes).expect("the system random number generator failed");
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::form_decode;

    #[test]
    fn form_decoding_handles_plus_percent_and_a_trailing_percent() {
        assert_eq!(form_decode("my+client%3A1"), "my client:1");
        assert_eq!(form_decode("a%"), "a%");
        assert_eq!(form_decode("a%4"), "a%4");
        assert_eq!(form_decode("%zz"), "%zz");
    }
}
