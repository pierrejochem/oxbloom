//! Cross-origin resource sharing: [`CorsConfig`], [`CorsFilter`] and [`CorsError`].

use std::fmt;
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};

use crate::{Filter, HttpMethod, Next, Request, Response, order};

/// The settings of a [`CorsFilter`], built step by step and checked by [`CorsFilter::new`].
///
/// Defaults: no allowed origin, methods `GET, HEAD, POST`, no allowed or exposed headers,
/// credentials off and a preflight max age of 30 minutes.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_web::{CorsConfig, CorsFilter};
///
/// let config = CorsConfig::new()
///     .allowed_origins(["https://app.example"])
///     .allowed_methods(["GET", "PUT"])
///     .allowed_headers(["Content-Type"])
///     .exposed_headers(["X-Total"])
///     .allow_credentials(true)
///     .max_age(Duration::from_secs(600));
/// assert!(CorsFilter::new(config).is_ok());
/// ```
#[derive(Debug, Clone)]
pub struct CorsConfig {
    origins: Vec<String>,
    methods: Vec<String>,
    headers: Vec<String>,
    exposed: Vec<String>,
    credentials: bool,
    max_age: Duration,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CorsConfig {
    /// A configuration with the defaults and no allowed origin.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CorsConfig, CorsFilter};
    ///
    /// // Without an allowed origin the configuration is rejected.
    /// assert!(CorsFilter::new(CorsConfig::new()).is_err());
    /// ```
    pub fn new() -> Self {
        Self {
            origins: Vec::new(),
            methods: vec!["GET".into(), "HEAD".into(), "POST".into()],
            headers: Vec::new(),
            exposed: Vec::new(),
            credentials: false,
            max_age: Duration::from_secs(30 * 60),
        }
    }

    /// The origins that may read responses, each `scheme://host[:port]` with no path and no
    /// trailing slash, or the single entry `*` for every origin. `null` matches only the literal
    /// `Origin: null`. The host is compared without regard to case. Replaces the previous list.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CorsConfig, CorsFilter};
    ///
    /// let ok = CorsConfig::new().allowed_origins(["https://a.example", "http://localhost:8080"]);
    /// assert!(CorsFilter::new(ok).is_ok());
    /// let path = CorsConfig::new().allowed_origins(["https://a.example/"]);
    /// assert!(CorsFilter::new(path).is_err());
    /// ```
    pub fn allowed_origins(mut self, origins: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.origins = origins.into_iter().map(Into::into).collect();
        self
    }

    /// The methods a preflight may ask for, compared exactly (methods are case-sensitive), or
    /// the single entry `*` for every method. Replaces the default `GET, HEAD, POST`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CorsConfig, CorsFilter};
    ///
    /// let config = CorsConfig::new()
    ///     .allowed_origins(["https://a.example"])
    ///     .allowed_methods(["GET", "DELETE"]);
    /// assert!(CorsFilter::new(config).is_ok());
    /// ```
    pub fn allowed_methods(mut self, methods: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.methods = methods.into_iter().map(Into::into).collect();
        self
    }

    /// The request headers a preflight may ask for, compared without regard to case, or the
    /// single entry `*`, which allows and echoes every requested header. Default empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CorsConfig, CorsFilter};
    ///
    /// let config = CorsConfig::new()
    ///     .allowed_origins(["https://a.example"])
    ///     .allowed_headers(["Content-Type", "Authorization"]);
    /// assert!(CorsFilter::new(config).is_ok());
    /// ```
    pub fn allowed_headers(mut self, headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.headers = headers.into_iter().map(Into::into).collect();
        self
    }

    /// The response headers a browser lets the calling script read, sent as
    /// `Access-Control-Expose-Headers`. Default empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CorsConfig, CorsFilter};
    ///
    /// let config = CorsConfig::new()
    ///     .allowed_origins(["https://a.example"])
    ///     .exposed_headers(["X-Total"]);
    /// assert!(CorsFilter::new(config).is_ok());
    /// ```
    pub fn exposed_headers(mut self, headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.exposed = headers.into_iter().map(Into::into).collect();
        self
    }

    /// Whether browsers may send cookies and credentials, sent as
    /// `Access-Control-Allow-Credentials: true`. It cannot be combined with `*` in the origins,
    /// methods or headers. Default off.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CorsConfig, CorsFilter};
    ///
    /// let ok = CorsConfig::new().allowed_origins(["https://a.example"]).allow_credentials(true);
    /// assert!(CorsFilter::new(ok).is_ok());
    /// let star = CorsConfig::new().allowed_origins(["*"]).allow_credentials(true);
    /// assert!(CorsFilter::new(star).is_err());
    /// ```
    pub fn allow_credentials(mut self, on: bool) -> Self {
        self.credentials = on;
        self
    }

    /// How long a browser may cache a preflight answer, sent in whole seconds as
    /// `Access-Control-Max-Age`. Default 30 minutes.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_web::{CorsConfig, CorsFilter};
    ///
    /// let config = CorsConfig::new()
    ///     .allowed_origins(["https://a.example"])
    ///     .max_age(Duration::from_secs(60));
    /// assert!(CorsFilter::new(config).is_ok());
    /// ```
    pub fn max_age(mut self, max_age: Duration) -> Self {
        self.max_age = max_age;
        self
    }
}

/// A [`CorsConfig`] that cannot be used. The message names the setting (for example
/// `allowed-origins` or `allow-credentials`) and never contains a configured value.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{CorsConfig, CorsFilter};
///
/// let err = CorsFilter::new(CorsConfig::new().allowed_origins(["https://a.example/x"]))
///     .err()
///     .unwrap();
/// assert_eq!(err.setting(), "allowed-origins");
/// assert!(err.to_string().contains("allowed-origins"));
/// assert!(!err.to_string().contains("a.example"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorsError {
    setting: &'static str,
    reason: &'static str,
}

