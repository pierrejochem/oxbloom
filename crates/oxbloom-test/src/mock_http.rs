use std::collections::VecDeque;
use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use oxbloom_http_client::{HttpClient, HttpClientError, Request, Response};
use serde::Serialize;

/// A scripted answer for [`MockHttpClient`], built with a status and then optional headers
/// and a body.
///
/// Its `Debug` shows the status only, never the headers or the body.
///
/// # Examples
///
/// ```
/// use oxbloom_test::MockResponse;
///
/// let response = MockResponse::status(200)
///     .header("content-type", "application/json")
///     .json(&serde_json::json!({"ok": true}));
/// # let _ = response;
/// ```
#[derive(Clone)]
pub struct MockResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl MockResponse {
    /// An answer with `code` and no headers or body.
    ///
    /// # Panics
    ///
    /// Panics when `code` is not a status code (below 100 or above 999), which is a mistake
    /// in the test.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::MockResponse;
    ///
    /// let response = MockResponse::status(404);
    /// # let _ = response;
    /// ```
    pub fn status(code: u16) -> MockResponse {
        MockResponse {
            status: StatusCode::from_u16(code).expect("a valid HTTP status code"),
            headers: HeaderMap::new(),
            body: Vec::new(),
        }
    }

    /// Adds the header `name` with `value`, replacing an earlier value of that name.
    ///
    /// # Panics
    ///
    /// Panics when the name or the value is not valid in a header, which is a mistake in the
    /// test.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::MockResponse;
    ///
    /// let response = MockResponse::status(302).header("location", "https://example.test/next");
    /// # let _ = response;
    /// ```
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> MockResponse {
        self.headers.insert(
            HeaderName::from_bytes(name.as_bytes()).expect("a valid header name"),
            HeaderValue::from_str(value).expect("a valid header value"),
        );
        self
    }

    /// Sets the body to the JSON text of `value`. It does not set a `Content-Type`; add one
    /// with [`header`](Self::header) when the code under test reads it.
    ///
    /// # Panics
    ///
    /// Panics when `value` cannot be serialized, which is a mistake in the test.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::MockResponse;
    ///
    /// let response = MockResponse::status(200).json(&serde_json::json!({"sub": "alice"}));
    /// # let _ = response;
    /// ```
    #[must_use]
    pub fn json<T: Serialize>(mut self, value: &T) -> MockResponse {
        self.body = serde_json::to_vec(value).expect("a serializable value");
        self
    }

    /// Sets the body to `text`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::MockResponse;
    ///
    /// let response = MockResponse::status(200).text("pong");
    /// # let _ = response;
    /// ```
    #[must_use]
    pub fn text(mut self, text: &str) -> MockResponse {
        self.body = text.as_bytes().to_vec();
        self
    }

    /// Sets the body to `bytes`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::MockResponse;
    ///
    /// let response = MockResponse::status(200).bytes(vec![0xde, 0xad]);
    /// # let _ = response;
    /// ```
    #[must_use]
    pub fn bytes(mut self, bytes: Vec<u8>) -> MockResponse {
        self.body = bytes;
        self
    }

    fn into_response(self) -> Response {
        Response::new(self.status, self.headers, self.body)
    }
}

impl From<Response> for MockResponse {
    /// Scripts a [`Response`] as it is.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Response;
    /// use oxbloom_test::MockResponse;
    ///
    /// let response = Response::new(http::StatusCode::OK, http::HeaderMap::new(), b"ok".to_vec());
    /// let mock: MockResponse = response.into();
    /// # let _ = mock;
    /// ```
    fn from(response: Response) -> MockResponse {
        MockResponse {
            status: response.status(),
            headers: response.headers().clone(),
            body: response.bytes().to_vec(),
        }
    }
}

impl fmt::Debug for MockResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockResponse")
            .field("status", &self.status.as_u16())
            .finish_non_exhaustive()
    }
}

