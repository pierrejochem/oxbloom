//! Std-only panic catching for the request pipeline.

use std::any::Any;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::StatusCode;

use crate::{ErrorAttributes, ErrorRenderer, HttpMethod, Request, RequestInfo, Response};

/// Polls `F` and turns a panic during any poll into `Err(payload)`.
pub(crate) struct CatchUnwind<F> {
    inner: F,
}

impl<F: Future + Unpin> CatchUnwind<F> {
    pub(crate) fn new(inner: F) -> Self {
        Self { inner }
    }
}

impl<F: Future + Unpin> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = &mut self.inner;
        match catch_unwind(AssertUnwindSafe(|| Pin::new(inner).poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(v)) => Poll::Ready(Ok(v)),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

/// The panic message from a `&'static str` or `String` payload, else `"panic"`.
pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_owned()
    }
}

/// What the error pipeline needs to know about a request after it was moved on.
pub(crate) fn request_info(req: &Request) -> RequestInfo {
    let path = req.path.split('?').next().unwrap_or_default();
    RequestInfo {
        method: req.method.clone(),
        path: path.to_owned(),
    }
}

/// Logs a caught panic and renders it as a 500 through `renderer`.
///
/// The log line escapes control characters of the message (like the request log), so a
/// message cannot forge further log lines; the attributes keep the raw message.
pub(crate) fn panic_response(
    renderer: &dyn ErrorRenderer,
    payload: &(dyn Any + Send),
    info: &RequestInfo,
) -> Response {
    let message = panic_message(payload);
    tracing::error!(
        target: "oxbloom::web",
        method = %info.method,
        path = %info.path.escape_debug(),
        "request panicked: {}",
        message.escape_debug()
    );
    let mut attrs = ErrorAttributes::from_status(StatusCode::INTERNAL_SERVER_ERROR);
    attrs.internal = Some(message);
    let mut res = renderer.render(&attrs, info);
    strip_for_head(&mut res, info);
    res
}

/// A rendered answer to `HEAD` keeps its status and headers but has no body and no
/// `content-length` that would describe the dropped one.
pub(crate) fn strip_for_head(res: &mut Response, info: &RequestInfo) {
    if info.method == HttpMethod::Head {
        *res.body_mut() = Bytes::new();
        res.headers_mut().remove(http::header::CONTENT_LENGTH);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn catch_unwind_returns_the_payload() {
        let ok = CatchUnwind::new(Box::pin(async { 5 })).await;
        assert_eq!(ok.unwrap(), 5);
        let err = CatchUnwind::new(Box::pin(async {
            if true {
                panic!("boom {}", 1);
            }
            5
        }))
        .await
        .unwrap_err();
        assert_eq!(panic_message(&*err), "boom 1");
        let err = CatchUnwind::new(Box::pin(async {
            if true {
                panic!("static");
            }
        }))
        .await
        .unwrap_err();
        assert_eq!(panic_message(&*err), "static");
        let err = CatchUnwind::new(Box::pin(async {
            if true {
                std::panic::panic_any(1_u8);
            }
        }))
        .await
        .unwrap_err();
        assert_eq!(panic_message(&*err), "panic");
    }

    #[derive(Clone, Default)]
    struct Buf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Buf {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
        type Writer = Buf;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn panic_log_line_escapes_control_characters_and_attrs_keep_the_raw_message() {
        struct Capture(std::sync::Mutex<Option<ErrorAttributes>>);
        impl ErrorRenderer for Capture {
            fn render(&self, attrs: &ErrorAttributes, _info: &RequestInfo) -> Response {
                *self.0.lock().unwrap() = Some(attrs.clone());
                Response::new(Bytes::new())
            }
        }
        let buf = Buf::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        let renderer = Capture(std::sync::Mutex::new(None));
        let raw = "first\nsecond\r\u{1b}[31mthird\tend";
        let payload: Box<dyn Any + Send> = Box::new(raw.to_owned());
        let info = request_info(&Request::new(HttpMethod::Get, "/p"));
        tracing::subscriber::with_default(subscriber, || {
            panic_response(&renderer, &*payload, &info);
        });
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert_eq!(out.lines().count(), 1, "{out}");
        assert!(
            !out.trim_end_matches('\n').contains(['\n', '\r', '\u{1b}']),
            "{out:?}"
        );
        assert!(
            out.contains(r"request panicked: first\nsecond\r\u{1b}[31mthird\tend"),
            "{out}"
        );
        let attrs = renderer.0.lock().unwrap().clone().unwrap();
        assert_eq!(attrs.internal.as_deref(), Some(raw));
    }

    #[test]
    fn request_info_cuts_the_query() {
        let info = request_info(&Request::new(HttpMethod::Get, "/a?b=c?d"));
        assert_eq!(info.path, "/a");
        assert_eq!(info.method, HttpMethod::Get);
    }
}