impl CorsError {
    fn new(setting: &'static str, reason: &'static str) -> Self {
        Self { setting, reason }
    }

    /// The name of the offending setting, as in the `server.cors.*` keys: `allowed-origins`,
    /// `allowed-methods`, `allowed-headers`, `exposed-headers` or `allow-credentials`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CorsConfig, CorsFilter};
    ///
    /// let config = CorsConfig::new().allowed_origins(["*"]).allow_credentials(true);
    /// let err = CorsFilter::new(config).err().unwrap();
    /// assert_eq!(err.setting(), "allow-credentials");
    /// ```
    pub fn setting(&self) -> &'static str {
        self.setting
    }

    /// Why the setting cannot be used, as fixed text that never contains a configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CorsConfig, CorsFilter};
    ///
    /// let err = CorsFilter::new(CorsConfig::new()).err().unwrap();
    /// assert_eq!(err.reason(), "at least one origin is required");
    /// ```
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for CorsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid CORS setting `{}`: {}",
            self.setting, self.reason
        )
    }
}

impl std::error::Error for CorsError {}

/// A list that is either the single entry `*` or explicit values.
#[derive(Debug)]
enum Allowed<T> {
    Any,
    List(Vec<T>),
}

/// Splits off a lone `*` entry. `*` next to other entries is an error for `setting`.
fn wildcard(values: &[String], setting: &'static str) -> Result<Option<()>, CorsError> {
    let stars = values.iter().filter(|v| v.trim() == "*").count();
    match (stars, values.len()) {
        (0, _) => Ok(None),
        (1, 1) => Ok(Some(())),
        _ => Err(CorsError::new(
            setting,
            "`*` must be the only entry of the list",
        )),
    }
}

/// Whether `b` may appear in an HTTP token (RFC 9110 `tchar`).
fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// The canonical form of an origin, `scheme://host[:port]` with scheme and host in lower case
/// and the port without leading zeros, or `None` when `value` is not such an origin. `null` is
/// returned as is.
fn normalize_origin(value: &str) -> Option<String> {
    if value == "null" {
        return Some(value.to_owned());
    }
    let (scheme, rest) = value.split_once("://")?;
    let mut sb = scheme.bytes();
    if !sb.next()?.is_ascii_alphabetic()
        || !sb.all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    {
        return None;
    }
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let (addr, after) = inner.split_once(']')?;
        if addr.is_empty()
            || !addr
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b":.".contains(&b))
        {
            return None;
        }
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':')?),
        };
        (&rest[..addr.len() + 2], port)
    } else {
        let (host, port) = match rest.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (rest, None),
        };
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
        {
            return None;
        }
        (host, port)
    };
    let port = match port {
        None => String::new(),
        Some(p) => {
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            format!(":{}", p.parse::<u16>().ok()?)
        }
    };
    Some(format!(
        "{}://{}{}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase(),
        port
    ))
}

