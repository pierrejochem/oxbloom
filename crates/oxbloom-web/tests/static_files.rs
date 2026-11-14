//! Static resources: content types, index files, the path traversal matrix, validators,
//! conditional requests, `HEAD`, the size cap, I/O errors, embedded files and mounting.

use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use http::{HeaderValue, StatusCode};
use oxbloom_web::{
    App, Controller, DiskResources, EmbeddedResources, HttpMethod, Request, Resource,
    ResourceError, ResourceSource, Response, StaticResources,
};
use tracing_subscriber::fmt::MakeWriter;

const SECRET: &str = "TOP SECRET CONTENT";

/// A temporary directory that is removed when the value is dropped.
///
/// The layout is `<dir>/secret` (outside the root) and `<dir>/root` (the served root).
struct Site {
    dir: PathBuf,
}

impl Site {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("oxbloom-static-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(dir.join("root")).unwrap();
        let site = Self { dir };
        std::fs::write(site.dir.join("secret"), SECRET).unwrap();
        site.file("index.html", "<h1>home</h1>");
        site.file("app.css", "body { color: red }");
        site.file("docs/index.html", "<h1>docs</h1>");
        site.file("a/b/keep.txt", "keep");
        site.file("empty/nothing.txt", "x");
        std::fs::remove_file(site.root().join("empty/nothing.txt")).unwrap();
        site.file(".hidden", "hidden");
        site.file("dir/.git/config", "git config");
        site
    }

    fn root(&self) -> PathBuf {
        self.dir.join("root")
    }

    fn file(&self, rel: &str, content: &str) {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn app(&self) -> App {
        mount(StaticResources::new("/", DiskResources::new(self.root())))
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn mount(resources: StaticResources) -> App {
    App::new()
        .routes(Controller::routes(Arc::new(resources)))
        .unwrap()
}

fn get(path: &str) -> Request {
    Request::new(HttpMethod::Get, path)
}

fn with(mut req: Request, name: &'static str, value: &str) -> Request {
    req.headers
        .append(name, HeaderValue::from_str(value).unwrap());
    req
}

fn header<'a>(res: &'a Response, name: &str) -> Option<&'a str> {
    res.headers().get(name).map(|v| v.to_str().unwrap())
}

fn body(res: &Response) -> String {
    String::from_utf8_lossy(res.body()).into_owned()
}

mod content_types {
    use super::*;

    const FILES: &[(&str, &[u8])] = &[
        ("a.html", b"x"),
        ("a.css", b"x"),
        ("a.js", b"x"),
        ("a.mjs", b"x"),
        ("a.json", b"x"),
        ("a.xml", b"x"),
        ("a.txt", b"x"),
        ("a.svg", b"x"),
        ("a.png", b"x"),
        ("a.jpg", b"x"),
        ("a.jpeg", b"x"),
        ("a.gif", b"x"),
        ("a.webp", b"x"),
        ("a.avif", b"x"),
        ("a.ico", b"x"),
        ("a.woff", b"x"),
        ("a.woff2", b"x"),
        ("a.ttf", b"x"),
        ("a.wasm", b"x"),
        ("a.map", b"x"),
        ("a.pdf", b"x"),
        ("a.csv", b"x"),
        ("a.unknown", b"x"),
        ("noext", b"x"),
        ("UPPER.CSS", b"x"),
    ];

    async fn type_of(name: &str) -> String {
        let app = mount(StaticResources::new("/", EmbeddedResources::new(FILES)));
        let res = app.handle(get(&format!("/{name}"))).await;
        assert_eq!(res.status(), 200, "{name}");
        header(&res, "content-type").unwrap().to_owned()
    }

    #[tokio::test]
    async fn every_extension_in_the_table_has_its_content_type() {
        let expected = [
            ("a.html", "text/html; charset=utf-8"),
            ("a.css", "text/css; charset=utf-8"),
            ("a.js", "text/javascript; charset=utf-8"),
            ("a.mjs", "text/javascript; charset=utf-8"),
            ("a.json", "application/json"),
            ("a.xml", "application/xml"),
            ("a.txt", "text/plain; charset=utf-8"),
            ("a.svg", "image/svg+xml"),
            ("a.png", "image/png"),
            ("a.jpg", "image/jpeg"),
            ("a.jpeg", "image/jpeg"),
            ("a.gif", "image/gif"),
            ("a.webp", "image/webp"),
            ("a.avif", "image/avif"),
            ("a.ico", "image/x-icon"),
            ("a.woff", "font/woff"),
            ("a.woff2", "font/woff2"),
            ("a.ttf", "font/ttf"),
            ("a.wasm", "application/wasm"),
            ("a.map", "application/json"),
            ("a.pdf", "application/pdf"),
            ("a.csv", "text/csv; charset=utf-8"),
        ];
        for (name, content_type) in expected {
            assert_eq!(type_of(name).await, content_type, "{name}");
        }
    }

    #[tokio::test]
    async fn an_unknown_or_missing_extension_is_octet_stream() {
        assert_eq!(type_of("a.unknown").await, "application/octet-stream");
        assert_eq!(type_of("noext").await, "application/octet-stream");
    }

