//! End-to-end tests: a real server on loopback, raw HTTP/1.1 over TCP.

use std::net::SocketAddr;
use std::sync::Arc;

use jsonwebtoken::{EncodingKey, Header, encode, get_current_timestamp};
use oxbloom_config::Secret;
use oxbloom_security::{
    Algorithm, HttpSecurity, JwtAuthenticationConverter, JwtDecoderBuilder, RequestSecurityExt,
};
use oxbloom_web::{App, HttpMethod, Request, Route};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const SECRET: &[u8] = b"an-end-to-end-secret-of-32-bytes!!";

struct Server {
    addr: SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Server {
    async fn start(app: App) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(app.serve(listener, async {
            let _ = stopped.await;
        }));
        Self { addr, stop, task }
    }

    async fn shutdown(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap().unwrap();
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    raw: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

async fn request(addr: SocketAddr, method: &str, path: &str, token: Option<&str>) -> Reply {
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    if let Some(token) = token {
        text.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    text.push_str("\r\n");
    let mut conn = TcpStream::connect(addr).await.unwrap();
    conn.write_all(text.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    conn.read_to_end(&mut bytes).await.unwrap();
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").expect("complete response");
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Reply {
        status,
        headers,
        body: body.to_owned(),
        raw,
    }
}

fn sign_hs(claims: &serde_json::Value) -> String {
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(SECRET).unwrap(),
    )
    .unwrap()
}

fn rules(security: HttpSecurity) -> HttpSecurity {
    security.authorize(|a| {
        a.get("/public/**")
            .permit_all()
            .path("/admin/**")
            .has_role("ADMIN")
            .any_request()
            .authenticated()
    })
}

fn echo_app(chain: oxbloom_security::SecurityFilterChain) -> App {
    let who = |req: Request| async move {
        req.authentication()
            .map(|a| a.principal().to_owned())
            .unwrap_or_default()
    };
    App::new()
        .filter(chain)
        .route(Route::new(HttpMethod::Get, "/public/x", who))
        .unwrap()
        .route(Route::new(HttpMethod::Get, "/api/x", who))
        .unwrap()
        .route(Route::new(HttpMethod::Get, "/admin/x", who))
        .unwrap()
}

fn hs_app() -> App {
    let decoder = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(SECRET.to_vec()))
        .build()
        .unwrap();
    let chain = rules(HttpSecurity::new())
        .oauth2_resource_server(|r| {
            r.jwt(Arc::new(decoder)).converter(
                JwtAuthenticationConverter::default().authorities_claim("roles", "ROLE_"),
            )
        })
        .build()
        .unwrap();
    echo_app(chain)
}

fn token(roles: &[&str], exp_offset: i64) -> String {
    let exp = get_current_timestamp()
        .checked_add_signed(exp_offset)
        .unwrap();
    sign_hs(&json!({ "sub": "alice", "exp": exp, "roles": roles }))
}

#[tokio::test]
async fn jwt_protected_app_over_real_http() {
    let server = Server::start(hs_app()).await;
    let addr = server.addr;
    let valid = token(&["USER"], 300);
    let admin = token(&["ADMIN"], 300);
    let expired = token(&["ADMIN"], -3600);
    let mut seen: Vec<Reply> = Vec::new();

    let r = request(addr, "GET", "/public/x", None).await;
    assert_eq!(r.status, 200);
    seen.push(r);

    let r = request(addr, "GET", "/api/x", None).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.header("www-authenticate"), Some("Bearer"));
    seen.push(r);

    let r = request(addr, "GET", "/api/x", Some(&expired)).await;
    assert_eq!(r.status, 401);
    assert!(
        r.header("www-authenticate")
            .unwrap()
            .contains("error=\"invalid_token\"")
    );
    seen.push(r);

    let r = request(addr, "GET", "/api/x", Some(&valid)).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, "alice");
    seen.push(r);

    let r = request(addr, "GET", "/admin/x", Some(&valid)).await;
    assert_eq!(r.status, 403);
    // Rendered by the app's error pipeline as problem+json.
    assert!(r.body.contains("\"status\":403"), "{}", r.body);
    seen.push(r);

    let r = request(addr, "GET", "/admin/x", Some(&admin)).await;
    assert_eq!(r.status, 200);
    seen.push(r);

    // An anonymous client cannot probe routes: an unknown path is denied, not 404.
    let r = request(addr, "GET", "/nope/at/all", None).await;
    assert_eq!(r.status, 401);
    seen.push(r);