/// Parses header names for `setting`, lower-cased, keeping their order.
fn header_names(values: &[String], setting: &'static str) -> Result<Vec<HeaderName>, CorsError> {
    values
        .iter()
        .map(|v| {
            HeaderName::from_bytes(v.trim().as_bytes())
                .map_err(|_| CorsError::new(setting, "an entry is not a valid header name"))
        })
        .collect()
}

/// Joins names into one `a, b` header value.
fn joined<'a>(names: impl Iterator<Item = &'a str>) -> Option<HeaderValue> {
    let text = names.collect::<Vec<_>>().join(", ");
    if text.is_empty() {
        return None;
    }
    HeaderValue::from_str(&text).ok()
}

/// Answers browser cross-origin requests at [`order::CORS`], outside error handling and
/// security.
///
/// - A request without an `Origin` header, or from an origin that is not allowed, passes and
///   gets no CORS headers; it is not blocked. When the allowed origins are a list (not `*`) it
///   gets `Vary: Origin`, so a shared cache never hands its copy to an allowed origin.
/// - A preflight (`OPTIONS` with `Origin` and `Access-Control-Request-Method`) from an allowed
///   origin that asks for allowed methods and headers is answered 204 here and never reaches
///   the rest of the chain. Any other preflight passes like a request from a disallowed origin.
/// - Every other request from an allowed origin gets `Access-Control-Allow-Origin`,
///   `Vary: Origin` and, when configured, `Access-Control-Expose-Headers` and
///   `Access-Control-Allow-Credentials`, also on error answers.
///
/// `Access-Control-Allow-Origin` echoes the request origin, or is `*` when the allowed origins
/// are `*` (which is only valid with credentials off). A request with more than one `Origin`
/// header is treated as not allowed.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{App, CorsConfig, HttpMethod, Request, Route};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = App::new()
///     .cors(CorsConfig::new().allowed_origins(["https://a.example"]))?
///     .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
///
/// let mut req = Request::new(HttpMethod::Get, "/ping");
/// req.headers.insert("origin", "https://a.example".parse()?);
/// let res = app.handle(req).await;
/// assert_eq!(res.headers()["access-control-allow-origin"], "https://a.example");
///
/// let mut req = Request::new(HttpMethod::Get, "/ping");
/// req.headers.insert("origin", "https://evil.example".parse()?);
/// let res = app.handle(req).await;
/// assert_eq!(res.status(), 200);
/// assert!(!res.headers().contains_key("access-control-allow-origin"));
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct CorsFilter {
    origins: Allowed<String>,
    methods: Allowed<String>,
    headers: Allowed<HeaderName>,
    allow_methods: Option<HeaderValue>,
    allow_headers: Option<HeaderValue>,
    expose_headers: Option<HeaderValue>,
    credentials: bool,
    max_age: HeaderValue,
}