    #[tokio::test]
    async fn the_extension_is_matched_without_regard_to_case() {
        assert_eq!(type_of("UPPER.CSS").await, "text/css; charset=utf-8");
    }

    #[tokio::test]
    async fn text_types_carry_a_utf8_charset() {
        for (name, _) in FILES {
            let ct = type_of(name).await;
            if ct.starts_with("text/") {
                assert!(ct.ends_with("; charset=utf-8"), "{name}: {ct}");
            }
        }
    }
}

mod index {
    use super::*;

    #[tokio::test]
    async fn index_is_served_for_the_prefix_and_for_directories() {
        let site = Site::new();
        let app = site.app();
        for path in ["/", "/index.html"] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 200, "{path}");
            assert_eq!(body(&res), "<h1>home</h1>", "{path}");
            assert_eq!(
                header(&res, "content-type"),
                Some("text/html; charset=utf-8")
            );
        }
        for path in ["/docs", "/docs/", "/docs/index.html"] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 200, "{path}");
            assert_eq!(body(&res), "<h1>docs</h1>", "{path}");
        }
    }

    #[tokio::test]
    async fn an_index_file_is_served_at_its_own_path_and_at_its_directory() {
        // The one exception to "one raw spelling per file": protect a directory with
        // `/dir/**`, which also matches `/dir`, not with a rule on the index file name.
        let site = Site::new();
        let app = site.app();
        for path in ["/docs", "/docs/", "/docs/index.html"] {
            assert_eq!(
                body(&app.handle(get(path)).await),
                "<h1>docs</h1>",
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn a_custom_index_name_is_used() {
        let site = Site::new();
        site.file("start.txt", "start");
        let app =
            mount(StaticResources::new("/", DiskResources::new(site.root())).index("start.txt"));
        let res = app.handle(get("/")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(&res), "start");
    }

    #[tokio::test]
    async fn an_invalid_index_name_is_never_read_and_directories_answer_404() {
        for index in ["", "../secret", ".env", "a/b", "%41.html", "a\\b", "x\0"] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let app = mount(
                StaticResources::new("/", super::traversal::Recorder(seen.clone())).index(index),
            );
            assert_eq!(app.handle(get("/")).await.status(), 404, "{index:?}");
            assert_eq!(app.handle(get("/docs/")).await.status(), 404, "{index:?}");
            let seen = seen.lock().unwrap();
            assert_eq!(*seen, vec!["docs".to_owned()], "{index:?}");
        }
    }

    #[tokio::test]
    async fn directory_without_index_is_404() {
        let site = Site::new();
        let app = site.app();
        for path in ["/empty", "/empty/", "/a", "/a/b/"] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 404, "{path}");
        }
    }

    #[tokio::test]
    async fn a_missing_file_is_404() {
        let site = Site::new();
        let res = site.app().handle(get("/nope.css")).await;
        assert_eq!(res.status(), 404);
        assert_eq!(
            header(&res, "content-type"),
            Some("application/problem+json")
        );
    }

    #[tokio::test]
    async fn a_non_canonical_path_with_empty_or_dot_segments_is_404() {
        let site = Site::new();
        let app = site.app();
        for path in [
            "/./app.css",
            "//app.css",
            "/a/./b//keep.txt",
            "/%2e/app.css",
            "/a/b/keep.txt/.",
            "/docs//",
            "/a//",
        ] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 404, "{path}");
        }
    }

    #[tokio::test]
    async fn one_trailing_slash_is_allowed() {
        let site = Site::new();
        let app = site.app();
        for path in ["/docs/", "/a/b/keep.txt/"] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 200, "{path}");
        }
    }

    #[tokio::test]
    async fn an_escape_of_an_unreserved_character_is_404_in_either_case() {
        let site = Site::new();
        let app = site.app();
        for path in [
            "/%61pp.css",
            "/%41pp.css",
            "/app%2ecss",
            "/app%2Ecss",
            "/%7e",
            "/%2D",
            "/%5f",
            "/%30",
        ] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 404, "{path}");
        }
        // The plain form is served.
        assert_eq!(app.handle(get("/app.css")).await.status(), 200);
    }

    #[tokio::test]
    async fn a_percent_encoded_name_is_decoded_once() {
        let site = Site::new();
        site.file("my file.txt", "spaced");
        site.file("%41.txt", "literal percent");
        let app = site.app();
        let res = app.handle(get("/my%20file.txt")).await;
        assert_eq!(body(&res), "spaced");
        let res = app.handle(get("/%2541.txt")).await;
        assert_eq!(body(&res), "literal percent");
        let res = app.handle(get("/%41.txt")).await;
        assert_eq!(res.status(), 404);
    }
}

mod traversal {
    use super::*;

    async fn assert_hidden(app: &App, path: &str) {
        let res = app.handle(get(path)).await;
        assert_eq!(res.status(), 404, "{path}");
        assert!(!body(&res).contains(SECRET), "{path}");
        let res = app.handle(Request::new(HttpMethod::Head, path)).await;
        assert_eq!(res.status(), 404, "HEAD {path}");
    }