type Handler = Box<dyn Fn(&Request) -> Option<Response> + Send + Sync>;

enum Rule {
    /// The answers for one method and URL; `next` is the index of the one served next.
    Exact {
        method: Method,
        url: String,
        answers: VecDeque<Response>,
        next: usize,
    },
    Func(Handler),
}

#[derive(Default)]
struct State {
    rules: Vec<Rule>,
    requests: Vec<Request>,
    unmatched: Vec<Request>,
}

/// An [`HttpClient`] with scripted answers, for tests of code that calls an HTTP API.
///
/// [`on`](Self::on) answers a method and an exact URL; calling it again for the same method
/// and URL adds a later answer, answers are served in order and the last one repeats.
/// [`on_fn`](Self::on_fn) answers from the request. The rules are tried in the order they
/// were added and the first that has an answer wins. A request that no rule answers gets
/// [`HttpClientError::Network`]. Every valid request is recorded, answered or not, and
/// [`requests`](Self::requests) returns them; [`unmatched_requests`](Self::unmatched_requests)
/// returns the ones no rule answered. Like every client, `execute` first calls
/// `Request::validate`, so an invalid request is an error and is not recorded.
///
/// The mock holds its state behind a mutex, so it can be shared as an `Arc<dyn HttpClient>`
/// and scripted through a shared reference. Available with the cargo feature `http-client`.
/// Its `Debug` shows the counts only, never a URL, a header or a body.
///
/// # Examples
///
/// ```
/// use http::Method;
/// use oxbloom_http_client::{HttpClient, Request};
/// use oxbloom_test::{MockHttpClient, MockResponse};
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let mock = MockHttpClient::new();
/// mock.on(Method::GET, "https://idp.example.test/jwks", MockResponse::status(200).text("{}"));
///
/// let response = mock.execute(Request::get("https://idp.example.test/jwks")).await?;
/// assert_eq!(response.text()?, "{}");
/// assert_eq!(mock.request_count(), 1);
/// assert_eq!(mock.requests()[0].url(), "https://idp.example.test/jwks");
/// # Ok::<(), oxbloom_http_client::HttpClientError>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Default)]
pub struct MockHttpClient {
    state: Mutex<State>,
}

impl MockHttpClient {
    /// A mock without any rule: every request gets `HttpClientError::Network`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::MockHttpClient;
    ///
    /// assert_eq!(MockHttpClient::new().request_count(), 0);
    /// ```
    pub fn new() -> MockHttpClient {
        MockHttpClient::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Answers `method` requests for exactly `url` with `response`.
    ///
    /// The URL must match exactly, query and fragment included. A second call for the same
    /// method and URL adds a later answer: the first request gets the first answer, the next
    /// one the second, and the last answer repeats from then on.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::Method;
    /// use oxbloom_http_client::{HttpClient, Request};
    /// use oxbloom_test::{MockHttpClient, MockResponse};
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let mock = MockHttpClient::new();
    /// mock.on(Method::GET, "https://api.example.test/", MockResponse::status(503))
    ///     .on(Method::GET, "https://api.example.test/", MockResponse::status(200));
    /// let first = mock.execute(Request::get("https://api.example.test/")).await?;
    /// let second = mock.execute(Request::get("https://api.example.test/")).await?;
    /// assert_eq!(first.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    /// assert_eq!(second.status(), http::StatusCode::OK);
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn on(&self, method: Method, url: &str, response: impl Into<MockResponse>) -> &Self {
        let response = response.into().into_response();
        let mut state = self.state();
        let existing = state.rules.iter_mut().find_map(|rule| match rule {
            Rule::Exact {
                method: m,
                url: u,
                answers,
                ..
            } if *m == method && u == url => Some(answers),
            _ => None,
        });
        match existing {
            Some(answers) => answers.push_back(response),
            None => state.rules.push(Rule::Exact {
                method,
                url: url.to_owned(),
                answers: VecDeque::from([response]),
                next: 0,
            }),
        }
        drop(state);
        self
    }

    /// Answers with `f`: it gets the request and returns the response, or `None` to let a
    /// later rule answer.
    ///
    /// The closure runs while the mock's state lock is held, so it must not call the same
    /// mock (for example through `execute`, `requests` or `on`): that would deadlock.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::{HttpClient, Request, Response};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let mock = MockHttpClient::new();
    /// mock.on_fn(|request| {
    ///     request.url().ends_with("/ping").then(|| {
    ///         Response::new(http::StatusCode::OK, http::HeaderMap::new(), b"pong".to_vec())
    ///     })
    /// });
    /// let response = mock.execute(Request::get("https://api.example.test/ping")).await?;
    /// assert_eq!(response.text()?, "pong");
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn on_fn(&self, f: impl Fn(&Request) -> Option<Response> + Send + Sync + 'static) -> &Self {
        self.state().rules.push(Rule::Func(Box::new(f)));
        self
    }

    /// The requests received so far, in order, answered or not.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::{HttpClient, Request};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let mock = MockHttpClient::new();
    /// let _ = mock.execute(Request::get("https://api.example.test/a")).await;
    /// let seen = mock.requests();
    /// assert_eq!(seen.len(), 1);
    /// assert_eq!(seen[0].url(), "https://api.example.test/a");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn requests(&self) -> Vec<Request> {
        self.state().requests.clone()
    }