impl CorsFilter {
    /// Checks `config` and builds the filter.
    ///
    /// Fails when an origin is not `scheme://host[:port]` or `null`, when the origin list is
    /// empty, when `*` is mixed with other entries, when `*` origins, methods or headers meet
    /// credentials, or when a method or header name is not a valid token.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CorsConfig, CorsFilter, Filter, order};
    ///
    /// let filter = CorsFilter::new(CorsConfig::new().allowed_origins(["https://a.example"]))?;
    /// assert_eq!(filter.order(), order::CORS);
    ///
    /// let mixed = CorsConfig::new().allowed_origins(["*", "https://a.example"]);
    /// assert_eq!(CorsFilter::new(mixed).err().unwrap().setting(), "allowed-origins");
    /// # Ok::<(), oxbloom_web::CorsError>(())
    /// ```
    pub fn new(config: CorsConfig) -> Result<Self, CorsError> {
        const ORIGINS: &str = "allowed-origins";
        const METHODS: &str = "allowed-methods";
        const HEADERS: &str = "allowed-headers";
        const EXPOSED: &str = "exposed-headers";
        const CREDENTIALS: &str = "allow-credentials";

        if config.origins.is_empty() {
            return Err(CorsError::new(ORIGINS, "at least one origin is required"));
        }
        let origins = match wildcard(&config.origins, ORIGINS)? {
            Some(()) if config.credentials => {
                return Err(CorsError::new(
                    CREDENTIALS,
                    "credentials cannot be allowed for every origin (`*` in allowed-origins)",
                ));
            }
            Some(()) => Allowed::Any,
            None => Allowed::List(
                config
                    .origins
                    .iter()
                    .map(|o| normalize_origin(o.trim()))
                    .collect::<Option<_>>()
                    .ok_or_else(|| {
                        CorsError::new(
                            ORIGINS,
                            "an origin must be `null` or scheme://host[:port] without a path or trailing slash",
                        )
                    })?,
            ),
        };

        let methods = match wildcard(&config.methods, METHODS)? {
            Some(()) if config.credentials => {
                return Err(CorsError::new(
                    CREDENTIALS,
                    "credentials cannot be combined with `*` in allowed-methods",
                ));
            }
            Some(()) => Allowed::Any,
            None => {
                let list: Vec<String> =
                    config.methods.iter().map(|m| m.trim().to_owned()).collect();
                if list
                    .iter()
                    .any(|m| m.is_empty() || !m.bytes().all(is_tchar))
                {
                    return Err(CorsError::new(
                        METHODS,
                        "an entry is not a valid method token",
                    ));
                }
                Allowed::List(list)
            }
        };

        let headers = match wildcard(&config.headers, HEADERS)? {
            Some(()) if config.credentials => {
                return Err(CorsError::new(
                    CREDENTIALS,
                    "credentials cannot be combined with `*` in allowed-headers",
                ));
            }
            Some(()) => Allowed::Any,
            None => Allowed::List(header_names(&config.headers, HEADERS)?),
        };
        let exposed = header_names(&config.exposed, EXPOSED)?;

        let allow_methods = match &methods {
            Allowed::Any => None,
            Allowed::List(list) => joined(list.iter().map(String::as_str)),
        };
        let allow_headers = match &headers {
            Allowed::Any => None,
            Allowed::List(list) => joined(list.iter().map(HeaderName::as_str)),
        };
        Ok(Self {
            origins,
            methods,
            headers,
            allow_methods,
            allow_headers,
            expose_headers: joined(exposed.iter().map(HeaderName::as_str)),
            credentials: config.credentials,
            max_age: HeaderValue::from(config.max_age.as_secs()),
        })
    }

    /// The value of `Access-Control-Allow-Origin` for the request `headers`, or `None` when
    /// the request has no single, allowed `Origin`.
    fn allow_origin(&self, headers: &HeaderMap) -> Option<HeaderValue> {
        let mut all = headers.get_all(header::ORIGIN).iter();
        let origin = all.next()?;
        if all.next().is_some() {
            return None;
        }
        let text = origin.to_str().ok()?;
        match &self.origins {
            Allowed::Any => Some(HeaderValue::from_static("*")),
            Allowed::List(list) => {
                let canonical = normalize_origin(text)?;
                list.contains(&canonical).then(|| origin.clone())
            }
        }
    }

    /// Runs the rest of the chain for a request that gets no CORS headers. With a list of
    /// origins the answer still depends on `Origin`, so it gets `Vary: Origin`.
    async fn pass(&self, req: Request, next: Next<'_>) -> Response {
        let mut res = next.run(req).await;
        if matches!(self.origins, Allowed::List(_)) {
            vary_origin(res.headers_mut());
        }
        res
    }

