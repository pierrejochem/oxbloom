//! Compression: codec choice, round trips, skip rules, header effects, `HEAD` and the place of
//! the filter in the chain.

use bytes::Bytes;
use http::{HeaderValue, StatusCode, header};
use oxbloom_web::{
    App, CompressionConfig, CompressionFilter, Filter, HttpMethod, Request, Response, Route,
    codecs, order,
};

fn text() -> String {
    "hello world ".repeat(200)
}

fn reply(content_type: &'static str, body: impl Into<Bytes>, extra: &[(&str, &str)]) -> Response {
    let mut res = Response::new(body.into());
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    for (name, value) in extra {
        res.headers_mut().append(
            header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    res
}

fn app_with(config: CompressionConfig, route: Route) -> App {
    App::new().compression(config).route(route).unwrap()
}

fn serving(config: CompressionConfig, make: fn() -> Response) -> App {
    app_with(
        config,
        Route::new(HttpMethod::Get, "/r", move |_| async move { make() }),
    )
}

fn plain_text() -> Response {
    reply("text/plain", text(), &[])
}

fn request(method: HttpMethod, accept: Option<&str>) -> Request {
    let mut req = Request::new(method, "/r");
    if let Some(a) = accept {
        req.headers
            .insert(header::ACCEPT_ENCODING, HeaderValue::from_str(a).unwrap());
    }
    req
}

async fn get(app: &App, accept: Option<&str>) -> Response {
    app.handle(request(HttpMethod::Get, accept)).await
}

fn encoding(res: &Response) -> Option<&str> {
    res.headers()
        .get(header::CONTENT_ENCODING)
        .map(|v| v.to_str().unwrap())
}

#[cfg(any(feature = "gzip", feature = "brotli", feature = "zstd"))]
fn decode(coding: &str, body: &[u8]) -> Vec<u8> {
    #[allow(unused_imports)]
    use std::io::Read;
    match coding {
        #[cfg(feature = "gzip")]
        "gzip" => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(body)
                .read_to_end(&mut out)
                .unwrap();
            out
        }
        #[cfg(feature = "brotli")]
        "br" => {
            let mut out = Vec::new();
            brotli::Decompressor::new(body, 4096)
                .read_to_end(&mut out)
                .unwrap();
            out
        }
        #[cfg(feature = "zstd")]
        "zstd" => zstd::decode_all(body).unwrap(),
        other => panic!("cannot decode {other}"),
    }
}

fn config() -> CompressionConfig {
    CompressionConfig::new()
}

#[test]
fn codecs_lists_exactly_the_compiled_codecs() {
    let mut expected = Vec::new();
    if cfg!(feature = "zstd") {
        expected.push("zstd");
    }
    if cfg!(feature = "brotli") {
        expected.push("br");
    }
    if cfg!(feature = "gzip") {
        expected.push("gzip");
    }
    assert_eq!(codecs(), expected.as_slice());
}

#[test]
fn the_filter_runs_in_the_compression_slot() {
    assert_eq!(CompressionFilter::new(config()).order(), order::COMPRESSION);
}

#[cfg(all(feature = "gzip", feature = "brotli", feature = "zstd"))]
#[tokio::test]
async fn codec_choice_follows_accept_encoding() {
    let table: &[(Option<&str>, Option<&str>)] = &[
        (Some("gzip"), Some("gzip")),
        (Some("br"), Some("br")),
        (Some("zstd"), Some("zstd")),
        (Some("gzip, br"), Some("br")),
        (Some("gzip;q=0.5, br;q=0.4"), Some("gzip")),
        (Some("gzip, br, zstd"), Some("zstd")),
        (Some("gzip;q=0.8, br;q=0.8, zstd;q=0.8"), Some("zstd")),
        (Some("*"), Some("zstd")),
        (Some("*;q=0"), None),
        (Some("*;q=0, gzip"), Some("gzip")),
        (Some("gzip, *;q=0"), Some("gzip")),
        (Some("br;q=0, *"), Some("zstd")),
        (Some("identity"), None),
        (Some("GZIP"), Some("gzip")),
        (Some("Br, GzIp;q=0.1"), Some("br")),
        (Some("deflate, compress"), None),
        (Some("deflate, gzip"), Some("gzip")),
        (Some("gzip, gzip;q=0"), Some("gzip")),
        (Some("gzip;q=0, gzip"), None),
        (Some("br;q=0, gzip"), Some("gzip")),
        (Some("gzip;q=abc, br"), Some("br")),
        (Some("gzip;q=2, br"), Some("br")),
        (Some(",, ;q=1, gzip"), Some("gzip")),
        (Some("zstd;q=0.001, gzip;q=0"), Some("zstd")),
        (Some(""), None),
        (None, None),
    ];
    let app = serving(config(), plain_text);
    for (accept, expected) in table {
        let res = get(&app, *accept).await;
        assert_eq!(encoding(&res), *expected, "Accept-Encoding: {accept:?}");
    }
}

#[tokio::test]
async fn identity_or_no_header_passes_unchanged() {
    let app = serving(config(), plain_text);
    for accept in [None, Some("identity")] {
        let res = get(&app, accept).await;
        assert_eq!(encoding(&res), None);
        assert_eq!(res.body().as_ref(), text().as_bytes());
        assert!(res.headers().get(header::VARY).is_none());
    }
}

#[tokio::test]
async fn identity_refused_with_no_usable_codec_answers_406_as_problem_json() {
    let app = serving(config(), plain_text);
    for accept in ["identity;q=0", "deflate, identity;q=0", "IDENTITY;q=0"] {
        let res = get(&app, Some(accept)).await;
        assert_eq!(res.status(), StatusCode::NOT_ACCEPTABLE, "{accept}");
        assert_eq!(
            res.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        assert_eq!(encoding(&res), None);
    }
}

#[cfg(feature = "gzip")]
#[tokio::test]
async fn identity_refused_with_a_usable_codec_compresses() {
    let app = serving(config(), plain_text);
    let res = get(&app, Some("identity;q=0, gzip")).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(encoding(&res), Some("gzip"));
}

#[tokio::test]
async fn identity_refused_does_not_touch_a_response_that_would_not_be_compressed() {
    let app = serving(config().min_size(1 << 20), plain_text);
    let res = get(&app, Some("identity;q=0")).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.body().as_ref(), text().as_bytes());
}

macro_rules! round_trip {
    ($name:ident, $feature:literal, $coding:literal) => {
        #[cfg(feature = $feature)]
        #[tokio::test]
        async fn $name() {
            use oxbloom_web::CompressionLevel;
            for level in [
                CompressionLevel::Fast,
                CompressionLevel::Default,
                CompressionLevel::Best,
            ] {
                let app = serving(config().level(level), plain_text);
                let res = get(&app, Some($coding)).await;
                assert_eq!(encoding(&res), Some($coding));
                assert!(res.body().len() < text().len());
                assert_eq!(decode($coding, res.body()), text().as_bytes());
            }
        }
    };
}

round_trip!(gzip_output_decodes_to_the_original_body, "gzip", "gzip");
round_trip!(brotli_output_decodes_to_the_original_body, "brotli", "br");
round_trip!(zstd_output_decodes_to_the_original_body, "zstd", "zstd");

#[cfg(feature = "gzip")]
mod skips {
    use super::*;
    use oxbloom_web::HttpError;

    async fn unchanged(app: &App, extra: Option<&str>) {
        let res = get(app, extra.or(Some("gzip"))).await;
        assert_eq!(encoding(&res), None);
        assert!(res.headers().get(header::VARY).is_none());
    }

    #[tokio::test]
    async fn a_body_under_min_size_passes_unchanged() {
        let small: fn() -> Response = || reply("text/plain", "tiny", &[]);
        unchanged(&serving(config(), small), None).await;
        let exact: fn() -> Response = || reply("text/plain", "x".repeat(1024), &[]);
        let res = get(&serving(config(), exact), Some("gzip")).await;
        assert_eq!(encoding(&res), Some("gzip"));
        let below: fn() -> Response = || reply("text/plain", "x".repeat(1023), &[]);
        unchanged(&serving(config(), below), None).await;
    }

    #[tokio::test]
    async fn a_type_that_is_not_in_the_list_passes_unchanged() {
        let png: fn() -> Response = || reply("image/png", text(), &[]);
        unchanged(&serving(config(), png), None).await;
        let custom: fn() -> Response = || reply("application/json", text(), &[]);
        unchanged(&serving(config().mime_types(["text/csv"]), custom), None).await;
    }

    #[tokio::test]
    async fn text_html_with_a_charset_matches_the_text_wildcard() {
        let html: fn() -> Response = || reply("text/html; charset=utf-8", text(), &[]);
        let res = get(&serving(config(), html), Some("gzip")).await;
        assert_eq!(encoding(&res), Some("gzip"));
        let upper: fn() -> Response = || reply("TEXT/HTML ; Charset=UTF-8", text(), &[]);
        let res = get(&serving(config(), upper), Some("gzip")).await;
        assert_eq!(encoding(&res), Some("gzip"));
    }

    #[tokio::test]
    async fn the_default_types_are_compressed() {
        for ty in [
            "text/plain",
            "application/json",
            "application/problem+json",
            "application/javascript",
            "image/svg+xml",
            "application/xml",
        ] {
            let app = app_with(
                config(),
                Route::new(HttpMethod::Get, "/r", move |_| async move {
                    reply(ty, text(), &[])
                }),
            );
            let res = get(&app, Some("gzip")).await;
            assert_eq!(encoding(&res), Some("gzip"), "{ty}");
        }
    }

    #[tokio::test]
    async fn a_response_without_a_content_type_passes_unchanged() {
        let bare: fn() -> Response = || Response::new(Bytes::from(text()));
        unchanged(&serving(config(), bare), None).await;
    }

    #[tokio::test]
    async fn a_response_that_already_has_content_encoding_passes_unchanged() {
        let coded: fn() -> Response =
            || reply("text/plain", text(), &[("content-encoding", "identity")]);
        let res = get(&serving(config(), coded), Some("gzip")).await;
        assert_eq!(res.headers()[header::CONTENT_ENCODING], "identity");
        assert_eq!(res.body().as_ref(), text().as_bytes());
    }

    #[tokio::test]
    async fn status_204_and_304_pass_unchanged() {
        for status in [StatusCode::NO_CONTENT, StatusCode::NOT_MODIFIED] {
            let app = app_with(
                config().min_size(0),
                Route::new(HttpMethod::Get, "/r", move |_| async move {
                    let mut res = reply("text/plain", text(), &[]);
                    *res.status_mut() = status;
                    res
                }),
            );
            let res = get(&app, Some("gzip")).await;
            assert_eq!(res.status(), status);
            assert_eq!(encoding(&res), None);
        }
    }

    #[tokio::test]
    async fn cache_control_no_transform_passes_unchanged() {
        for value in [
            "no-transform",
            "public, No-Transform",
            "max-age=5,no-transform",
        ] {
            let app = app_with(
                config(),
                Route::new(HttpMethod::Get, "/r", move |_| async move {
                    reply("text/plain", text(), &[("cache-control", value)])
                }),
            );
            unchanged(&app, None).await;
        }
        let other: fn() -> Response =
            || reply("text/plain", text(), &[("cache-control", "no-cache")]);
        let res = get(&serving(config(), other), Some("gzip")).await;
        assert_eq!(encoding(&res), Some("gzip"));
    }

    #[tokio::test]
    async fn a_body_over_max_body_bytes_passes_unchanged() {
        let app = serving(config().max_body_bytes(text().len() - 1), plain_text);
        unchanged(&app, None).await;
        let app = serving(config().max_body_bytes(text().len()), plain_text);
        let res = get(&app, Some("gzip")).await;
        assert_eq!(encoding(&res), Some("gzip"));
    }

    #[tokio::test]
    async fn a_partial_content_response_passes_unchanged() {
        let part: fn() -> Response = || {
            let mut res = reply("text/plain", text(), &[("content-range", "bytes 0-9/100")]);
            *res.status_mut() = StatusCode::PARTIAL_CONTENT;
            res
        };
        unchanged(&serving(config(), part), None).await;
    }

    #[tokio::test]
    async fn an_error_answer_rendered_by_the_error_filter_is_not_compressed() {
        let app = app_with(
            config().min_size(1),
            Route::new(HttpMethod::Get, "/r", |_| async {
                Err::<&str, _>(HttpError::new(StatusCode::BAD_REQUEST))
            }),
        );
        let res = get(&app, Some("gzip")).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_eq!(encoding(&res), None);
        assert!(serde_json::from_slice::<serde_json::Value>(res.body()).is_ok());
    }
}

#[cfg(feature = "gzip")]
mod headers {
    use super::*;

    #[tokio::test]
    async fn content_encoding_is_set_and_content_length_is_removed() {
        let sized: fn() -> Response =
            || reply("text/plain", text(), &[("content-length", "999999")]);
        let res = get(&serving(config(), sized), Some("gzip")).await;
        assert_eq!(encoding(&res), Some("gzip"));
        assert!(res.headers().get(header::CONTENT_LENGTH).is_none());
    }

    #[tokio::test]
    async fn vary_accept_encoding_is_added() {
        let res = get(&serving(config(), plain_text), Some("gzip")).await;
        assert_eq!(res.headers().get_all(header::VARY).iter().count(), 1);
        assert_eq!(res.headers()[header::VARY], "Accept-Encoding");
    }

    #[tokio::test]
    async fn vary_is_kept_when_it_already_lists_accept_encoding() {
        for value in ["accept-encoding", "Origin, Accept-Encoding", "*"] {
            let app = app_with(
                config(),
                Route::new(HttpMethod::Get, "/r", move |_| async move {
                    reply("text/plain", text(), &[("vary", value)])
                }),
            );
            let res = get(&app, Some("gzip")).await;
            let all: Vec<_> = res.headers().get_all(header::VARY).iter().collect();
            assert_eq!(all, [value], "{value}");
        }
    }

    #[tokio::test]
    async fn other_vary_values_are_extended() {
        let origin: fn() -> Response = || reply("text/plain", text(), &[("vary", "Origin")]);
        let res = get(&serving(config(), origin), Some("gzip")).await;
        let all: Vec<_> = res.headers().get_all(header::VARY).iter().collect();
        assert_eq!(all, ["Origin", "Accept-Encoding"]);
    }

    #[tokio::test]
    async fn a_strong_etag_becomes_weak() {
        let strong: fn() -> Response = || reply("text/plain", text(), &[("etag", "\"x\"")]);
        let res = get(&serving(config(), strong), Some("gzip")).await;
        assert_eq!(res.headers()[header::ETAG], "W/\"x\"");
    }

    #[tokio::test]
    async fn a_weak_etag_stays() {
        let weak: fn() -> Response = || reply("text/plain", text(), &[("etag", "W/\"x\"")]);
        let res = get(&serving(config(), weak), Some("gzip")).await;
        assert_eq!(res.headers()[header::ETAG], "W/\"x\"");
    }

    #[tokio::test]
    async fn other_headers_are_kept() {
        let tagged: fn() -> Response = || {
            reply(
                "text/plain",
                text(),
                &[("x-keep", "1"), ("cache-control", "public")],
            )
        };
        let res = get(&serving(config(), tagged), Some("gzip")).await;
        assert_eq!(res.headers()["x-keep"], "1");
        assert_eq!(res.headers()[header::CACHE_CONTROL], "public");
        assert_eq!(res.headers()[header::CONTENT_TYPE], "text/plain");
    }

    #[tokio::test]
    async fn a_get_with_no_matching_codec_is_unchanged() {
        let strong: fn() -> Response = || reply("text/plain", text(), &[("etag", "\"x\"")]);
        let app = serving(config(), strong);
        let res = get(&app, Some("deflate")).await;
        assert_eq!(encoding(&res), None);
        assert_eq!(res.headers()[header::ETAG], "\"x\"");
        assert!(res.headers().get(header::VARY).is_none());
        assert_eq!(res.body().as_ref(), text().as_bytes());
    }

    fn etagged() -> Response {
        reply("text/plain", text(), &[("etag", "\"x\"")])
    }

    // The router drops the body of a HEAD answer and records its length, so the filter decides
    // for HEAD exactly as for GET.
    #[tokio::test]
    async fn head_of_a_plain_handler_gets_the_headers_of_the_get_and_an_empty_body() {
        let app = serving(config(), etagged);
        let head = app.handle(request(HttpMethod::Head, Some("gzip"))).await;
        let get = get(&app, Some("gzip")).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(head.body().is_empty());
        assert_eq!(encoding(&head), Some("gzip"));
        assert!(head.headers().get(header::CONTENT_LENGTH).is_none());
        let sorted = |res: &Response| {
            let mut v: Vec<_> = res
                .headers()
                .iter()
                .map(|(n, v)| (n.as_str().to_string(), v.as_bytes().to_vec()))
                .collect();
            v.sort();
            v
        };
        assert_eq!(sorted(&head), sorted(&get));
        assert_eq!(head.headers()[header::ETAG], "W/\"x\"");
        assert_eq!(head.headers()[header::VARY], "Accept-Encoding");
    }

    #[tokio::test]
    async fn head_whose_get_would_not_be_compressed_stays_unchanged() {
        let small: fn() -> Response = || reply("text/plain", "tiny", &[("etag", "\"x\"")]);
        let head = app_head(serving(config(), small)).await;
        assert_eq!(encoding(&head), None);
        assert!(head.body().is_empty());
        assert_eq!(head.headers()[header::ETAG], "\"x\"");
        assert!(head.headers().get(header::VARY).is_none());

        let png: fn() -> Response = || reply("image/png", text(), &[]);
        assert_eq!(encoding(&app_head(serving(config(), png)).await), None);

        let capped = serving(config().max_body_bytes(text().len() - 1), etagged);
        assert_eq!(encoding(&app_head(capped).await), None);

        let app = serving(config(), etagged);
        let res = app.handle(request(HttpMethod::Head, Some("deflate"))).await;
        assert_eq!(encoding(&res), None);
    }

    async fn app_head(app: App) -> Response {
        app.handle(request(HttpMethod::Head, Some("gzip"))).await
    }

    #[tokio::test]
    async fn head_of_a_get_with_no_transform_stays_unchanged() {
        let nt: fn() -> Response =
            || reply("text/plain", text(), &[("cache-control", "no-transform")]);
        assert_eq!(encoding(&app_head(serving(config(), nt)).await), None);
    }

    struct Bodiless;

    #[async_trait::async_trait]
    impl Filter for Bodiless {
        async fn filter(&self, _req: Request, _next: oxbloom_web::Next<'_>) -> Response {
            reply(
                "text/plain",
                Bytes::new(),
                &[("content-length", &text().len().to_string())],
            )
        }
    }

    // Without the router's marker, a Content-Length is the only size there is.
    #[tokio::test]
    async fn head_without_the_marker_is_sized_by_content_length() {
        let app = App::new()
            .compression(config())
            .filter_ordered(order::SECURITY, Bodiless);
        let head = app_head(app).await;
        assert_eq!(encoding(&head), Some("gzip"));
        assert!(head.headers().get(header::CONTENT_LENGTH).is_none());
        assert!(head.body().is_empty());
    }

    #[tokio::test]
    async fn two_accept_encoding_lines_are_combined() {
        let app = serving(config(), plain_text);
        let mut req = request(HttpMethod::Get, Some("deflate"));
        req.headers
            .append(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        assert_eq!(encoding(&app.handle(req).await), Some("gzip"));
    }
}

#[cfg(feature = "gzip")]
mod chain {
    use super::*;
    use oxbloom_web::Next;

    struct Late;

    #[async_trait::async_trait]
    impl Filter for Late {
        async fn filter(&self, _req: Request, _next: Next<'_>) -> Response {
            plain_text()
        }
    }

    #[tokio::test]
    async fn an_answer_of_a_security_slot_filter_is_compressed() {
        let app = App::new()
            .compression(config())
            .filter_ordered(order::SECURITY, Late);
        let res = get(&app, Some("gzip")).await;
        assert_eq!(encoding(&res), Some("gzip"));
        assert_eq!(decode("gzip", res.body()), text().as_bytes());
    }

    #[tokio::test]
    async fn compression_is_registered_in_its_slot_regardless_of_call_order() {
        let app = App::new()
            .filter_ordered(order::SECURITY, Late)
            .compression(config());
        let res = get(&app, Some("gzip")).await;
        assert_eq!(encoding(&res), Some("gzip"));
    }
}

#[tokio::test]
async fn max_body_bytes_zero_never_compresses_a_non_empty_body() {
    let app = serving(config().max_body_bytes(0), plain_text);
    let res = get(&app, Some("gzip")).await;
    assert_eq!(encoding(&res), None);
}

#[cfg(not(any(feature = "gzip", feature = "brotli", feature = "zstd")))]
#[tokio::test]
async fn without_any_codec_feature_nothing_is_compressed() {
    let app = serving(config(), plain_text);
    for accept in ["gzip", "br", "zstd", "*"] {
        let res = get(&app, Some(accept)).await;
        assert_eq!(encoding(&res), None);
        assert_eq!(res.body().as_ref(), text().as_bytes());
    }
}
