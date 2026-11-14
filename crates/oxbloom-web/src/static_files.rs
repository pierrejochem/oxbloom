//! Static resources: files from a directory or embedded in the binary, served as routes.
//!
//! The request path below the prefix is percent-decoded exactly once and cleaned before any
//! source sees it. [`DiskResources`] checks the path again and keeps every file under its
//! canonical root, so a symlink cannot lead out of it.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read};
use std::path::{Component, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use oxbloom_core::HttpMethod;

use crate::router::HeadBodyStripped;
use crate::{Controller, HttpError, IntoResponse, Request, Response, Route};

/// The longest raw path below the prefix that is looked at; longer paths answer 404.
const MAX_RAW_PATH_BYTES: usize = 3 * 1024;
/// The longest cleaned path that is looked up; longer paths answer 404.
const MAX_PATH_BYTES: usize = 1024;
/// The longest single path segment that is looked up; longer segments answer 404.
const MAX_SEGMENT_BYTES: usize = 255;
/// The default size cap of [`DiskResources`]: 10 MiB.
const DEFAULT_MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

/// One file as a [`ResourceSource`] returns it.
///
/// `etag` is a complete entity tag including its quotes, such as `"1a-2b"` (or `W/"1a-2b"` for
/// a weak one). `last_modified` is sent as `Last-Modified` when it is set.
///
/// # Examples
///
/// ```
/// use bytes::Bytes;
/// use oxbloom_web::Resource;
///
/// let resource = Resource {
///     bytes: Bytes::from_static(b"body {}"),
///     content_type: "text/css; charset=utf-8".into(),
///     etag: "\"v1\"".into(),
///     last_modified: None,
/// };
/// assert_eq!(resource.bytes.len(), 7);
/// ```
#[derive(Debug, Clone)]
pub struct Resource {
    /// The content of the file.
    pub bytes: Bytes,
    /// The value of the `Content-Type` header.
    pub content_type: String,
    /// The complete entity tag, including its quotes.
    pub etag: String,
    /// The time the file was last changed, when the source knows it.
    pub last_modified: Option<SystemTime>,
}

/// Why a [`ResourceSource`] could not return a file.
///
/// `NotFound` and `TooLarge` answer 404; `TooLarge` also logs a warning. `Io` answers 500 and
/// logs the error; the client never sees its text.
///
/// # Examples
///
/// ```
/// use oxbloom_web::ResourceError;
///
/// let err = ResourceError::Io(std::io::Error::other("disk failed"));
/// assert_eq!(err.to_string(), "static resource I/O error: disk failed");
/// assert!(matches!(ResourceError::NotFound, ResourceError::NotFound));
/// ```
#[derive(Debug)]
pub enum ResourceError {
    /// There is no file at the path, or it is not one that may be served.
    NotFound,
    /// The file is larger than the size cap of the source.
    TooLarge,
    /// Reading failed for another reason.
    Io(io::Error),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("static resource not found"),
            Self::TooLarge => f.write_str("static resource is larger than the size cap"),
            Self::Io(e) => write!(f, "static resource I/O error: {e}"),
        }
    }
}

impl std::error::Error for ResourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Where [`StaticResources`] reads files from.
///
/// `path` is already cleaned: segments joined by `/`, no leading slash, no `.` or `..`
/// segments, no segment that starts with `.`, and no backslash or NUL byte. It is empty for the
/// prefix itself. A source answers [`ResourceError::NotFound`] for a directory;
/// [`StaticResources`] then tries the index file below it.
///
/// # Examples
///
/// ```
/// use bytes::Bytes;
/// use oxbloom_web::{Resource, ResourceError, ResourceSource};
///
/// /// Serves one generated file.
/// struct Version;
///
/// #[async_trait::async_trait]
/// impl ResourceSource for Version {
///     async fn read(&self, path: &str) -> Result<Resource, ResourceError> {
///         if path != "version.txt" {
///             return Err(ResourceError::NotFound);
///         }
///         Ok(Resource {
///             bytes: Bytes::from_static(b"1.0.0"),
///             content_type: "text/plain; charset=utf-8".into(),
///             etag: "\"1.0.0\"".into(),
///             last_modified: None,
///         })
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// assert!(Version.read("version.txt").await.is_ok());
/// assert!(matches!(Version.read("other").await, Err(ResourceError::NotFound)));
/// # }
/// ```
#[async_trait::async_trait]
pub trait ResourceSource: Send + Sync {
    /// Reads the file at the cleaned relative `path`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{EmbeddedResources, ResourceSource};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let files = EmbeddedResources::new(&[("app.js", b"run()")]);
    /// let resource = files.read("app.js").await.unwrap();
    /// assert_eq!(resource.content_type, "text/javascript; charset=utf-8");
    /// # }
    /// ```
    async fn read(&self, path: &str) -> Result<Resource, ResourceError>;
}

