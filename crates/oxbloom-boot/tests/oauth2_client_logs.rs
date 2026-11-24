//! No client secret, access token, refresh token or id token reaches the log lines of the
//! successful flows of the OAuth2 client bean: client credentials through a
//! `BearerRetryClient`, an OAuth2 login, and the refresh of the user's token. A binary of its
//! own: the log subscriber is global, so one test installs it at debug level.

#![cfg(all(feature = "oauth2", feature = "logging", feature = "web"))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use http::Method;
use oxbloom_boot::{
    Banner, JwtDecoderAutoConfiguration, OAuth2ClientAutoConfiguration, OxbloomApplication,
    PasswordEncoderAutoConfiguration, RunListener, SecurityAutoConfiguration,
    SessionAutoConfiguration,
};
use oxbloom_config::Config;
use oxbloom_core::Clock;
use oxbloom_di::Container;
use oxbloom_http_client::{HttpClient, Request as OutRequest};
use oxbloom_security::{
    BearerRetryClient, OAuth2Client, RequestSessionExt, SecurityFilterChain, SessionFilter,
};
use oxbloom_test::{
    Captured, FakeProvider, FakeUser, ManualClock, MockHttpClient, MockResponse, TestClient,
    TestResponse,
};
use oxbloom_web::{App, HttpMethod, Request, Route};
use serde_json::Value;

const ISSUER: &str = "https://idp.example.test";
const BASE: &str = "https://app.example.test";
const SECRET: &str = "client-secret-that-must-never-be-logged";
const COOKIE: &str = "OXBLOOM_SESSION";
const TOKENS_KEY: &str = "oxbloom.security.oauth2.tokens.idp";
const DOWNSTREAM: &str = "https://orders.example.test/orders";

/// Debug logs for every target, without colors.
const LOGGING: &str = "[logging.console]\ncolor = \"never\"\n[logging.level]\nroot = \"debug\"\n";

fn config() -> String {
    format!(
        "{LOGGING}[security.form-login]\nenabled = true\n[security.session]\n\
         [security.oauth2]\nbase-url = \"{BASE}\"\n\
         [security.oauth2.client.registration.idp]\nclient-id = \"app\"\n\
         client-secret = \"{SECRET}\"\nscope = [\"email\"]\n\
         [security.oauth2.client.registration.billing]\nclient-id = \"orders-service\"\n\
         client-secret = \"{SECRET}\"\nprovider = \"idp\"\n\
         [security.oauth2.client.provider.idp]\nissuer-uri = \"{ISSUER}\"\n"
    )
}

/// The session filter, the chain and the OAuth2 client of the container.
#[derive(Clone, Default)]
#[allow(clippy::type_complexity)]
struct Beans(
    Arc<
        Mutex<
            Option<(
                Arc<SessionFilter>,
                Arc<SecurityFilterChain>,
                Arc<OAuth2Client>,
            )>,
        >,
    >,
);

impl RunListener for Beans {
    fn context_ready(&self, c: &Container) {
        let session = c.get::<SessionFilter>().unwrap();
        let chain = c.get::<SecurityFilterChain>().unwrap();
        let client = c.get::<OAuth2Client>().unwrap();
        *self.0.lock().unwrap() = Some((session, chain, client));
    }
}