    const ATTEMPTS: &[&str] = &[
        "/../secret",
        "/%2e%2e/secret",
        "/%2E%2E/secret",
        "/a/%2e%2e/%2e%2e/secret",
        "/a/b/../../../secret",
        "/%2fetc%2fpasswd",
        "/..%2fsecret",
        "/a%5c..%5csecret",
        "/a\\..\\secret",
        "/x%00.html",
        "/index.html%00",
        "/%252e%252e/secret",
        "/%zz",
        "/%e9.txt",
        "/.hidden",
        "/%2ehidden",
        "/dir/.git/config",
    ];

    #[tokio::test]
    async fn every_traversal_attempt_answers_404_and_never_the_outside_file() {
        let site = Site::new();
        let app = site.app();
        for path in ATTEMPTS {
            assert_hidden(&app, path).await;
        }
    }

    /// Records every path it is asked for and finds nothing.
    pub(super) struct Recorder(pub(super) Arc<Mutex<Vec<String>>>);

    #[async_trait::async_trait]
    impl ResourceSource for Recorder {
        async fn read(&self, path: &str) -> Result<Resource, ResourceError> {
            self.0.lock().unwrap().push(path.to_owned());
            Err(ResourceError::NotFound)
        }
    }

    #[tokio::test]
    async fn a_source_only_ever_sees_clean_paths() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let app = mount(StaticResources::new("/", Recorder(seen.clone())));
        for path in ATTEMPTS {
            assert_eq!(app.handle(get(path)).await.status(), 404, "{path}");
        }
        let seen = seen.lock().unwrap();
        // Only the double-encoded attempt decodes to a harmless literal name.
        assert_eq!(
            *seen,
            vec![
                "%2e%2e/secret".to_owned(),
                "%2e%2e/secret/index.html".to_owned()
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_that_leaves_the_root_is_404() {
        let site = Site::new();
        std::os::unix::fs::symlink(site.dir.join("secret"), site.root().join("link")).unwrap();
        std::os::unix::fs::symlink(&site.dir, site.root().join("up")).unwrap();
        let app = site.app();
        assert_hidden(&app, "/link").await;
        assert_hidden(&app, "/up/secret").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_that_stays_inside_the_root_is_served() {
        let site = Site::new();
        std::os::unix::fs::symlink(site.root().join("app.css"), site.root().join("alias.css"))
            .unwrap();
        let res = site.app().handle(get("/alias.css")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(&res), "body { color: red }");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_to_a_dotfile_inside_the_root_is_404() {
        let site = Site::new();
        std::os::unix::fs::symlink(site.root().join(".hidden"), site.root().join("visible"))
            .unwrap();
        assert_hidden(&site.app(), "/visible").await;
    }

    #[tokio::test]
    async fn dotfiles_and_dot_directories_are_404() {
        let site = Site::new();
        let app = site.app();
        for path in ["/.hidden", "/dir/.git/config", "/%2ehidden", "/dir/.git/"] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 404, "{path}");
        }
    }

    #[tokio::test]
    async fn a_very_long_path_is_404_without_a_panic() {
        let site = Site::new();
        let app = site.app();
        for path in [
            format!("/{}", "a".repeat(4096)),
            format!("/{}", "a/".repeat(2048)),
            format!("/{}", "%2e%2e/".repeat(700)),
        ] {
            let res = app.handle(get(&path)).await;
            assert_eq!(res.status(), 404);
        }
    }

    #[tokio::test]
    async fn a_root_that_does_not_exist_answers_500_and_does_not_panic() {
        let site = Site::new();
        let app = mount(StaticResources::new(
            "/",
            DiskResources::new(site.dir.join("missing")),
        ));
        let res = app.handle(get("/app.css")).await;
        assert_eq!(res.status(), 500);
    }

    #[tokio::test]
    async fn disk_resources_rejects_unclean_paths_given_directly() {
        let site = Site::new();
        let disk = DiskResources::new(site.root());
        for path in [
            "../secret",
            "/etc/passwd",
            ".hidden",
            "a/../../secret",
            "a\\b",
        ] {
            assert!(
                matches!(disk.read(path).await, Err(ResourceError::NotFound)),
                "{path}"
            );
        }
    }
}

mod validators {
    use super::*;

    fn etag(res: &Response) -> String {
        header(res, "etag").unwrap().to_owned()
    }

    #[tokio::test]
    async fn disk_etag_is_strong_and_changes_with_size_and_mtime() {
        let site = Site::new();
        let app = site.app();
        let first = etag(&app.handle(get("/app.css")).await);
        assert!(first.starts_with('"') && first.ends_with('"'), "{first}");
        assert_eq!(first, etag(&app.handle(get("/app.css")).await));

        let path = site.root().join("app.css");
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "body { color: blue; }").unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(mtime).unwrap();
        drop(file);
        let resized = etag(&app.handle(get("/app.css")).await);
        assert_ne!(first, resized, "size change");

        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(mtime + Duration::from_secs(5)).unwrap();
        drop(file);
        let touched = etag(&app.handle(get("/app.css")).await);
        assert_ne!(resized, touched, "mtime change");
    }

    #[tokio::test]
    async fn embedded_etag_is_the_fnv1a_64_of_the_bytes() {
        const FILES: &[(&str, &[u8])] = &[("foobar.txt", b"foobar"), ("empty.txt", b"")];
        let app = mount(StaticResources::new("/", EmbeddedResources::new(FILES)));
        let res = app.handle(get("/foobar.txt")).await;
        assert_eq!(etag(&res), "\"85944171f73967e8\"");
        assert!(header(&res, "last-modified").is_none());
        let res = app.handle(get("/empty.txt")).await;
        assert_eq!(etag(&res), "\"cbf29ce484222325\"");
    }

    #[tokio::test]
    async fn disk_last_modified_is_a_valid_http_date() {
        let site = Site::new();
        let path = site.root().join("app.css");
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        // 1994-11-06T08:49:37Z, the example date of RFC 9110.
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777))
            .unwrap();
        drop(file);
        let res = site.app().handle(get("/app.css")).await;
        assert_eq!(
            header(&res, "last-modified"),
            Some("Sun, 06 Nov 1994 08:49:37 GMT")
        );
    }

    #[tokio::test]
    async fn last_modified_is_formatted_for_a_leap_day() {
        let site = Site::new();
        let path = site.root().join("app.css");
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        // 2024-02-29T23:59:59Z.
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_709_251_199))
            .unwrap();
        drop(file);
        let res = site.app().handle(get("/app.css")).await;
        assert_eq!(
            header(&res, "last-modified"),
            Some("Thu, 29 Feb 2024 23:59:59 GMT")
        );
    }

    #[tokio::test]
    async fn cache_control_is_no_cache_for_zero_and_public_max_age_otherwise() {
        let site = Site::new();
        let res = site.app().handle(get("/app.css")).await;
        assert_eq!(header(&res, "cache-control"), Some("no-cache"));

        let app = mount(
            StaticResources::new("/", DiskResources::new(site.root()))
                .cache_max_age(Duration::from_secs(3600)),
        );
        let res = app.handle(get("/app.css")).await;
        assert_eq!(header(&res, "cache-control"), Some("public, max-age=3600"));
    }
}