/// Files below a directory on disk, read with `tokio::fs`.
///
/// The root is canonicalized once, when the value is built. Each file is canonicalized too and
/// must lie under the canonical root, so a symlink that leads out of the root, or to a name that
/// starts with `.`, is not found. Only regular files are served. A file larger than
/// [`max_file_bytes`](Self::max_file_bytes) (default 10 MiB) is [`ResourceError::TooLarge`]. A
/// root that does not exist makes every read an [`ResourceError::Io`] error, not a panic.
///
/// On Linux (including Android), Apple systems, FreeBSD, OpenBSD and NetBSD the file is opened
/// with `O_NOFOLLOW` and `O_NONBLOCK` after these checks, so a symlink or FIFO swapped in
/// between is neither followed nor waited on. Other Unix targets get no open flags and rely on
/// the checks before the open. Other changes made to the tree while a request is checked are
/// not detected: the root and everything below it must be writable only by people trusted to
/// choose what is served.
///
/// # Security
///
/// Rule matching in `oxbloom-security` compares raw request paths, which is why
/// [`StaticResources`] accepts only one canonical raw spelling of each file. On a
/// case-insensitive file system (the macOS default, Windows) `/ADMIN/x` still finds the file
/// `admin/x`, which a case-sensitive rule on `/admin/**` does not match. Such deployments should
/// use case-insensitive rules or keep the root on a case-sensitive file system.
/// `ETag` is built from the size and the modification time; `Last-Modified` is the
/// modification time.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{DiskResources, ResourceSource};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> std::io::Result<()> {
/// let dir = std::env::temp_dir().join(format!("oxbloom-doc-disk-{}", std::process::id()));
/// std::fs::create_dir_all(&dir)?;
/// std::fs::write(dir.join("hello.txt"), "hello")?;
///
/// let files = DiskResources::new(&dir).max_file_bytes(1024);
/// let resource = files.read("hello.txt").await.unwrap();
/// assert_eq!(resource.bytes, "hello");
/// assert!(resource.last_modified.is_some());
///
/// std::fs::remove_dir_all(&dir)?;
/// # Ok(())
/// # }
/// ```
pub struct DiskResources {
    root: Result<PathBuf, (io::ErrorKind, String)>,
    max_file_bytes: usize,
}

impl DiskResources {
    /// Serves the files below `root`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::DiskResources;
    ///
    /// // A missing root is not an error here; reading from it answers 500 later.
    /// let _files = DiskResources::new("/no/such/oxbloom/directory");
    /// ```
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = std::fs::canonicalize(&root)
            .map_err(|e| (e.kind(), format!("static root {}: {e}", root.display())));
        Self {
            root,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }

    /// Sets the largest file that is served, in bytes. Larger files answer 404 and log a
    /// warning.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::DiskResources;
    ///
    /// let _files = DiskResources::new("static").max_file_bytes(1024 * 1024);
    /// ```
    pub fn max_file_bytes(mut self, n: usize) -> Self {
        self.max_file_bytes = n;
        self
    }
}

/// Maps the errors that mean "there is nothing to serve here" to `NotFound`.
fn classify(e: io::Error) -> ResourceError {
    match e.kind() {
        io::ErrorKind::NotFound
        | io::ErrorKind::NotADirectory
        | io::ErrorKind::IsADirectory
        | io::ErrorKind::InvalidFilename => ResourceError::NotFound,
        _ => ResourceError::Io(e),
    }
}

