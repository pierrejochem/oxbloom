//! Response compression: gzip, brotli and zstd, chosen from `Accept-Encoding`.

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};

use crate::router::HeadBodyStripped;
use crate::{Filter, HttpError, HttpMethod, IntoResponse, Next, Request, Response, order};

/// How hard the encoders work. Each level maps to the scale of each codec: gzip 1, 6 and 9;
/// brotli 2, 5 and 11; zstd 1, 3 and 19.
///
/// # Examples
///
/// ```
/// use oxbloom_web::CompressionLevel;
///
/// assert_eq!(CompressionLevel::Default, CompressionLevel::Default);
/// assert_ne!(CompressionLevel::Fast, CompressionLevel::Best);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompressionLevel {
    /// The fastest setting with the weakest ratio.
    Fast,
    /// A balance of speed and ratio.
    Default,
    /// The strongest ratio, which is also the slowest.
    Best,
}

/// Settings of a [`CompressionFilter`].
///
/// The defaults are: bodies of at least 1024 bytes, at most 8 MiB, the types `text/*`,
/// `application/json`, `application/problem+json`, `application/javascript`, `image/svg+xml`
/// and `application/xml`, and [`CompressionLevel::Default`].
///
/// # Examples
///
/// ```
/// use oxbloom_web::{CompressionConfig, CompressionLevel};
///
/// let config = CompressionConfig::new()
///     .min_size(256)
///     .mime_types(["text/*", "application/wasm"])
///     .level(CompressionLevel::Fast)
///     .max_body_bytes(1 << 20);
/// assert!(format!("{config:?}").contains("256"));
/// ```
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    min_size: usize,
    mime_types: Vec<String>,
    level: CompressionLevel,
    max_body_bytes: usize,
}

impl CompressionConfig {
    /// The default settings.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::CompressionConfig;
    ///
    /// let config = CompressionConfig::new();
    /// assert!(format!("{config:?}").contains("1024"));
    /// ```
    pub fn new() -> Self {
        Self {
            min_size: 1024,
            mime_types: [
                "text/*",
                "application/json",
                "application/problem+json",
                "application/javascript",
                "image/svg+xml",
                "application/xml",
            ]
            .map(String::from)
            .to_vec(),
            level: CompressionLevel::Default,
            max_body_bytes: 8 * 1024 * 1024,
        }
    }

    /// Sets the smallest body, in bytes, that is compressed.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::CompressionConfig;
    ///
    /// let config = CompressionConfig::new().min_size(10);
    /// assert!(format!("{config:?}").contains("min_size: 10"));
    /// ```
    pub fn min_size(mut self, bytes: usize) -> Self {
        self.min_size = bytes;
        self
    }

    /// Replaces the list of media types that are compressed. An entry is `type/subtype` or
    /// `type/*`; parameters of the response `Content-Type` are ignored and case does not matter.
    /// An entry `*/*` matches nothing, only `type/*` and `type/subtype` entries match.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::CompressionConfig;
    ///
    /// let config = CompressionConfig::new().mime_types(["text/csv"]);
    /// assert!(format!("{config:?}").contains("text/csv"));
    /// ```
    pub fn mime_types(mut self, types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.mime_types = types.into_iter().map(Into::into).collect();
        self
    }

    /// Sets how hard the encoders work. Encoding runs synchronously on the worker that handles
    /// the request, so [`CompressionLevel::Best`] on a large body can take seconds; keep
    /// `max_body_bytes` modest when using it. `Best` costs memory too: brotli quality 11 with a
    /// 4 MiB window and zstd level 19 can use tens of MB per encode, and nothing limits how many
    /// encodes run at once.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CompressionConfig, CompressionLevel};
    ///
    /// let config = CompressionConfig::new().level(CompressionLevel::Best);
    /// assert!(format!("{config:?}").contains("Best"));
    /// ```
    pub fn level(mut self, level: CompressionLevel) -> Self {
        self.level = level;
        self
    }