mod conditional {
    use super::*;

    const FILES: &[(&str, &[u8])] = &[("foobar.txt", b"foobar")];
    const TAG: &str = "\"85944171f73967e8\"";

    fn embedded() -> App {
        mount(
            StaticResources::new("/", EmbeddedResources::new(FILES))
                .cache_max_age(Duration::from_secs(60)),
        )
    }

    fn assert_not_modified(res: &Response) {
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert!(res.body().is_empty());
        assert_eq!(header(res, "etag"), Some(TAG));
        assert_eq!(header(res, "cache-control"), Some("public, max-age=60"));
    }

    #[tokio::test]
    async fn if_none_match_matches_exact_weak_list_and_star() {
        let app = embedded();
        for value in [
            TAG.to_owned(),
            format!("W/{TAG}"),
            format!("\"other\", W/\"x\" , {TAG}"),
            "*".to_owned(),
        ] {
            for method in [HttpMethod::Get, HttpMethod::Head] {
                let req = with(Request::new(method, "/foobar.txt"), "if-none-match", &value);
                assert_not_modified(&app.handle(req).await);
            }
        }
    }

    #[tokio::test]
    async fn if_none_match_in_two_header_lines_is_one_list() {
        let req = with(get("/foobar.txt"), "if-none-match", "\"other\"");
        let req = with(req, "if-none-match", TAG);
        assert_not_modified(&embedded().handle(req).await);
    }

    #[tokio::test]
    async fn if_none_match_that_does_not_match_gives_the_full_answer() {
        let app = embedded();
        for value in ["\"other\"", "W/\"other\"", "85944171f73967e8", "garbage"] {
            let req = with(get("/foobar.txt"), "if-none-match", value);
            let res = app.handle(req).await;
            assert_eq!(res.status(), 200, "{value}");
            assert_eq!(body(&res), "foobar");
        }
    }

    #[tokio::test]
    async fn not_modified_from_disk_carries_last_modified() {
        let site = Site::new();
        let app = site.app();
        let full = app.handle(get("/app.css")).await;
        let tag = header(&full, "etag").unwrap().to_owned();
        let modified = header(&full, "last-modified").unwrap().to_owned();
        let res = app
            .handle(with(get("/app.css"), "if-none-match", &tag))
            .await;
        assert_eq!(res.status(), 304);
        assert!(res.body().is_empty());
        assert_eq!(header(&res, "etag"), Some(tag.as_str()));
        assert_eq!(header(&res, "last-modified"), Some(modified.as_str()));
        assert_eq!(header(&res, "cache-control"), Some("no-cache"));
    }