#[async_trait::async_trait]
impl ResourceSource for DiskResources {
    async fn read(&self, path: &str) -> Result<Resource, ResourceError> {
        let root = match &self.root {
            Ok(root) => root,
            Err((kind, msg)) => return Err(ResourceError::Io(io::Error::new(*kind, msg.clone()))),
        };
        // Check again: this source may be used on its own, with a path that was not cleaned.
        let mut target = root.clone();
        for segment in path.split('/') {
            let mut parts = std::path::Path::new(segment).components();
            let single =
                matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none();
            if !is_clean_segment(segment) || !single {
                return Err(ResourceError::NotFound);
            }
            target.push(segment);
        }
        let canonical = tokio::fs::canonicalize(&target).await.map_err(classify)?;
        let inside = canonical
            .strip_prefix(root)
            .map_err(|_| ResourceError::NotFound)?;
        let hidden = inside.components().any(|c| match c {
            Component::Normal(name) => name.as_encoded_bytes().starts_with(b"."),
            _ => true,
        });
        if hidden {
            return Err(ResourceError::NotFound);
        }
        // Only regular files: opening a FIFO or a device could block or never end.
        let meta = tokio::fs::metadata(&canonical).await.map_err(classify)?;
        if !meta.is_file() {
            return Err(ResourceError::NotFound);
        }
        let max = self.max_file_bytes;
        let (bytes, meta) = tokio::task::spawn_blocking(move || read_capped(&canonical, max))
            .await
            .map_err(|e| ResourceError::Io(io::Error::other(e)))??;
        let modified = meta.modified().ok();
        let nanos = modified
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        Ok(Resource {
            etag: format!("\"{:x}-{:x}\"", bytes.len(), nanos),
            content_type: content_type_for(path).to_owned(),
            bytes: Bytes::from(bytes),
            last_modified: modified,
        })
    }
}

/// `O_NOFOLLOW | O_NONBLOCK` for the targets whose values are known; elsewhere no flags, and
/// only the checks before the open apply.
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    any(
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "riscv64",
        target_arch = "loongarch64",
        target_arch = "s390x"
    )
))]
const OPEN_FLAGS: i32 = 0o400_000 | 0o4_000;
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    any(
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    )
))]
const OPEN_FLAGS: i32 = 0o100_000 | 0o4_000;
#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
const OPEN_FLAGS: i32 = 0x0100 | 0x0004;
#[cfg(all(
    unix,
    not(any(
        all(
            any(target_os = "linux", target_os = "android"),
            any(
                target_arch = "x86",
                target_arch = "x86_64",
                target_arch = "riscv64",
                target_arch = "loongarch64",
                target_arch = "s390x",
                target_arch = "arm",
                target_arch = "aarch64",
                target_arch = "powerpc",
                target_arch = "powerpc64"
            )
        ),
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))
))]
const OPEN_FLAGS: i32 = 0;

/// Opens `path` for reading without following a symlink in its last component and without
/// blocking on a FIFO, so a file swapped after the checks cannot redirect or stall the read.
/// The `fstat` check of the caller then rejects anything that is not a regular file.
fn open_no_follow(path: &std::path::Path) -> Result<std::fs::File, ResourceError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(OPEN_FLAGS);
    }
    options.open(path).map_err(|e| {
        // `O_NOFOLLOW` on a symlink fails with `ELOOP`, whose error kind is not stable yet.
        let swapped = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        if swapped {
            ResourceError::NotFound
        } else {
            classify(e)
        }
    })
}

/// Reads at most `max` bytes of the regular file at `path`, with the metadata of the open file.
fn read_capped(
    path: &std::path::Path,
    max: usize,
) -> Result<(Vec<u8>, std::fs::Metadata), ResourceError> {
    let file = open_no_follow(path)?;
    let meta = file.metadata().map_err(ResourceError::Io)?;
    if !meta.is_file() {
        return Err(ResourceError::NotFound);
    }
    let cap = max as u64;
    if meta.len() > cap {
        return Err(ResourceError::TooLarge);
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // The file may grow after `metadata`; never read more than one byte past the cap.
    file.take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(classify)?;
    if bytes.len() > max {
        return Err(ResourceError::TooLarge);
    }
    Ok((bytes, meta))
}

/// Files compiled into the binary, given as a list of `(name, bytes)` pairs.
///
/// A name is a relative path such as `css/app.css`; a leading `/` is ignored. `ETag` is the
/// FNV-1a 64 hash of the bytes, so it is stable across builds and releases. There is no
/// `Last-Modified`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_web::{App, Controller, EmbeddedResources, HttpMethod, Request, StaticResources};
///
/// static FILES: &[(&str, &[u8])] = &[
///     ("index.html", b"<h1>hello</h1>"),
///     ("css/app.css", b"h1 { color: red }"),
/// ];
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), oxbloom_web::RouteError> {
/// let site = StaticResources::new("/", EmbeddedResources::new(FILES));
/// let app = App::new().routes(Controller::routes(Arc::new(site)))?;
///
/// let res = app.handle(Request::new(HttpMethod::Get, "/css/app.css")).await;
/// assert_eq!(res.status(), 200);
/// assert_eq!(res.headers()["content-type"], "text/css; charset=utf-8");
/// # Ok(())
/// # }
/// ```
pub struct EmbeddedResources {
    files: HashMap<&'static str, (&'static [u8], String)>,
}