    /// Sets the largest body, in bytes, that is compressed. The filter has no streaming mode,
    /// so a larger body passes unchanged, which bounds memory and CPU.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::CompressionConfig;
    ///
    /// let config = CompressionConfig::new().max_body_bytes(2048);
    /// assert!(format!("{config:?}").contains("max_body_bytes: 2048"));
    /// ```
    pub fn max_body_bytes(mut self, bytes: usize) -> Self {
        self.max_body_bytes = bytes;
        self
    }
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// The names of the codecs compiled into this build, best first: `"zstd"`, `"br"` and `"gzip"`
/// for the features `zstd`, `brotli` and `gzip`. A codec that is not listed is never chosen.
///
/// # Examples
///
/// ```
/// let names = oxbloom_web::codecs();
/// assert!(names.iter().all(|n| ["zstd", "br", "gzip"].contains(n)));
/// ```
pub fn codecs() -> &'static [&'static str] {
    &[
        #[cfg(feature = "zstd")]
        "zstd",
        #[cfg(feature = "brotli")]
        "br",
        #[cfg(feature = "gzip")]
        "gzip",
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    Zstd,
    Brotli,
    Gzip,
}

impl Codec {
    /// In order of preference when the client rates codecs equally.
    const PREFERENCE: [Codec; 3] = [Codec::Zstd, Codec::Brotli, Codec::Gzip];

    fn token(self) -> &'static str {
        match self {
            Codec::Zstd => "zstd",
            Codec::Brotli => "br",
            Codec::Gzip => "gzip",
        }
    }

    fn compiled(self) -> bool {
        codecs().contains(&self.token())
    }

    #[cfg_attr(
        not(any(feature = "gzip", feature = "brotli", feature = "zstd")),
        allow(unused_variables)
    )]
    fn encode(self, level: CompressionLevel, data: &[u8]) -> Option<Vec<u8>> {
        let pick = |fast: i32, default: i32, best: i32| match level {
            CompressionLevel::Fast => fast,
            CompressionLevel::Default => default,
            CompressionLevel::Best => best,
        };
        match self {
            #[cfg(feature = "gzip")]
            Codec::Gzip => {
                use std::io::Write;
                let mut enc = flate2::write::GzEncoder::new(
                    Vec::new(),
                    flate2::Compression::new(pick(1, 6, 9) as u32),
                );
                enc.write_all(data).ok()?;
                enc.finish().ok()
            }
            #[cfg(feature = "brotli")]
            Codec::Brotli => {
                use std::io::Write;
                let mut enc =
                    brotli::CompressorWriter::new(Vec::new(), 4096, pick(2, 5, 11) as u32, 22);
                enc.write_all(data).ok()?;
                Some(enc.into_inner())
            }
            #[cfg(feature = "zstd")]
            Codec::Zstd => zstd::stream::encode_all(data, pick(1, 3, 19)).ok(),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }
}

/// What the `Accept-Encoding` headers of a request ask for.
#[derive(Debug, PartialEq, Eq)]
struct Wanted {
    codec: Option<Codec>,
    /// `identity;q=0` was sent, so an answer without a content coding is not acceptable.
    identity_refused: bool,
}

/// Reads the `Accept-Encoding` headers. Names are case-insensitive, a malformed entry (no
/// token, or a `q` that is not a number in `0..=1`) is skipped, and when a name is listed more
/// than once its first entry counts. A codec that is not listed takes the `q` of `*`, and is not
/// acceptable when `*` is absent. The best `q` above zero wins, ties go to zstd, then brotli,
/// then gzip. `*;q=0` alone only rules out every codec; it does not refuse the identity.
fn wanted(headers: &HeaderMap) -> Wanted {
    let mut seen: Vec<(String, f32)> = Vec::new();
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else { continue };
        for entry in value.split(',') {
            let (name, params) = entry.split_once(';').unwrap_or((entry, ""));
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() || !name.bytes().all(is_token_byte) {
                continue;
            }
            let Some(q) = quality(params) else { continue };
            if !seen.iter().any(|(n, _)| *n == name) {
                seen.push((name, q));
            }
        }
    }
    let q_of = |name: &str| seen.iter().find(|(n, _)| n == name).map(|(_, q)| *q);
    let star = q_of("*");
    let mut best: Option<(Codec, f32)> = None;
    for codec in Codec::PREFERENCE {
        if !codec.compiled() {
            continue;
        }
        let q = q_of(codec.token()).or(star).unwrap_or(0.0);
        if q > 0.0 && best.is_none_or(|(_, b)| q > b) {
            best = Some((codec, q));
        }
    }
    Wanted {
        codec: best.map(|(c, _)| c),
        identity_refused: q_of("identity") == Some(0.0),
    }
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// The `q` in `params` (the text after the first `;`), `1` when absent, or `None` when it is
/// not a number in `0..=1`.
fn quality(params: &str) -> Option<f32> {
    let mut q = 1.0;
    for piece in params.split(';') {
        if let Some((name, value)) = piece.split_once('=')
            && name.trim().eq_ignore_ascii_case("q")
        {
            let value = value.trim();
            let digits = value.bytes().all(|b| b.is_ascii_digit() || b == b'.');
            let parsed: f32 = value.parse().ok().filter(|_| digits)?;
            if !(0.0..=1.0).contains(&parsed) {
                return None;
            }
            q = parsed;
        }
    }
    Some(q)
}

/// Whether the media type of `Content-Type` is in `types` (parameters and case are ignored).
fn type_matches(headers: &HeaderMap, types: &[String]) -> bool {
    let Some(value) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let media = value.split(';').next().unwrap_or("").trim();
    types.iter().any(|t| {
        let t = t.trim();
        match t.strip_suffix("/*") {
            Some(prefix) => media
                .split_once('/')
                .is_some_and(|(ty, _)| ty.eq_ignore_ascii_case(prefix)),
            None => media.eq_ignore_ascii_case(t),
        }
    })
}

fn has_directive(headers: &HeaderMap, directive: &str) -> bool {
    headers
        .get_all(header::CACHE_CONTROL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|d| d.trim().eq_ignore_ascii_case(directive))
}

