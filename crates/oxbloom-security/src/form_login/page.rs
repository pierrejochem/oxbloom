//! The default login page.

use http::{HeaderValue, StatusCode, header};
use oxbloom_web::Response;

use super::settings::FormLogin;

/// Escapes the five characters that matter in HTML text and quoted attribute values.
pub(crate) fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Builds the default login page. `error` adds the fixed failure text and `logout` the fixed
/// logout text; no request value is ever reflected. Every dynamic value is escaped.
pub(crate) fn login_page(settings: &FormLogin, error: bool, logout: bool) -> Response {
    let mut notices = String::new();
    if error {
        notices.push_str("<p class=\"error\">Invalid username or password.</p>\n");
    }
    if logout {
        notices.push_str("<p class=\"notice\">You have been logged out.</p>\n");
    }
    let body = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Sign in</title>\n\
         <style>body{{font-family:sans-serif;max-width:20em;margin:4em auto}}\
         label,input{{display:block;width:100%;margin:.5em 0}}\
         .error{{color:#b00020}}</style>\n</head>\n<body>\n<h1>Sign in</h1>\n{notices}\
         <form method=\"post\" action=\"{action}\">\n\
         <label for=\"u\">Username</label>\n\
         <input id=\"u\" name=\"{user}\" type=\"text\" autocomplete=\"username\" required>\n\
         <label for=\"p\">Password</label>\n\
         <input id=\"p\" name=\"{pass}\" type=\"password\" autocomplete=\"current-password\" required>\n\
         <button type=\"submit\">Sign in</button>\n</form>\n</body>\n</html>\n",
        action = escape_html(&settings.login_processing_url),
        user = escape_html(&settings.username_field),
        pass = escape_html(&settings.password_field),
    );
    let mut res = Response::new(body.into());
    *res.status_mut() = StatusCode::OK;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'",
        ),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(res: &Response) -> String {
        String::from_utf8(res.body().to_vec()).unwrap()
    }

    #[test]
    fn headers_are_exact() {
        let res = login_page(&FormLogin::default(), false, false);
        assert_eq!(res.status(), 200);
        let mut names: Vec<_> = res
            .headers()
            .keys()
            .map(|k| k.as_str().to_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "cache-control",
                "content-security-policy",
                "content-type",
                "x-content-type-options"
            ]
        );
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert_eq!(res.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            res.headers()["content-security-policy"],
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'"
        );
        assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    }

    #[test]
    fn body_has_the_form_and_no_script() {
        let s = FormLogin::default()
            .login_processing_url("/in")
            .username_field("u1")
            .password_field("p1");
        let body = text(&login_page(&s, false, false));
        assert!(body.contains("action=\"/in\""));
        assert!(body.contains("method=\"post\""));
        assert!(body.contains("name=\"u1\"") && body.contains("name=\"p1\""));
        assert!(!body.contains("<script"));
        assert!(!body.contains("Invalid username"));
        assert!(!body.contains("logged out"));
    }

    #[test]
    fn error_and_logout_add_fixed_texts() {
        let s = FormLogin::default();
        assert!(text(&login_page(&s, true, false)).contains("Invalid username or password."));
        assert!(text(&login_page(&s, false, true)).contains("You have been logged out."));
        let both = text(&login_page(&s, true, true));
        assert!(both.contains("Invalid username") && both.contains("logged out"));
    }

    #[test]
    fn dynamic_values_are_escaped() {
        let s = FormLogin::default()
            .login_processing_url("/a\"><script>&'")
            .username_field("<u>")
            .password_field("\"p\"");
        let body = text(&login_page(&s, false, false));
        assert!(body.contains("action=\"/a&quot;&gt;&lt;script&gt;&amp;&#39;\""));
        assert!(body.contains("name=\"&lt;u&gt;\""));
        assert!(body.contains("name=\"&quot;p&quot;\""));
        assert!(!body.contains("<script"));
    }

    #[test]
    fn escape_html_covers_the_five_characters() {
        assert_eq!(escape_html("<>&\"'"), "&lt;&gt;&amp;&quot;&#39;");
        assert_eq!(escape_html("plain"), "plain");
    }
}