impl EmbeddedResources {
    /// Serves `files`. A later entry with the same name replaces an earlier one.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::EmbeddedResources;
    ///
    /// let _files = EmbeddedResources::new(&[("robots.txt", b"User-agent: *\n")]);
    /// ```
    pub fn new(files: &'static [(&'static str, &'static [u8])]) -> Self {
        let files = files
            .iter()
            .map(|(name, bytes)| {
                let name = name.trim_start_matches('/');
                (name, (*bytes, format!("\"{:016x}\"", fnv1a_64(bytes))))
            })
            .collect();
        Self { files }
    }
}

#[async_trait::async_trait]
impl ResourceSource for EmbeddedResources {
    async fn read(&self, path: &str) -> Result<Resource, ResourceError> {
        let (bytes, etag) = self.files.get(path).ok_or(ResourceError::NotFound)?;
        Ok(Resource {
            bytes: Bytes::from_static(bytes),
            content_type: content_type_for(path).to_owned(),
            etag: etag.clone(),
            last_modified: None,
        })
    }
}

/// Serves the files of a [`ResourceSource`] below a URL prefix, as a [`Controller`].
///
/// It registers `GET` and `HEAD` for the prefix (with and without a trailing slash) and for
/// every path below it; other methods answer 405 from the router. Mounted at `/`, that
/// catch-all route makes a non-`GET` request to an unknown path a 405 instead of a 404.
///
/// The raw path below the prefix must be canonical, because security rules (in
/// `oxbloom-security`) match the raw request path, so every file has exactly one raw spelling:
///
/// - An empty segment (`//`) or a `.` segment, other than one trailing `/`, answers 404.
/// - A raw byte must be printable ASCII and not one of space, `"`, `#`, `<`, `>`, `?`, a
///   backtick, `{` or `}`. Non-ASCII characters and those must be escaped, as browsers do.
/// - An escape is allowed only for a non-ASCII byte or for one of the characters just listed
///   and `%`, and only with upper-case hex digits (`%C3%A9`, `%20`, `%7B`, `%25`). Any other
///   escape answers 404, so `%61`, `%2B`, `%5B` and `%c3%a9` do; `[`, `]`, `^` and `|` must be
///   written raw.
/// - After decoding each segment once: a `..` segment, a NUL byte, a backslash, an encoded `/`,
///   a segment that starts with `.`, a segment longer than 255 bytes or a path longer than 1024
///   bytes answers 404.
///
/// A directory serves its [`index`](Self::index) file (default `index.html`), or 404 without
/// one. An index file is the one exception to the single spelling: `/docs`, `/docs/` and
/// `/docs/index.html` all serve it. Protect a directory with a `/docs/**` rule, which also
/// matches `/docs`, not with a rule on the index file name or `/docs/*`.
///
/// Answers carry `Content-Type`, `ETag`, `Last-Modified` (when the source knows it) and
/// `Cache-Control` (`no-cache` for a [`cache_max_age`](Self::cache_max_age) of zero, the
/// default, and `public, max-age=N` otherwise). `If-None-Match` (weak comparison, lists, `*`)
/// and, only without it, `If-Modified-Since` give 304 with the same validators and no body. A
/// file over the size cap answers 404 and logs a warning on target `oxbloom::web`; any other
/// read error answers 500 and logs the error there.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// use oxbloom_web::{App, Controller, EmbeddedResources, HttpMethod, Request, StaticResources};
///
/// static FILES: &[(&str, &[u8])] = &[("index.html", b"<h1>hello</h1>")];
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), oxbloom_web::RouteError> {
/// let site = StaticResources::new("/site", EmbeddedResources::new(FILES))
///     .cache_max_age(Duration::from_secs(60));
/// let app = App::new().routes(Controller::routes(Arc::new(site)))?;
///
/// let res = app.handle(Request::new(HttpMethod::Get, "/site")).await;
/// assert_eq!(res.body(), "<h1>hello</h1>");
/// assert_eq!(res.headers()["cache-control"], "public, max-age=60");
///
/// let res = app.handle(Request::new(HttpMethod::Get, "/site/../secret")).await;
/// assert_eq!(res.status(), 404);
/// # Ok(())
/// # }
/// ```
pub struct StaticResources {
    prefix: String,
    source: Arc<dyn ResourceSource>,
    index: Option<String>,
    cache_max_age: Duration,
}

impl StaticResources {
    /// Serves `source` below `prefix`. A missing leading `/` is added and trailing ones are
    /// removed, so `assets/` and `/assets` are the same prefix; an empty prefix is `/`. Braces in
    /// the prefix are literal characters, not route parameters.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{DiskResources, StaticResources};
    ///
    /// let _assets = StaticResources::new("/assets", DiskResources::new("static"));
    /// ```
    pub fn new(prefix: impl Into<String>, source: impl ResourceSource + 'static) -> Self {
        let prefix = prefix.into();
        let trimmed = prefix.trim_matches('/');
        Self {
            prefix: format!("/{trimmed}"),
            source: Arc::new(source),
            index: Some("index.html".into()),
            cache_max_age: Duration::ZERO,
        }
    }

    /// Sets the file served for a directory. Default `index.html`.
    ///
    /// The name must be a single plain file name: not empty, not starting with `.`, and without
    /// `/`, `\`, `%` or NUL. Any other value turns the index off, so every directory answers
    /// 404; it is never passed to the source.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{EmbeddedResources, StaticResources};
    ///
    /// let _site = StaticResources::new("/", EmbeddedResources::new(&[])).index("start.html");
    /// ```
    pub fn index(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.index = (is_clean_segment(&name) && !name.contains('%')).then_some(name);
        self
    }

    /// Sets the `max-age` of `Cache-Control`, in whole seconds. Zero (the default) sends
    /// `no-cache`, so clients revalidate every time.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_web::{EmbeddedResources, StaticResources};
    ///
    /// let _site = StaticResources::new("/", EmbeddedResources::new(&[]))
    ///     .cache_max_age(Duration::from_secs(3600));
    /// ```
    pub fn cache_max_age(mut self, max_age: Duration) -> Self {
        self.cache_max_age = max_age;
        self
    }

    async fn lookup(&self, path: &str) -> (String, Result<Resource, ResourceError>) {
        if path.is_empty() {
            return match &self.index {
                Some(index) => (index.clone(), self.source.read(index).await),
                None => (String::new(), Err(ResourceError::NotFound)),
            };
        }
        match self.source.read(path).await {
            Err(ResourceError::NotFound) if self.index.is_some() => {
                let index = format!("{path}/{}", self.index.as_deref().unwrap_or_default());
                let found = self.source.read(&index).await;
                (index, found)
            }
            other => (path.to_owned(), other),
        }
    }

    async fn serve(&self, req: Request, head: bool) -> Response {
        let Some(path) = clean_path(req.param("rest").unwrap_or("")) else {
            return HttpError::new(StatusCode::NOT_FOUND).into_response();
        };
        let resource = match self.lookup(&path).await {
            (_, Ok(resource)) => resource,
            (_, Err(ResourceError::NotFound)) => {
                return HttpError::new(StatusCode::NOT_FOUND).into_response();
            }
            (path, Err(ResourceError::TooLarge)) => {
                tracing::warn!(
                    target: "oxbloom::web",
                    path = ?path,
                    "static resource is larger than the size cap and is not served"
                );
                return HttpError::new(StatusCode::NOT_FOUND).into_response();
            }
            (path, Err(ResourceError::Io(error))) => {
                tracing::error!(
                    target: "oxbloom::web",
                    path = ?path,
                    error = %error,
                    "static resource could not be read"
                );
                return HttpError::new(StatusCode::INTERNAL_SERVER_ERROR).into_response();
            }
        };

        let mut headers = HeaderMap::new();
        if let Ok(etag) = HeaderValue::from_str(&resource.etag) {
            headers.insert(header::ETAG, etag);
        }
        if let Some(date) = resource.last_modified.and_then(http_date) {
            headers.insert(
                header::LAST_MODIFIED,
                HeaderValue::from_str(&date).expect("an HTTP date is a valid header value"),
            );
        }
        headers.insert(header::CACHE_CONTROL, cache_control(self.cache_max_age));

        if not_modified(&req.headers, &resource) {
            let mut res = StatusCode::NOT_MODIFIED.into_response();
            *res.headers_mut() = headers;
            return res;
        }

        let content_type = HeaderValue::from_str(&resource.content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream"));
        headers.insert(header::CONTENT_TYPE, content_type);
        let mut res = Response::new(resource.bytes);
        *res.headers_mut() = headers;
        if head && !res.body().is_empty() {
            // Same as the router's own `HEAD` fallback, so later filters treat it like `GET`.
            let len = res.body().len();
            *res.body_mut() = Bytes::new();
            res.extensions_mut().insert(HeadBodyStripped(len));
        }
        res
    }
}

impl Controller for StaticResources {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        // The router reads `{` and `}` as parameter syntax; doubled, they are literal.
        let prefix = self.prefix.replace('{', "{{").replace('}', "}}");
        let mut patterns = vec![prefix.clone()];
        let base = if prefix == "/" {
            ""
        } else {
            patterns.push(format!("{prefix}/"));
            prefix.as_str()
        };
        patterns.push(format!("{base}/{{*rest}}"));

        let mut routes = Vec::with_capacity(patterns.len() * 2);
        for pattern in patterns {
            for (method, head) in [(HttpMethod::Get, false), (HttpMethod::Head, true)] {
                let this = Arc::clone(&self);
                routes.push(Route::new(method, pattern.clone(), move |req| {
                    let this = Arc::clone(&this);
                    async move { this.serve(req, head).await }
                }));
            }
        }
        routes
    }
}

/// Whether `segment` may name a file: not empty, not starting with `.` (so also not `.` or
/// `..`), no `/`, `\` or NUL, and not longer than [`MAX_SEGMENT_BYTES`].
fn is_clean_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= MAX_SEGMENT_BYTES
        && !segment.starts_with('.')
        && !segment.contains(['/', '\\', '\0'])
}