fn add_vary(headers: &mut HeaderMap) {
    let covered = headers.get_all(header::VARY).iter().any(|v| {
        v.to_str().is_ok_and(|s| {
            s.split(',').any(|item| {
                let item = item.trim();
                item == "*" || item.eq_ignore_ascii_case("accept-encoding")
            })
        })
    });
    if !covered {
        headers.append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
}

fn weaken_etag(headers: &mut HeaderMap) {
    if let Some(etag) = headers.get(header::ETAG)
        && etag.as_bytes().starts_with(b"\"")
    {
        let mut weak = Vec::with_capacity(etag.len() + 2);
        weak.extend_from_slice(b"W/");
        weak.extend_from_slice(etag.as_bytes());
        if let Ok(value) = HeaderValue::from_bytes(&weak) {
            headers.insert(header::ETAG, value);
        }
    }
}

/// Compresses response bodies with gzip, brotli or zstd, as the client's `Accept-Encoding`
/// allows.
///
/// A response is compressed only when all of these hold: a compiled codec is acceptable, the
/// size of the body is within `min_size` and `max_body_bytes`, the `Content-Type` is in the
/// configured list, the response has no `Content-Encoding` or `Content-Range`, its status is not
/// 1xx, 204 or 304, and its `Cache-Control` has no `no-transform`. Otherwise it passes
/// unchanged. A compressed response gets `Content-Encoding`, loses `Content-Length` (the server
/// sets it), gets `Vary: Accept-Encoding` (unless it already lists it), and has a strong `ETag`
/// turned into a weak one.
///
/// A client that sends `identity;q=0` with no codec it can use gets a 406 for a response that
/// would otherwise have been compressed; any other response is untouched.
///
/// `HEAD` gets the headers of the matching `GET`, with an empty body. The router drops the body of
/// a `HEAD` answer before filters run but records its length, so the filter decides as it would
/// for `GET`. A bodiless `HEAD` answer that did not come from the router is sized by its
/// `Content-Length`, and passes unchanged without one.
///
/// Registered with [`App::compression`](crate::App::compression) at [`order::COMPRESSION`]. Do
/// not compress responses that mix secrets with input an attacker controls over TLS (BREACH).
///
/// # Examples
///
/// ```
/// use oxbloom_web::{CompressionConfig, CompressionFilter, Filter, order};
///
/// let filter = CompressionFilter::new(CompressionConfig::new());
/// assert_eq!(filter.order(), order::COMPRESSION);
/// ```
pub struct CompressionFilter {
    config: CompressionConfig,
}

impl CompressionFilter {
    /// Creates a filter with `config`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{CompressionConfig, CompressionFilter};
    ///
    /// let _filter = CompressionFilter::new(CompressionConfig::new().min_size(512));
    /// ```
    pub fn new(config: CompressionConfig) -> Self {
        Self { config }
    }

    /// The size of the body the client would get, when this response is one the filter would
    /// compress apart from the choice of codec. A `HEAD` answer that lost its body is sized by
    /// its `Content-Length`.
    fn candidate_size(&self, res: &Response, head: bool) -> Option<usize> {
        let status = res.status();
        if status.is_informational()
            || status == StatusCode::NO_CONTENT
            || status == StatusCode::NOT_MODIFIED
        {
            return None;
        }
        let headers = res.headers();
        if headers.contains_key(header::CONTENT_ENCODING)
            || headers.contains_key(header::CONTENT_RANGE)
            || has_directive(headers, "no-transform")
            || !type_matches(headers, &self.config.mime_types)
        {
            return None;
        }
        let len = if head && res.body().is_empty() {
            match res.extensions().get::<HeadBodyStripped>() {
                Some(stripped) => stripped.0,
                None => headers
                    .get(header::CONTENT_LENGTH)?
                    .to_str()
                    .ok()?
                    .trim()
                    .parse::<usize>()
                    .ok()?,
            }
        } else {
            res.body().len()
        };
        (len > 0 && len >= self.config.min_size && len <= self.config.max_body_bytes).then_some(len)
    }
}

#[async_trait::async_trait]
impl Filter for CompressionFilter {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        let head = req.method == HttpMethod::Head;
        let wanted = wanted(&req.headers);
        let mut res = next.run(req).await;
        if self.candidate_size(&res, head).is_none() {
            return res;
        }
        let Some(codec) = wanted.codec else {
            if wanted.identity_refused {
                return HttpError::new(StatusCode::NOT_ACCEPTABLE).into_response();
            }
            return res;
        };
        let body = std::mem::take(res.body_mut());
        if !body.is_empty() {
            match codec.encode(self.config.level, &body) {
                Some(out) if !head => *res.body_mut() = Bytes::from(out),
                Some(_) => {}
                None => {
                    *res.body_mut() = body;
                    return res;
                }
            }
        }
        let headers = res.headers_mut();
        headers.insert(
            header::CONTENT_ENCODING,
            HeaderValue::from_static(codec.token()),
        );
        headers.remove(header::CONTENT_LENGTH);
        add_vary(headers);
        weaken_etag(headers);
        res
    }

    fn order(&self) -> i32 {
        order::COMPRESSION
    }
}
