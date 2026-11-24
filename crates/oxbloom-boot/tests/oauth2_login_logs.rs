//! No client secret, token, code, state, nonce or verifier reaches the startup failure report
//! or the log lines of a successful OAuth2 login. A binary of its own: the log subscriber is
//! global, so one test installs it at debug level and runs both scenarios in order.

#![cfg(all(feature = "oauth2", feature = "logging", feature = "web"))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use oxbloom_boot::{
    Banner, JwtDecoderAutoConfiguration, OxbloomApplication, OxbloomApplicationBuilder,
    PasswordEncoderAutoConfiguration, RunListener, SecurityAutoConfiguration,
    SessionAutoConfiguration,
};
use oxbloom_config::Config;
use oxbloom_di::Container;
use oxbloom_http_client::HttpClient;
use oxbloom_security::{RequestSecurityExt, RequestSessionExt, SecurityFilterChain, SessionFilter};
use oxbloom_test::{Captured, FakeProvider, FakeUser, MockHttpClient, TestClient, TestResponse};
use oxbloom_web::{App, HttpMethod, Request, Route};
use serde_json::Value;

const ISSUER: &str = "https://idp.example.test";
const BASE: &str = "https://app.example.test";
const SECRET: &str = "app-secret-that-must-never-be-logged";
const COOKIE: &str = "OXBLOOM_SESSION";
const TOKENS_KEY: &str = "oxbloom.security.oauth2.tokens.idp";

/// Debug logs for every target, without colors.
const LOGGING: &str = "[logging.console]\ncolor = \"never\"\n[logging.level]\nroot = \"debug\"\n";

fn oauth2(registration: &str) -> String {
    format!(
        "{LOGGING}[security.form-login]\nenabled = true\n[security.session]\n\
         [security.oauth2]\nbase-url = \"{BASE}\"\n\
         [security.oauth2.client.registration.idp]\n{registration}\
         client-secret = \"{SECRET}\"\nscope = [\"email\"]\n\
         [security.oauth2.client.provider.idp]\nissuer-uri = \"{ISSUER}\"\n"
    )
}

fn app(
    toml: &str,
    out: &Captured,
    logs: &Captured,
    client: Arc<MockHttpClient>,
) -> OxbloomApplicationBuilder {
    let logs = logs.clone();
    OxbloomApplication::builder()
        .with_config(Config::from_str(toml).unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .failure_report(true)
        .logging_writer(move || logs.clone())
        .auto_configuration::<JwtDecoderAutoConfiguration>()
        .auto_configuration::<PasswordEncoderAutoConfiguration>()
        .auto_configuration::<SessionAutoConfiguration>()
        .auto_configuration::<SecurityAutoConfiguration>()
        .configure(move |b| {
            b.instance(client)
                .provides(|c: Arc<Arc<MockHttpClient>>| -> Arc<dyn HttpClient> { (*c).clone() })
        })
}

/// The session filter and the chain of the container.
#[derive(Clone, Default)]
#[allow(clippy::type_complexity)]
struct Beans(Arc<Mutex<Option<(Arc<SessionFilter>, Arc<SecurityFilterChain>)>>>);

impl RunListener for Beans {
    fn context_ready(&self, c: &Container) {
        let session = c.get::<SessionFilter>().unwrap();
        let chain = c.get::<SecurityFilterChain>().unwrap();
        *self.0.lock().unwrap() = Some((session, chain));
    }
}

/// The query parameters of a URL, percent-decoded.
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

fn assert_absent(text: &str, values: &[(&str, String)]) {
    for (name, value) in values {
        assert!(!value.is_empty(), "{name} is empty");
        assert!(!text.contains(value.as_str()), "the {name} leaked: {text}");
    }
}

#[tokio::test]
async fn no_secret_token_or_code_reaches_the_report_or_the_logs() {
    let logs = Captured::new();

    // A broken registration (no client id) fails the startup with the key, and neither the
    // report nor the log lines hold the secret.
    let out = Captured::new();
    let fake = FakeProvider::new(ISSUER);
    let error = app(&oauth2(""), &out, &logs, fake.client())
        .run()
        .await
        .unwrap_err();
    let report = out.contents();
    assert!(
        report.contains("security.oauth2.client.registration.idp.client-id"),
        "{report}"
    );
    let all = format!("{error} {error:?} {report} {}", logs.contents());
    assert_absent(&all, &[("client secret", SECRET.to_owned())]);

    // A successful login at debug level.
    let fake = FakeProvider::new(ISSUER);
    fake.expect_client_secret(SECRET);
    let beans = Beans::default();
    app(
        &oauth2("client-id = \"app\"\n"),
        &Captured::new(),
        &logs,
        fake.client(),
    )
    .listener(beans.clone())
    .run()
    .await
    .unwrap();
    let (session, chain) = beans
        .0
        .lock()
        .unwrap()
        .clone()
        .expect("context_ready fired");
    let web = App::new()
        .filter(session)
        .filter(chain)
        .route(Route::new(
            HttpMethod::Get,
            "/me",
            |req: Request| async move {
                req.authentication()
                    .map(|a| a.principal().to_string())
                    .unwrap_or_default()
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
    assert_eq!(get(&client, &jar, "/me").await.text(), "alice");
    let tokens: Value = serde_json::from_str(&get(&client, &jar, "/tokens").await.text()).unwrap();
    let token = |name: &str| tokens[name].as_str().unwrap_or_default().to_owned();
    let verifier = fake.token_requests()[0]
        .field("code_verifier")
        .unwrap_or_default()
        .to_owned();
    let text = logs.contents();
    // The login was logged, so the check below looks at real lines.
    assert!(text.contains("alice"), "{text}");
    assert_absent(
        &text,
        &[
            ("client secret", SECRET.to_owned()),
            ("code", code),
            ("state", params["state"].clone()),
            ("nonce", params["nonce"].clone()),
            ("code challenge", params["code_challenge"].clone()),
            ("code verifier", verifier),
            ("access token", token("access_token")),
            ("refresh token", token("refresh_token")),
            ("id token", token("id_token")),
        ],
    );
}