/// Checks and decodes the raw path below the prefix. `None` means 404.
///
/// The raw path must be canonical, because the security chain matches the raw request path and
/// this handler must not serve a different file than the one the chain judged: an empty or `.`
/// segment (other than one trailing `/`) gives `None`, and [`percent_decode`] allows each byte
/// in only one spelling, raw or escaped. The raw path is split on `/` first and each segment is decoded once, so an encoded `/` stays
/// inside its segment (and is rejected there) and `%252e` becomes the literal `%2e`.
fn clean_path(raw: &str) -> Option<String> {
    if raw.len() > MAX_RAW_PATH_BYTES {
        return None;
    }
    if raw.is_empty() {
        return Some(String::new());
    }
    let mut out = String::new();
    for raw_segment in raw.strip_suffix('/').unwrap_or(raw).split('/') {
        if raw_segment.is_empty() || raw_segment == "." {
            return None;
        }
        let segment = percent_decode(raw_segment)?;
        if !is_clean_segment(&segment) {
            return None;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&segment);
    }
    (out.len() <= MAX_PATH_BYTES).then_some(out)
}

/// Decodes `%XX` escapes once, insisting on the one canonical raw spelling: each byte has
/// either a raw form ([`raw_allowed`]) or an escaped form ([`escape_allowed`]), never both. A
/// raw byte that must be escaped, an escape of a byte that must be raw, a malformed escape, an
/// escape with a lower-case hex digit or a result that is not UTF-8 gives `None`.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = upper_hex_digit(hex[0])?;
            let lo = upper_hex_digit(hex[1])?;
            let byte = hi * 16 + lo;
            if !escape_allowed(byte) {
                return None;
            }
            out.push(byte);
            i += 3;
        } else {
            if !raw_allowed(bytes[i]) {
                return None;
            }
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The value of an upper-case hex digit (`0-9`, `A-F`); `None` for anything else.
fn upper_hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The printable ASCII characters that browsers escape in a path, so they are allowed only as
/// escapes, together with `%` itself.
const ESCAPE_ONLY: &[u8] = b" \"#<>?`{}%";

/// Whether `byte` may appear unescaped: printable ASCII that is not in [`ESCAPE_ONLY`].
fn raw_allowed(byte: u8) -> bool {
    byte.is_ascii_graphic() && !ESCAPE_ONLY.contains(&byte)
}

/// Whether `byte` may appear as an escape: a non-ASCII byte (part of a UTF-8 sequence) or a
/// character of [`ESCAPE_ONLY`].
fn escape_allowed(byte: u8) -> bool {
    !byte.is_ascii() || ESCAPE_ONLY.contains(&byte)
}

/// The content type for the extension of `path`, from a built-in table.
fn content_type_for(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some((_, ext)) = name.rsplit_once('.') else {
        return "application/octet-stream";
    };
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "xml" => "application/xml",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// The 64-bit FNV-1a hash. Written here because `DefaultHasher` may change between releases.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn cache_control(max_age: Duration) -> HeaderValue {
    match max_age.as_secs() {
        0 => HeaderValue::from_static("no-cache"),
        secs => HeaderValue::from_str(&format!("public, max-age={secs}"))
            .expect("a number is a valid header value"),
    }
}

/// RFC 9110 rules for `GET` and `HEAD`: `If-None-Match` decides when it is present, otherwise
/// `If-Modified-Since` does. A date that cannot be parsed is ignored.
fn not_modified(headers: &HeaderMap, resource: &Resource) -> bool {
    let mut lists = headers.get_all(header::IF_NONE_MATCH).iter().peekable();
    if lists.peek().is_some() {
        return lists.any(|list| {
            list.to_str()
                .is_ok_and(|list| etag_list_matches(list, &resource.etag))
        });
    }
    let since = headers
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_http_date);
    match (since, resource.last_modified) {
        (Some(since), Some(modified)) => modified
            .duration_since(UNIX_EPOCH)
            .is_ok_and(|d| d.as_secs() <= since),
        _ => false,
    }
}