    /// The requests that no rule answered, in order. They were also answered with
    /// `HttpClientError::Network`, and they are in [`requests`](Self::requests) too.
    ///
    /// A test of a failure path should assert that this is empty: otherwise a mistyped URL
    /// in the rule also ends in an error, and the negative test passes for the wrong reason.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::Method;
    /// use oxbloom_http_client::{HttpClient, Request};
    /// use oxbloom_test::{MockHttpClient, MockResponse};
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let mock = MockHttpClient::new();
    /// mock.on(Method::POST, "https://idp.example.test/token", MockResponse::status(401));
    ///
    /// // The code under test must fail on the 401, not on an unanswered request.
    /// let response = mock.execute(Request::post("https://idp.example.test/token")).await?;
    /// assert!(response.error_for_status().is_err());
    /// assert!(mock.unmatched_requests().is_empty());
    ///
    /// let _ = mock.execute(Request::post("https://idp.example.test/tokn")).await;
    /// assert_eq!(mock.unmatched_requests()[0].url(), "https://idp.example.test/tokn");
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn unmatched_requests(&self) -> Vec<Request> {
        self.state().unmatched.clone()
    }

    /// How many requests were received so far.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::MockHttpClient;
    ///
    /// assert_eq!(MockHttpClient::new().request_count(), 0);
    /// ```
    pub fn request_count(&self) -> usize {
        self.state().requests.len()
    }
}

impl fmt::Debug for MockHttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state();
        f.debug_struct("MockHttpClient")
            .field("rules", &state.rules.len())
            .field("requests", &state.requests.len())
            .finish()
    }
}

#[async_trait]
impl HttpClient for MockHttpClient {
    /// Validates the request, records it and answers it from the first matching rule, or fails
    /// with [`HttpClientError::Network`].
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
        request.validate()?;
        let mut state = self.state();
        state.requests.push(request.clone());
        let State {
            rules, unmatched, ..
        } = &mut *state;
        for rule in rules.iter_mut() {
            match rule {
                Rule::Exact {
                    method,
                    url,
                    answers,
                    next,
                } if *method == request.method() && url == request.url() => {
                    let index = (*next).min(answers.len() - 1);
                    *next += 1;
                    return Ok(answers[index].clone());
                }
                Rule::Func(f) => {
                    if let Some(response) = f(&request) {
                        return Ok(response);
                    }
                }
                Rule::Exact { .. } => {}
            }
        }
        unmatched.push(request);
        Err(HttpClientError::Network)
    }
}