    #[tokio::test]
    async fn if_modified_since_is_honored_without_if_none_match() {
        let site = Site::new();
        let path = site.root().join("app.css");
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777))
            .unwrap();
        drop(file);
        let app = site.app();

        let same = "Sun, 06 Nov 1994 08:49:37 GMT";
        let later = "Mon, 07 Nov 1994 08:49:37 GMT";
        let earlier = "Sat, 05 Nov 1994 08:49:37 GMT";
        for (value, status) in [(same, 304), (later, 304), (earlier, 200)] {
            let res = app
                .handle(with(get("/app.css"), "if-modified-since", value))
                .await;
            assert_eq!(res.status(), status, "{value}");
        }
    }

    #[tokio::test]
    async fn if_modified_since_is_ignored_when_if_none_match_is_present() {
        let site = Site::new();
        let app = site.app();
        let req = with(get("/app.css"), "if-none-match", "\"other\"");
        let req = with(req, "if-modified-since", "Fri, 31 Dec 9999 23:59:59 GMT");
        assert_eq!(app.handle(req).await.status(), 200);
    }

    #[tokio::test]
    async fn a_bad_if_modified_since_date_is_ignored() {
        let site = Site::new();
        let app = site.app();
        for value in [
            "yesterday",
            "Sun, 06 Nov 1994 08:49:37 PST",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "",
        ] {
            let res = app
                .handle(with(get("/app.css"), "if-modified-since", value))
                .await;
            assert_eq!(res.status(), 200, "{value}");
        }
    }

    #[cfg(feature = "gzip")]
    #[tokio::test]
    async fn a_compressed_answer_is_answered_304_for_its_weak_etag() {
        let big = "body { color: red }\n".repeat(200);
        let site = Site::new();
        site.file("big.css", &big);
        let app = App::new()
            .compression(oxbloom_web::CompressionConfig::new())
            .routes(Controller::routes(Arc::new(StaticResources::new(
                "/",
                DiskResources::new(site.root()),
            ))))
            .unwrap();
        let full = app
            .handle(with(get("/big.css"), "accept-encoding", "gzip"))
            .await;
        assert_eq!(full.status(), 200);
        assert!(header(&full, "content-encoding").is_some());
        let tag = header(&full, "etag").unwrap().to_owned();
        assert!(tag.starts_with("W/\""), "{tag}");

        let req = with(get("/big.css"), "accept-encoding", "gzip");
        let res = app.handle(with(req, "if-none-match", &tag)).await;
        assert_eq!(res.status(), 304);
        assert!(res.body().is_empty());
    }
}

mod methods {
    use super::*;

    #[tokio::test]
    async fn head_has_the_get_headers_and_no_body() {
        let site = Site::new();
        let app = site.app();
        let full = app.handle(get("/app.css")).await;
        let head = app.handle(Request::new(HttpMethod::Head, "/app.css")).await;
        assert_eq!(head.status(), 200);
        assert!(head.body().is_empty());
        for name in ["content-type", "etag", "last-modified", "cache-control"] {
            assert_eq!(header(&head, name), header(&full, name), "{name}");
        }
        let head = app.handle(Request::new(HttpMethod::Head, "/")).await;
        assert_eq!(head.status(), 200);
        assert!(head.body().is_empty());
    }

    #[cfg(feature = "gzip")]
    #[tokio::test]
    async fn head_is_compressed_like_get() {
        let big = "body { color: red }\n".repeat(200);
        let site = Site::new();
        site.file("big.css", &big);
        let app = App::new()
            .compression(oxbloom_web::CompressionConfig::new())
            .routes(Controller::routes(Arc::new(StaticResources::new(
                "/",
                DiskResources::new(site.root()),
            ))))
            .unwrap();
        let req = with(
            Request::new(HttpMethod::Head, "/big.css"),
            "accept-encoding",
            "gzip",
        );
        let res = app.handle(req).await;
        assert_eq!(res.status(), 200);
        assert!(res.body().is_empty());
        assert_eq!(header(&res, "content-encoding"), Some("gzip"));
        assert!(header(&res, "etag").unwrap().starts_with("W/"));
    }

    #[tokio::test]
    async fn post_is_405_with_allow_get_head() {
        let site = Site::new();
        let app = site.app();
        for path in ["/", "/app.css", "/missing"] {
            let res = app.handle(Request::new(HttpMethod::Post, path)).await;
            assert_eq!(res.status(), 405, "{path}");
            assert_eq!(header(&res, "allow"), Some("GET, HEAD, OPTIONS"), "{path}");
        }
    }

    #[test]
    fn the_controller_registers_get_and_head_for_the_prefix_and_below() {
        let routes = Controller::routes(Arc::new(StaticResources::new(
            "/",
            EmbeddedResources::new(&[]),
        )));
        let mut seen: Vec<(HttpMethod, String)> =
            routes.into_iter().map(|r| (r.method, r.path)).collect();
        seen.sort_by(|a, b| (a.1.as_str(), a.0.as_str()).cmp(&(b.1.as_str(), b.0.as_str())));
        assert_eq!(
            seen,
            vec![
                (HttpMethod::Get, "/".to_owned()),
                (HttpMethod::Head, "/".to_owned()),
                (HttpMethod::Get, "/{*rest}".to_owned()),
                (HttpMethod::Head, "/{*rest}".to_owned()),
            ]
        );
    }
}