/// Weak comparison of `etag` with an `If-None-Match` list: `*`, or any entity tag in the list
/// with the same opaque tag, ignoring `W/` on either side. Parsing stops at the first malformed
/// entry.
fn etag_list_matches(list: &str, etag: &str) -> bool {
    let ours = etag.strip_prefix("W/").unwrap_or(etag);
    if !(ours.len() >= 2 && ours.starts_with('"') && ours.ends_with('"')) {
        return false;
    }
    let mut rest = list.trim();
    if rest == "*" {
        return true;
    }
    loop {
        rest = rest.trim_start_matches([' ', '\t', ',']);
        if rest.is_empty() {
            return false;
        }
        let tagged = rest.strip_prefix("W/").unwrap_or(rest);
        if !tagged.starts_with('"') {
            return false;
        }
        let Some(end) = tagged[1..].find('"') else {
            return false;
        };
        let theirs = &tagged[..end + 2];
        if theirs == ours {
            return true;
        }
        rest = &tagged[end + 2..];
    }
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Formats `time` as an IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`). `None` before 1970 or
/// after 9999.
fn http_date(time: SystemTime) -> Option<String> {
    let secs = time.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let days = i64::try_from(secs / 86_400).ok()?;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    if year > 9999 {
        return None;
    }
    let weekday = WEEKDAYS[((days + 4) % 7) as usize];
    Some(format!(
        "{weekday}, {day:02} {} {year:04} {:02}:{:02}:{:02} GMT",
        MONTHS[(month - 1) as usize],
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    ))
}

/// Parses an IMF-fixdate into seconds since the Unix epoch. Other formats give `None`.
fn parse_http_date(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() != 29 || &b[3..5] != b", " || &b[25..] != b" GMT" {
        return None;
    }
    if !WEEKDAYS.iter().any(|d| d.as_bytes() == &b[..3]) {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = &s[range];
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let sep = |i: usize, c: u8| b[i] == c;
    if !(sep(7, b' ') && sep(11, b' ') && sep(16, b' ') && sep(19, b':') && sep(22, b':')) {
        return None;
    }
    let day = num(5..7)?;
    let month = MONTHS.iter().position(|m| m.as_bytes() == &b[8..11])? as i64 + 1;
    let year = num(12..16)?;
    let (hour, minute, second) = (num(17..19)?, num(20..22)?, num(23..25)?);
    if year < 1970 || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second).ok()
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_path_decodes_once_and_rejects_unsafe_segments() {
        assert_eq!(clean_path("").as_deref(), Some(""));
        assert_eq!(clean_path("a/b/").as_deref(), Some("a/b"));
        assert_eq!(clean_path("a%20b%2F").as_deref(), None);
        assert_eq!(clean_path("%25").as_deref(), Some("%"));
        assert_eq!(clean_path("%C3%A9").as_deref(), Some("é"));
        assert_eq!(clean_path("a%20b").as_deref(), Some("a b"));
        assert_eq!(clean_path("%252e%252e").as_deref(), Some("%2e%2e"));
        assert_eq!(clean_path("my%20file").as_deref(), Some("my file"));
        assert_eq!(clean_path("a[b]^c|d").as_deref(), Some("a[b]^c|d"));
        assert_eq!(
            clean_path("%7B%7D%22%23%3C%3E%3F%60").as_deref(),
            Some("{}\"#<>?`")
        );
        for bad in [
            "é", "a{", "a}", "a\"", "a b", "a<", "a>", "a`", "a\t", "%5B", "%5D", "%5E", "%7C",
            "%7b", "%0A", "%7F", "..", "%2e%2e", "a%2fb", "a%5cb", "a\\b", "a%00", ".git", "%zz",
            "%e9", "%2", "a//b", "/a", "./a", "a/.", "a/./b", "a//", "/", "%61", "%7E", "%2d",
            "%5F", "%39", "%2B", "%2b", "%40", "%3A", "%3B", "%3D", "%21", "%24", "%26", "%27",
            "%28", "%29", "%2A", "%2C", "%c3%a9", "%C3%a9", "%c3%A9", "a%2F", "%5C", "%00",
        ] {
            assert_eq!(clean_path(bad), None, "{bad}");
        }
    }

    /// A fresh directory below the system temporary directory, removed by the caller.
    #[cfg(unix)]
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxbloom-unit-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn read_capped_does_not_follow_a_symlink_swapped_in_after_canonicalize() {
        let dir = scratch("nofollow");
        std::fs::write(dir.join("target"), "outside").unwrap();
        std::os::unix::fs::symlink(dir.join("target"), dir.join("link")).unwrap();
        let result = read_capped(&dir.join("link"), 1024);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(result.is_err(), "the symlink was followed");
    }

    #[cfg(unix)]
    #[test]
    fn read_capped_does_not_block_on_a_fifo_swapped_in_after_the_check() {
        let dir = scratch("fifo");
        let fifo = dir.join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success());
        if made {
            let (tx, rx) = std::sync::mpsc::channel();
            let path = fifo.clone();
            std::thread::spawn(move || {
                let _ = tx.send(matches!(
                    read_capped(&path, 1024),
                    Err(ResourceError::NotFound)
                ));
            });
            let answer = rx.recv_timeout(Duration::from_secs(5));
            if answer.is_err() {
                // Unblock the reader so the thread ends, then fail.
                let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
            }
            std::fs::remove_dir_all(&dir).unwrap();
            assert_eq!(answer, Ok(true), "opening a FIFO blocked or was served");
        } else {
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn http_dates_round_trip() {
        let t = UNIX_EPOCH + Duration::from_secs(784_111_777);
        let s = http_date(t).unwrap();
        assert_eq!(s, "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(parse_http_date(&s), Some(784_111_777));
        assert_eq!(
            http_date(UNIX_EPOCH).unwrap(),
            "Thu, 01 Jan 1970 00:00:00 GMT"
        );
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        for secs in [
            0_u64,
            951_782_400,
            1_709_251_199,
            4_102_444_800,
            253_402_300_799,
        ] {
            let s = http_date(UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
            assert_eq!(parse_http_date(&s), Some(secs), "{s}");
        }
        assert_eq!(parse_http_date("Sun, 29 Feb 2023 00:00:00 GMT"), None);
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 24:00:00 GMT"), None);
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), None);
    }

    #[test]
    fn etag_lists_use_weak_comparison() {
        assert!(etag_list_matches("\"a\"", "\"a\""));
        assert!(etag_list_matches("W/\"a\"", "\"a\""));
        assert!(etag_list_matches("\"a\"", "W/\"a\""));
        assert!(etag_list_matches("\"x,y\", \"a\"", "\"a\""));
        assert!(etag_list_matches("*", "\"a\""));
        assert!(!etag_list_matches("\"b\"", "\"a\""));
        assert!(!etag_list_matches("a", "\"a\""));
        assert!(!etag_list_matches("\"a", "\"a\""));
        assert!(!etag_list_matches("", "\"a\""));
    }

    #[test]
    fn fnv1a_64_matches_the_reference_values() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
