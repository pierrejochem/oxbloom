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
///
/// `csrf` is the field name and the token of the session. With `Some` the form holds the hidden
/// field. The framing protection headers are always added, with or without the field.
///
/// `remember_me` is the name of the remember-me field when remember-me is on; the form then
/// holds a checkbox of that name with the value `on` and a fixed label.
pub(crate) fn login_page(
    settings: &FormLogin,
    error: bool,
    logout: bool,
    csrf: Option<(&str, &str)>,
    remember_me: Option<&str>,
) -> Response {
    let hidden = csrf.map_or_else(String::new, |(name, token)| {
        format!(
            "<input type=\"hidden\" name=\"{}\" value=\"{}\">\n",
            escape_html(name),
            escape_html(token)
        )
    });
    let checkbox = remember_me.map_or_else(String::new, |name| {
        format!(
            "<label><input type=\"checkbox\" name=\"{}\" value=\"on\"> Remember me</label>\n",
            escape_html(name)
        )
    });
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
         <form method=\"post\" action=\"{action}\">\n{hidden}\
         <label for=\"u\">Username</label>\n\
         <input id=\"u\" name=\"{user}\" type=\"text\" autocomplete=\"username\" required>\n\
         <label for=\"p\">Password</label>\n\
         <input id=\"p\" name=\"{pass}\" type=\"password\" autocomplete=\"current-password\" required>\n\
         {checkbox}<button type=\"submit\">Sign in</button>\n</form>\n</body>\n</html>\n",
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
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; \
             frame-ancestors 'none'",
        ),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
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
        let res = login_page(&FormLogin::default(), false, false, None, None);
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
                "x-content-type-options",
                "x-frame-options"
            ]
        );
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert_eq!(res.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            res.headers()["content-security-policy"],
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'"
        );
        assert_eq!(res.headers()["x-frame-options"], "DENY");
        assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    }

    #[test]
    fn body_has_the_form_and_no_script() {
        let s = FormLogin::default()
            .login_processing_url("/in")
            .username_field("u1")
            .password_field("p1");
        let body = text(&login_page(&s, false, false, None, None));
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
        assert!(
            text(&login_page(&s, true, false, None, None))
                .contains("Invalid username or password.")
        );
        assert!(
            text(&login_page(&s, false, true, None, None)).contains("You have been logged out.")
        );
        let both = text(&login_page(&s, true, true, None, None));
        assert!(both.contains("Invalid username") && both.contains("logged out"));
    }

    #[test]
    fn dynamic_values_are_escaped() {
        let s = FormLogin::default()
            .login_processing_url("/a\"><script>&'")
            .username_field("<u>")
            .password_field("\"p\"");
        let body = text(&login_page(&s, false, false, None, None));
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

    #[test]
    fn the_hidden_field_is_in_the_form_and_escaped() {
        let s = FormLogin::default();
        let body = text(&login_page(
            &s,
            false,
            false,
            Some(("_csrf", "abc-_123")),
            None,
        ));
        assert!(body.contains("<input type=\"hidden\" name=\"_csrf\" value=\"abc-_123\">"));
        let form_start = body.find("<form").unwrap();
        assert!(body.find("type=\"hidden\"").unwrap() > form_start);
        let hostile = text(&login_page(&s, false, false, Some(("\"><b>", "'&<")), None));
        assert!(hostile.contains("name=\"&quot;&gt;&lt;b&gt;\" value=\"&#39;&amp;&lt;\""));
        assert!(!hostile.contains("<b>"));
    }

    #[test]
    fn without_csrf_there_is_no_field_but_the_framing_headers_stay() {
        let res = login_page(&FormLogin::default(), false, false, None, None);
        assert!(!text(&res).contains("hidden"));
        assert_eq!(res.headers()["x-frame-options"], "DENY");
        assert!(
            res.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .ends_with("frame-ancestors 'none'")
        );
    }

    #[test]
    fn the_checkbox_is_in_the_form_only_with_remember_me_and_escaped() {
        let s = FormLogin::default();
        let body = text(&login_page(&s, false, false, None, Some("remember-me")));
        let checkbox = "<input type=\"checkbox\" name=\"remember-me\" value=\"on\">";
        assert!(body.contains(&format!("<label>{checkbox} Remember me</label>")));
        let (start, end) = (body.find("<form").unwrap(), body.find("</form>").unwrap());
        let at = body.find(checkbox).unwrap();
        assert!(start < at && at < end);
        assert!(!text(&login_page(&s, false, false, None, None)).contains("checkbox"));
        let hostile = text(&login_page(&s, false, false, None, Some("\"><b>")));
        assert!(hostile.contains("name=\"&quot;&gt;&lt;b&gt;\" value=\"on\""));
        assert!(!hostile.contains("<b>"));
    }
}