mod limits_and_errors {
    use super::*;

    #[tokio::test]
    async fn file_over_max_file_bytes_is_404_and_logs_a_warning() {
        let site = Site::new();
        site.file("big.bin", &"x".repeat(100));
        site.file("small.bin", &"x".repeat(10));
        let app = mount(StaticResources::new(
            "/",
            DiskResources::new(site.root()).max_file_bytes(50),
        ));
        let (res, logs) = captured(&app, get("/big.bin")).await;
        assert_eq!(res.status(), 404);
        assert!(logs.contains("WARN"), "{logs}");
        assert!(logs.contains("oxbloom::web"), "{logs}");
        assert!(logs.contains("big.bin"), "{logs}");

        let res = app.handle(get("/small.bin")).await;
        assert_eq!(res.status(), 200);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unreadable_file_is_a_500_problem_and_logs_an_error() {
        use std::os::unix::fs::PermissionsExt;

        let site = Site::new();
        let path = site.root().join("locked.txt");
        std::fs::write(&path, "locked").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            // Running as a user that ignores file permissions, such as root.
            return;
        }
        let (res, logs) = captured(&site.app(), get("/locked.txt")).await;
        assert_eq!(res.status(), 500);
        assert_eq!(
            header(&res, "content-type"),
            Some("application/problem+json")
        );
        assert!(!body(&res).contains("ermission"), "{}", body(&res));
        assert!(logs.contains("ERROR"), "{logs}");
        assert!(logs.contains("oxbloom::web"), "{logs}");
        assert!(logs.contains("locked.txt"), "{logs}");
    }

    struct Broken;

    #[async_trait::async_trait]
    impl ResourceSource for Broken {
        async fn read(&self, _path: &str) -> Result<Resource, ResourceError> {
            Err(ResourceError::Io(io::Error::other("disk on fire")))
        }
    }

    #[tokio::test]
    async fn an_io_error_of_the_source_is_a_500_problem_and_logs_an_error() {
        let app = mount(StaticResources::new("/", Broken));
        let (res, logs) = captured(&app, get("/x.txt")).await;
        assert_eq!(res.status(), 500);
        assert_eq!(
            header(&res, "content-type"),
            Some("application/problem+json")
        );
        assert!(!body(&res).contains("fire"));
        assert!(logs.contains("ERROR"), "{logs}");
        assert!(logs.contains("disk on fire"), "{logs}");
        assert!(logs.contains("x.txt"), "{logs}");
    }

    #[tokio::test]
    async fn disk_resources_answers_not_found_for_a_directory_or_the_empty_path() {
        let site = Site::new();
        let disk = DiskResources::new(site.root());
        assert!(matches!(
            disk.read("docs").await,
            Err(ResourceError::NotFound)
        ));
        assert!(matches!(disk.read("").await, Err(ResourceError::NotFound)));
    }

    /// Creates a FIFO with the `mkfifo` program; `false` when it is not available.
    #[cfg(unix)]
    fn mkfifo(path: &std::path::Path) -> bool {
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .is_ok_and(|s| s.success())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_fifo_in_the_root_is_404_and_does_not_block() {
        let site = Site::new();
        if !mkfifo(&site.root().join("pipe")) {
            return;
        }
        let app = site.app();
        let res = tokio::time::timeout(Duration::from_secs(5), app.handle(get("/pipe")))
            .await
            .expect("reading a FIFO must not block");
        assert_eq!(res.status(), 404);
    }
}

mod security_chain {
    use super::*;
    use oxbloom_web::{Filter, IntoResponse, Next, order};

    /// Emulates the raw-path matcher of the security chain: `/admin/**` needs a login.
    pub(super) struct AdminGuard;

    #[async_trait::async_trait]
    impl Filter for AdminGuard {
        async fn filter(&self, req: Request, next: Next<'_>) -> Response {
            if ["/admin/", "/team+ops/", "/caf%C3%A9/"]
                .iter()
                .any(|p| req.path.starts_with(p))
            {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            next.run(req).await
        }
    }

    #[tokio::test]
    async fn a_path_the_security_chain_reads_differently_is_never_served() {
        let site = Site::new();
        site.file("admin/report.html", "ADMIN REPORT");
        let app = site.app().filter_ordered(order::SECURITY, AdminGuard);
        assert_eq!(app.handle(get("/admin/report.html")).await.status(), 401);
        for path in [
            "/%61dmin/report.html",
            "//admin/report.html",
            "/./admin/report.html",
            "/%2e/admin/report.html",
            "/%41dmin/report.html",
            "/admin%2freport.html",
        ] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 404, "{path}");
            assert!(!body(&res).contains("ADMIN REPORT"), "{path}");
        }
    }