fn query(url: &str) -> HashMap<String, String> {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else {
            out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// Sends a GET with the session cookie in `jar` and keeps the cookie of the answer.
async fn get(client: &TestClient, jar: &Mutex<Option<String>>, path: &str) -> TestResponse {
    let mut req = client.get(path).header("accept", "text/html");
    if let Some(id) = jar.lock().unwrap().clone() {
        req = req.header("cookie", format!("{COOKIE}={id}"));
    }
    let res = req.send().await;
    for value in res.header_all("set-cookie") {
        if let Some(rest) = value.strip_prefix(&format!("{COOKIE}=")) {
            *jar.lock().unwrap() = Some(rest.split("; ").next().unwrap().to_owned());
        }
    }
    res
}

fn tokens_of(text: &str) -> Vec<(&'static str, String)> {
    let tokens: Value = serde_json::from_str(text).unwrap();
    let token = |name: &str| tokens[name].as_str().unwrap_or_default().to_owned();
    vec![
        ("access token", token("access_token")),
        ("refresh token", token("refresh_token")),
        ("id token", token("id_token")),
    ]
}

#[tokio::test]
async fn no_secret_or_token_of_the_client_flows_reaches_the_logs() {
    let logs = Captured::new();
    let clock = ManualClock::at(SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000));
    let fake = FakeProvider::new(ISSUER).clock(Arc::new(clock.clone()));
    fake.expect_client_secret(SECRET);
    fake.access_token_lifetime(Some(600));
    let provider = fake.client();
    let app_clock = clock.clone();
    let beans = Beans::default();
    let sink = logs.clone();
    OxbloomApplication::builder()
        .with_config(Config::from_str(&config()).unwrap())
        .banner(Banner::off())
        .output(Captured::new())
        .logging_writer(move || sink.clone())
        .auto_configuration::<JwtDecoderAutoConfiguration>()
        .auto_configuration::<PasswordEncoderAutoConfiguration>()
        .auto_configuration::<SessionAutoConfiguration>()
        .auto_configuration::<SecurityAutoConfiguration>()
        .auto_configuration::<OAuth2ClientAutoConfiguration>()
        .configure(move |b| {
            b.instance(provider)
                .provides(|c: Arc<Arc<MockHttpClient>>| -> Arc<dyn HttpClient> { (*c).clone() })
                .instance(app_clock)
                .provides(|c: Arc<ManualClock>| -> Arc<dyn Clock> { c })
        })
        .listener(beans.clone())
        .run()
        .await
        .unwrap();
    let (session, chain, oauth2) = beans
        .0
        .lock()
        .unwrap()
        .clone()
        .expect("context_ready fired");

    let api = Arc::new(MockHttpClient::new());
    api.on(
        Method::GET,
        DOWNSTREAM,
        MockResponse::status(200).text("orders"),
    );
    let downstream = BearerRetryClient::new(api.clone(), oauth2.clone(), "billing");
    let user_client = oauth2.clone();
    let web = App::new()
        .filter(session)
        .filter(chain)
        .route(Route::new(
            HttpMethod::Get,
            "/user-token",
            move |req: Request| {
                let client = user_client.clone();
                async move {
                    match client.access_token_for(&req, "idp").await {
                        Ok(token) => token.value.expose().clone(),
                        Err(error) => format!("error: {error}"),
                    }
                }
            },
        ))
        .unwrap()
        .route(Route::new(
            HttpMethod::Get,
            "/tokens",
            |req: Request| async move {
                req.session()
                    .and_then(|s| s.get::<Value>(TOKENS_KEY).ok().flatten())
                    .unwrap_or(Value::Null)
                    .to_string()
            },
        ))
        .unwrap();
    let client = TestClient::in_memory(web);

    // Client credentials, twice, through the retrying bearer client.
    for _ in 0..2 {
        let response = downstream
            .execute(OutRequest::get(DOWNSTREAM))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    let bearer = api.requests()[0].headers()["authorization"]
        .to_str()
        .unwrap()
        .to_owned();
    let client_token = bearer.strip_prefix("Bearer ").unwrap().to_owned();

    // An OAuth2 login.
    let jar = Mutex::new(None);
    let res = get(&client, &jar, "/oauth2/authorization/idp").await;
    let params = query(res.header("location").unwrap());
    fake.expect_pkce(&params["code_challenge"]);
    fake.expect_nonce(&params["nonce"]);
    let code = fake.issue_code(FakeUser::new("alice").scope("openid email"));
    let res = get(
        &client,
        &jar,
        &format!(
            "/login/oauth2/code/idp?code={code}&state={}",
            params["state"]
        ),
    )
    .await;
    assert_eq!(res.header("location"), Some("/"));
    let login = tokens_of(&get(&client, &jar, "/tokens").await.text());

    // The refresh of the user's token past its expiry.
    clock.advance(Duration::from_secs(600));
    let renewed = get(&client, &jar, "/user-token").await.text();
    assert!(!renewed.starts_with("error"), "{renewed}");
    let refreshed = tokens_of(&get(&client, &jar, "/tokens").await.text());
    assert_eq!(refreshed[0].1, renewed);
    assert_eq!(
        fake.token_requests()
            .iter()
            .filter(|r| r.field("grant_type") == Some("refresh_token"))
            .count(),
        1
    );

    let text = logs.contents();
    // The flows were logged, so the check below looks at real lines.
    assert!(text.contains("refreshed"), "{text}");
    let mut hidden = vec![
        ("client secret", SECRET.to_owned()),
        ("client credentials token", client_token),
    ];
    // The Basic header form of the secret, `base64("<client-id>:<secret>")`, exactly as the
    // client sent it for both registrations.
    let mut basics: Vec<String> = fake
        .token_requests()
        .iter()
        .filter_map(|r| r.authorization())
        .filter_map(|a| a.strip_prefix("Basic ").map(str::to_owned))
        .collect();
    basics.sort();
    basics.dedup();
    assert_eq!(basics.len(), 2, "one Basic credential per registration");
    hidden.extend(basics.into_iter().map(|b| ("Basic credentials", b)));
    hidden.extend(login);
    hidden.extend(refreshed);
    for (name, value) in &hidden {
        assert!(!value.is_empty(), "{name} is empty");
        assert!(!text.contains(value.as_str()), "the {name} leaked: {text}");
    }
}