    // HEAD is authorized as GET: a `get(..)` permit also governs HEAD, and an anonymous HEAD to a
    // protected path is denied.
    let r = request(addr, "HEAD", "/public/x", None).await;
    assert_eq!(r.status, 200);
    seen.push(r);
    let r = request(addr, "HEAD", "/api/x", None).await;
    assert_eq!(r.status, 401);
    seen.push(r);

    // No response may echo any token text.
    for reply in &seen {
        for t in [&valid, &admin, &expired] {
            assert!(
                !reply.raw.contains(t.as_str()),
                "token leaked into a response"
            );
        }
    }
    server.shutdown().await;
}

/// Records, per request, its name and whether the request was authenticated when it passed.
struct Spy(
    &'static str,
    Arc<std::sync::Mutex<Vec<(&'static str, bool)>>>,
);

#[async_trait::async_trait]
impl oxbloom_web::Filter for Spy {
    async fn filter(&self, req: Request, next: oxbloom_web::Next<'_>) -> oxbloom_web::Response {
        let authenticated = req.authentication().is_some();
        self.1.lock().unwrap().push((self.0, authenticated));
        next.run(req).await
    }
}

#[tokio::test]
async fn a_filter_ordered_before_security_sees_the_request_unauthenticated() {
    use oxbloom_web::order;

    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let app = hs_app()
        .filter_ordered(order::SECURITY - 50, Spy("early", seen.clone()))
        .filter(Spy("late", seen.clone()));
    let server = Server::start(app).await;

    // A valid token: the early filter runs before the chain authenticated the request, the
    // ordinary filter after it.
    let r = request(server.addr, "GET", "/api/x", Some(&token(&["USER"], 300))).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, "alice");
    assert_eq!(*seen.lock().unwrap(), [("early", false), ("late", true)]);

    // No token: the early filter still runs; the chain answers 401 and the ordinary filter is
    // never reached.
    seen.lock().unwrap().clear();
    let r = request(server.addr, "GET", "/api/x", None).await;
    assert_eq!(r.status, 401);
    assert_eq!(*seen.lock().unwrap(), [("early", false)]);
    server.shutdown().await;
}

#[cfg(feature = "jwks")]
mod jwks {
    use std::time::Duration;

    use jsonwebtoken::DecodingKey;
    use jsonwebtoken::jwk::{Jwk, JwkSet, PublicKeyUse};
    use oxbloom_security::{JwksSource, ReqwestJwksFetcher};

    use super::*;

    const RSA_PRIVATE: &str = include_str!("fixtures/rsa_private.pem");
    const RSA_PUBLIC: &str = include_str!("fixtures/rsa_public.pem");

    fn sign_rs(kid: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_owned());
        let exp = get_current_timestamp() + 300;
        encode(
            &header,
            &json!({ "sub": "bob", "exp": exp }),
            &EncodingKey::from_rsa_pem(RSA_PRIVATE.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn jwks_resource_server_over_real_http() {
        // Issuer: a loopback app that serves the JWK set.
        let key = DecodingKey::from_rsa_pem(RSA_PUBLIC.as_bytes()).unwrap();
        let mut jwk = Jwk::from_decoding_key(&key, Some(Algorithm::RS256)).unwrap();
        jwk.common.key_id = Some("key-1".into());
        jwk.common.public_key_use = Some(PublicKeyUse::Signature);
        let body = serde_json::to_string(&JwkSet { keys: vec![jwk] }).unwrap();
        let issuer = App::new()
            .route(Route::new(HttpMethod::Get, "/jwks.json", move |_| {
                let body = body.clone();
                async move { body }
            }))
            .unwrap();
        let issuer = Server::start(issuer).await;
        let uri = format!("http://{}/jwks.json", issuer.addr);

        let source = JwksSource::builder(Arc::new(ReqwestJwksFetcher::new().unwrap()), uri)
            .allow_insecure_http()
            .fetch_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let decoder = JwtDecoderBuilder::with_jwks(source, vec![Algorithm::RS256])
            .build()
            .unwrap();
        let chain = rules(HttpSecurity::new())
            .oauth2_resource_server(|r| r.jwt(Arc::new(decoder)))
            .build()
            .unwrap();
        let api = Server::start(echo_app(chain)).await;

        let good = sign_rs("key-1");
        let r = request(api.addr, "GET", "/api/x", Some(&good)).await;
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "bob");

        let unknown = sign_rs("other-kid");
        let r = request(api.addr, "GET", "/api/x", Some(&unknown)).await;
        assert_eq!(r.status, 401);
        assert!(
            r.header("www-authenticate")
                .unwrap()
                .contains("invalid_token")
        );
        assert!(!r.raw.contains(&unknown));

        api.shutdown().await;
        issuer.shutdown().await;
    }
}