    #[tokio::test]
    async fn an_escaped_sub_delimiter_cannot_dodge_a_raw_rule() {
        let site = Site::new();
        site.file("team+ops/x", "TEAM SECRET");
        let app = site.app().filter_ordered(order::SECURITY, AdminGuard);
        assert_eq!(app.handle(get("/team+ops/x")).await.status(), 401);
        for path in ["/team%2Bops/x", "/team%2bops/x"] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 404, "{path}");
            assert!(!body(&res).contains("TEAM SECRET"), "{path}");
        }
    }
}

mod raw_spelling {
    use super::*;

    #[tokio::test]
    async fn a_raw_non_ascii_spelling_cannot_dodge_a_rule_on_the_escaped_one() {
        let site = Site::new();
        site.file("café/x", "CAFE SECRET");
        let app = site.app().filter_ordered(
            oxbloom_web::order::SECURITY,
            super::security_chain::AdminGuard,
        );
        assert_eq!(app.handle(get("/caf%C3%A9/x")).await.status(), 401);
        let res = app.handle(get("/café/x")).await;
        assert_eq!(res.status(), 404);
        assert!(!body(&res).contains("CAFE SECRET"));
    }

    #[tokio::test]
    async fn non_ascii_must_be_escaped_and_is_served_only_escaped() {
        let site = Site::new();
        site.file("é.txt", "accent");
        site.file("café/x.txt", "cafe");
        let app = site.app();
        assert_eq!(body(&app.handle(get("/%C3%A9.txt")).await), "accent");
        assert_eq!(body(&app.handle(get("/caf%C3%A9/x.txt")).await), "cafe");
        for path in ["/é.txt", "/café/x.txt", "/%c3%a9.txt"] {
            assert_eq!(app.handle(get(path)).await.status(), 404, "{path}");
        }
    }

    #[tokio::test]
    async fn quote_and_braces_must_be_escaped() {
        let site = Site::new();
        site.file("{x}", "braces");
        site.file("\"q\"", "quoted");
        let app = site.app();
        assert_eq!(body(&app.handle(get("/%7Bx%7D")).await), "braces");
        assert_eq!(body(&app.handle(get("/%22q%22")).await), "quoted");
        for path in ["/{x}", "/%7Bx}", "/\"q\"", "/%22q\""] {
            assert_eq!(app.handle(get(path)).await.status(), 404, "{path}");
        }
    }

    #[tokio::test]
    async fn brackets_caret_and_pipe_must_be_raw() {
        let site = Site::new();
        site.file("a[b]^c|d.txt", "raw");
        let app = site.app();
        assert_eq!(body(&app.handle(get("/a[b]^c|d.txt")).await), "raw");
        for path in [
            "/a%5Bb]^c|d.txt",
            "/a[b%5D^c|d.txt",
            "/a[b]%5Ec|d.txt",
            "/a[b]^c%7Cd.txt",
            "/a%5Bb%5D%5Ec%7Cd.txt",
        ] {
            assert_eq!(app.handle(get(path)).await.status(), 404, "{path}");
        }
    }

    #[tokio::test]
    async fn characters_that_cannot_appear_raw_are_served_from_their_escape() {
        let site = Site::new();
        for name in [
            "a#b.txt", "a?b.txt", "a<b.txt", "a>b.txt", "a`b.txt", "a%b.txt", "a b.txt",
        ] {
            site.file(name, name);
        }
        let app = site.app();
        for (escaped, name) in [
            ("/a%23b.txt", "a#b.txt"),
            ("/a%3Fb.txt", "a?b.txt"),
            ("/a%3Cb.txt", "a<b.txt"),
            ("/a%3Eb.txt", "a>b.txt"),
            ("/a%60b.txt", "a`b.txt"),
            ("/a%25b.txt", "a%b.txt"),
            ("/a%20b.txt", "a b.txt"),
        ] {
            let res = app.handle(get(escaped)).await;
            assert_eq!(res.status(), 200, "{escaped}");
            assert_eq!(body(&res), name);
        }
        // Their raw forms, which browsers never send, have no second spelling either.
        for path in ["/a<b.txt", "/a>b.txt", "/a`b.txt", "/a b.txt", "/a\tb.txt"] {
            assert_eq!(app.handle(get(path)).await.status(), 404, "{path:?}");
        }
    }

    const NAMES: &[(&str, &str)] = &[
        ("a+b.txt", "/a%2Bb.txt"),
        ("(x).txt", "/%28x%29.txt"),
        ("@x.txt", "/%40x.txt"),
        (";x.txt", "/%3Bx.txt"),
        ("=x.txt", "/%3Dx.txt"),
        (":x.txt", "/%3Ax.txt"),
        ("*x.txt", "/%2Ax.txt"),
        ("*y.txt", "/%2ay.txt"),
        ("!x.txt", "/%21x.txt"),
        ("$x.txt", "/%24x.txt"),
        ("&x.txt", "/%26x.txt"),
        ("'x.txt", "/%27x.txt"),
        (",x.txt", "/%2Cx.txt"),
    ];