    /// The preflight answer, or `None` when the requested method or headers are not allowed.
    fn preflight(&self, headers: &HeaderMap, allow_origin: HeaderValue) -> Option<Response> {
        let mut methods = headers
            .get_all(header::ACCESS_CONTROL_REQUEST_METHOD)
            .iter();
        let method = methods.next()?.to_str().ok()?;
        if methods.next().is_some() || method.is_empty() || !method.bytes().all(is_tchar) {
            return None;
        }
        let allow_methods = match &self.methods {
            Allowed::Any => HeaderValue::from_str(method).ok()?,
            Allowed::List(list) => {
                if !list.iter().any(|m| m == method) {
                    return None;
                }
                self.allow_methods.clone()?
            }
        };

        let mut requested = Vec::new();
        for value in headers.get_all(header::ACCESS_CONTROL_REQUEST_HEADERS) {
            for name in value.to_str().ok()?.split(',') {
                let name = name.trim();
                if !name.is_empty() {
                    requested.push(HeaderName::from_bytes(name.as_bytes()).ok()?);
                }
            }
        }
        let allow_headers = match &self.headers {
            Allowed::Any => joined(requested.iter().map(HeaderName::as_str)),
            Allowed::List(list) => {
                if !requested.iter().all(|h| list.contains(h)) {
                    return None;
                }
                self.allow_headers.clone()
            }
        };

        let mut res = Response::new(Default::default());
        *res.status_mut() = StatusCode::NO_CONTENT;
        let out = res.headers_mut();
        out.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin);
        out.insert(header::ACCESS_CONTROL_ALLOW_METHODS, allow_methods);
        if let Some(h) = allow_headers {
            out.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, h);
        }
        out.insert(header::ACCESS_CONTROL_MAX_AGE, self.max_age.clone());
        if self.credentials {
            out.insert(
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }
        out.insert(
            header::VARY,
            HeaderValue::from_static(
                "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
            ),
        );
        Some(res)
    }
}

/// Appends `Origin` to the `Vary` headers unless they already list it or are `*`.
fn vary_origin(headers: &mut HeaderMap) {
    let covered = headers.get_all(header::VARY).iter().any(|v| {
        v.to_str().is_ok_and(|s| {
            s.split(',').any(|item| {
                let item = item.trim();
                item == "*" || item.eq_ignore_ascii_case("origin")
            })
        })
    });
    if !covered {
        headers.append(header::VARY, HeaderValue::from_static("Origin"));
    }
}

#[async_trait::async_trait]
impl Filter for CorsFilter {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        let Some(allow_origin) = self.allow_origin(&req.headers) else {
            return self.pass(req, next).await;
        };
        if req.method == HttpMethod::Options
            && req
                .headers
                .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
        {
            return match self.preflight(&req.headers, allow_origin) {
                Some(res) => res,
                None => self.pass(req, next).await,
            };
        }
        let mut res = next.run(req).await;
        let out = res.headers_mut();
        out.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, allow_origin);
        vary_origin(out);
        if let Some(h) = &self.expose_headers {
            out.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, h.clone());
        }
        if self.credentials {
            out.insert(
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }
        res
    }

    fn order(&self) -> i32 {
        order::CORS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_normalized_or_rejected() {
        let table: &[(&str, Option<&str>)] = &[
            ("https://a.example", Some("https://a.example")),
            ("HTTPS://A.Example", Some("https://a.example")),
            ("http://localhost:08080", Some("http://localhost:8080")),
            ("http://[::1]:3000", Some("http://[::1]:3000")),
            ("http://[::1]", Some("http://[::1]")),
            ("null", Some("null")),
            ("NULL", None),
            ("https://a.example/", None),
            ("https://a.example/x", None),
            ("https://a.example?x", None),
            ("https://a.example#x", None),
            ("https://user@a.example", None),
            ("https://a.example:", None),
            ("https://a.example:99999", None),
            ("https://a.example:1:2", None),
            ("https://", None),
            ("a.example", None),
            ("://a.example", None),
            ("1http://a.example", None),
            ("http://[::1", None),
            ("http://[]", None),
            ("http://[::1]x", None),
            ("https://a example", None),
            ("*", None),
        ];
        for (input, expected) in table {
            assert_eq!(
                normalize_origin(input).as_deref(),
                *expected,
                "origin {input:?}"
            );
        }
    }
}
