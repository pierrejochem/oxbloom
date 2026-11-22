//! Adds the configured security headers to a response.

use http::{HeaderName, HeaderValue, header};
use oxbloom_web::{Response, Scheme};

use super::HeadersConfig;

/// Sets `name` to `value` unless the response already has the header.
fn set_if_absent(res: &mut Response, name: HeaderName, value: Option<&str>) {
    if let Some(value) = value
        && !res.headers().contains_key(&name)
        && let Ok(value) = HeaderValue::from_str(value)
    {
        res.headers_mut().insert(name, value);
    }
}

/// Adds the headers of `cfg` to `res`. A header the response already has is left alone.
/// `Cache-Control` is added only when `authenticated`, and HSTS only when `scheme` is HTTPS.
pub(crate) fn apply_headers(
    cfg: &HeadersConfig,
    authenticated: bool,
    scheme: Scheme,
    res: &mut Response,
) {
    if !cfg.enabled {
        return;
    }
    set_if_absent(
        res,
        header::X_CONTENT_TYPE_OPTIONS,
        cfg.content_type_options.as_deref(),
    );
    set_if_absent(res, header::REFERRER_POLICY, cfg.referrer_policy.as_deref());
    set_if_absent(res, header::X_FRAME_OPTIONS, cfg.frame_options.as_deref());
    if authenticated {
        set_if_absent(res, header::CACHE_CONTROL, cfg.cache_control.as_deref());
    }
    if scheme == Scheme::Https && cfg.hsts.enabled {
        set_if_absent(
            res,
            header::STRICT_TRANSPORT_SECURITY,
            Some(&cfg.hsts.value()),
        );
    }
}