    #[tokio::test]
    async fn an_escaped_pchar_is_404_while_the_raw_spelling_is_served() {
        let site = Site::new();
        for (name, _) in NAMES {
            site.file(name, name);
        }
        site.file("team+ops/x", "team");
        let app = site.app();
        for (name, escaped) in NAMES {
            let raw = app.handle(get(&format!("/{name}"))).await;
            assert_eq!(raw.status(), 200, "{name}");
            assert_eq!(body(&raw), *name);
            assert_eq!(app.handle(get(escaped)).await.status(), 404, "{escaped}");
        }
        assert_eq!(body(&app.handle(get("/team+ops/x")).await), "team");
    }

    #[tokio::test]
    async fn escapes_must_use_upper_case_hex_digits() {
        let site = Site::new();
        site.file("é.txt", "accent");
        site.file("my file.txt", "spaced");
        let app = site.app();
        assert_eq!(body(&app.handle(get("/%C3%A9.txt")).await), "accent");
        assert_eq!(body(&app.handle(get("/my%20file.txt")).await), "spaced");
        for path in ["/%c3%a9.txt", "/%c3%A9.txt", "/%C3%a9.txt", "/%C3%Ab.txt"] {
            assert_eq!(app.handle(get(path)).await.status(), 404, "{path}");
        }
    }
}

mod mounting {
    use super::*;

    const FILES: &[(&str, &[u8])] = &[
        ("app.css", b"css"),
        ("index.html", b"home"),
        ("img/logo.svg", b"<svg/>"),
    ];

    fn assets() -> App {
        mount(StaticResources::new(
            "/assets",
            EmbeddedResources::new(FILES),
        ))
    }

    #[tokio::test]
    async fn mounting_under_a_prefix_serves_below_it_and_the_prefix_itself() {
        let app = assets();
        let res = app.handle(get("/assets/app.css")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(&res), "css");
        let res = app.handle(get("/assets/img/logo.svg")).await;
        assert_eq!(body(&res), "<svg/>");
        for path in ["/assets", "/assets/"] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 200, "{path}");
            assert_eq!(body(&res), "home", "{path}");
        }
    }

    #[tokio::test]
    async fn paths_outside_the_prefix_are_404_from_the_router() {
        let app = assets();
        for path in ["/other/app.css", "/app.css", "/assetsx/app.css", "/"] {
            let res = app.handle(get(path)).await;
            assert_eq!(res.status(), 404, "{path}");
        }
    }

    #[tokio::test]
    async fn a_prefix_with_a_trailing_slash_or_without_a_leading_slash_is_normalized() {
        for prefix in ["/assets/", "assets", "assets/"] {
            let app = mount(StaticResources::new(prefix, EmbeddedResources::new(FILES)));
            let res = app.handle(get("/assets/app.css")).await;
            assert_eq!(res.status(), 200, "{prefix}");
        }
    }

    #[tokio::test]
    async fn a_prefix_with_braces_is_taken_literally() {
        let app = mount(StaticResources::new("/a{b}", EmbeddedResources::new(FILES)));
        assert_eq!(body(&app.handle(get("/a{b}/app.css")).await), "css");
        assert_eq!(body(&app.handle(get("/a{b}")).await), "home");
        assert_eq!(app.handle(get("/ax/app.css")).await.status(), 404);
    }

    #[tokio::test]
    async fn the_root_mount_sits_next_to_application_routes() {
        let app = mount(StaticResources::new("/", EmbeddedResources::new(FILES)))
            .route(oxbloom_web::Route::new(
                HttpMethod::Get,
                "/api/ping",
                |_| async { "pong" },
            ))
            .unwrap();
        assert_eq!(body(&app.handle(get("/api/ping")).await), "pong");
        assert_eq!(body(&app.handle(get("/app.css")).await), "css");
    }

    #[tokio::test]
    async fn embedded_resources_serve_nested_names_and_miss_unknown_ones() {
        let app = mount(StaticResources::new("/", EmbeddedResources::new(FILES)));
        assert_eq!(app.handle(get("/img/logo.svg")).await.status(), 200);
        assert_eq!(app.handle(get("/img")).await.status(), 404);
        assert_eq!(app.handle(get("/nope")).await.status(), 404);
        assert_eq!(app.handle(get("/../app.css")).await.status(), 404);
    }
}

// Tracing capture, as in `tests/errors.rs`.

thread_local! {
    static CAPTURE: RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { RefCell::new(None) };
}

/// Writes into the buffer of the current thread's capture, or nowhere.
struct ThreadBuf;

impl io::Write for ThreadBuf {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        CAPTURE.with(|c| {
            if let Some(buf) = &*c.borrow() {
                buf.lock().unwrap().extend_from_slice(data);
            }
        });
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for ThreadBuf {
    type Writer = ThreadBuf;

    fn make_writer(&'a self) -> Self::Writer {
        ThreadBuf
    }
}

fn init_tracing() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(ThreadBuf)
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
    });
}

async fn captured(app: &App, req: Request) -> (Response, String) {
    init_tracing();
    let buf = Arc::new(Mutex::new(Vec::new()));
    CAPTURE.with(|c| *c.borrow_mut() = Some(buf.clone()));
    let res = app.handle(req).await;
    CAPTURE.with(|c| *c.borrow_mut() = None);
    let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    (res, out)
}
