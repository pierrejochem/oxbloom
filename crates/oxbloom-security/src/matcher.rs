//! Ant-style request matching.

use oxbloom_core::HttpMethod;
use oxbloom_web::Request;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Star,
    DoubleStar,
}

/// Matches requests by optional HTTP method and an Ant-style path pattern.
///
/// `*` matches exactly one non-empty segment, `**` matches zero or more segments.
/// Literal segments are exact and case-sensitive; a trailing slash on the request is
/// ignored. Matching is done on the raw, undecoded request path (`/a%2Fb` is one segment
/// `a%2Fb`), and a leading slash in the pattern is optional (`api/**` equals `/api/**`).
/// A matcher for `GET` does not match `HEAD`; [`SecurityFilterChain`](crate::SecurityFilterChain)
/// closes the router's `HEAD`-to-`GET` fallback by also trying every rule with the method
/// treated as `GET`.
///
/// Any other use of `*`, and any `{`, `}` or `?` in a segment, makes the pattern invalid
/// (`{id}`, `*.pdf`, `a*`, `***`): an invalid pattern never matches, and
/// [`HttpSecurity::build`](crate::HttpSecurity::build) rejects it with
/// [`SecurityBuildError::InvalidPattern`](crate::SecurityBuildError::InvalidPattern).
///
/// # Examples
///
/// ```
/// use oxbloom_security::RequestMatcher;
/// use oxbloom_web::{HttpMethod, Request};
///
/// let matcher = RequestMatcher::new(Some(HttpMethod::Get), "/api/**");
/// assert!(matcher.matches(&Request::new(HttpMethod::Get, "/api/users/1")));
/// assert!(!matcher.matches(&Request::new(HttpMethod::Post, "/api/users/1")));
/// ```
#[derive(Debug, Clone)]
pub struct RequestMatcher {
    method: Option<HttpMethod>,
    segments: Vec<Segment>,
    valid: bool,
}

fn split(path: &str) -> impl Iterator<Item = &str> {
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    // An empty path has no segments; `split` would otherwise yield one empty segment.
    (!trimmed.is_empty())
        .then(|| trimmed.split('/'))
        .into_iter()
        .flatten()
}

impl RequestMatcher {
    /// Builds a matcher. `None` as the method matches any method. It never fails: an invalid
    /// pattern (see [`is_valid`](Self::is_valid)) gives a matcher that never matches.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::RequestMatcher;
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// let any = RequestMatcher::new(None, "/users/*");
    /// assert!(any.matches(&Request::new(HttpMethod::Delete, "/users/7")));
    /// assert!(!any.matches(&Request::new(HttpMethod::Delete, "/users")));
    /// ```
    pub fn new(method: Option<HttpMethod>, pattern: &str) -> Self {
        let valid = split(pattern)
            .all(|s| !s.contains(['{', '}', '?']) && (!s.contains('*') || s == "*" || s == "**"));
        let segments = split(pattern)
            .map(|s| match s {
                "*" => Segment::Star,
                "**" => Segment::DoubleStar,
                lit => Segment::Literal(lit.to_owned()),
            })
            .collect();
        Self {
            method,
            segments,
            valid,
        }
    }

    /// Whether the pattern is valid Ant syntax: no segment contains `{`, `}` or `?`, and `*`
    /// only appears as a whole `*` or `**` segment.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::RequestMatcher;
    ///
    /// assert!(RequestMatcher::new(None, "/users/*/secret").is_valid());
    /// assert!(RequestMatcher::new(None, "/files/**").is_valid());
    /// assert!(!RequestMatcher::new(None, "/users/{id}").is_valid());
    /// assert!(!RequestMatcher::new(None, "/files/*.pdf").is_valid());
    /// ```
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// Returns true when the request's method and path match.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::RequestMatcher;
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// let m = RequestMatcher::new(Some(HttpMethod::Get), "/health");
    /// assert!(m.matches(&Request::new(HttpMethod::Get, "/health/")));
    /// assert!(!m.matches(&Request::new(HttpMethod::Head, "/health")));
    /// ```
    pub fn matches(&self, req: &Request) -> bool {
        if !self.valid {
            return false;
        }
        if let Some(method) = &self.method
            && *method != req.method
        {
            return false;
        }
        let path: Vec<&str> = split(&req.path).collect();
        self.match_segments(&path)
    }

    fn match_segments(&self, path: &[&str]) -> bool {
        let pat = &self.segments;
        let (mut p, mut s) = (0, 0);
        // Backtrack point: (pattern index after the last `**`, path index it consumed up to).
        let mut star: Option<(usize, usize)> = None;
        loop {
            match pat.get(p) {
                Some(Segment::DoubleStar) => {
                    p += 1;
                    star = Some((p, s));
                    continue;
                }
                Some(seg) if s < path.len() => {
                    let ok = match seg {
                        Segment::Literal(l) => l == path[s],
                        Segment::Star => !path[s].is_empty(),
                        Segment::DoubleStar => unreachable!(),
                    };
                    if ok {
                        p += 1;
                        s += 1;
                        continue;
                    }
                }
                None if s == path.len() => return true,
                _ => {}
            }
            match star {
                Some((sp, ss)) if ss < path.len() => {
                    star = Some((sp, ss + 1));
                    p = sp;
                    s = ss + 1;
                }
                _ => return false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> Request {
        Request::new(HttpMethod::Get, path)
    }

    #[test]
    fn star_matches_one_segment_only() {
        let m = RequestMatcher::new(None, "/users/*");
        assert!(m.matches(&get("/users/1")));
        assert!(!m.matches(&get("/users")));
        assert!(!m.matches(&get("/users/1/x")));
        assert!(!m.matches(&get("/users//")));
        let mid = RequestMatcher::new(None, "/a/*/c");
        assert!(mid.matches(&get("/a/b/c")));
        assert!(!mid.matches(&get("/a//c")));
    }

    #[test]
    fn double_star_matches_zero_or_more() {
        let m = RequestMatcher::new(None, "/a/**/z");
        assert!(m.matches(&get("/a/z")));
        assert!(m.matches(&get("/a/b/z")));
        assert!(m.matches(&get("/a/b/c/z")));
        assert!(!m.matches(&get("/a/b/c")));
        assert!(RequestMatcher::new(None, "/**").matches(&get("/")));
        assert!(RequestMatcher::new(None, "/**").matches(&get("/x/y")));
    }

    #[test]
    fn double_star_matches_base_path_itself() {
        let m = RequestMatcher::new(None, "/a/**");
        assert!(m.matches(&get("/a")));
        assert!(m.matches(&get("/a/")));
        assert!(m.matches(&get("/a/b/c")));
        assert!(!m.matches(&get("/b")));
        assert!(!m.matches(&get("/ab")));
    }

    #[test]
    fn literal_is_exact_and_case_sensitive() {
        let m = RequestMatcher::new(None, "/Admin/panel");
        assert!(m.matches(&get("/Admin/panel")));
        assert!(!m.matches(&get("/admin/panel")));
        assert!(!m.matches(&get("/Admin/panel/x")));
    }

    #[test]
    fn trailing_slash_ignored() {
        let m = RequestMatcher::new(None, "/health");
        assert!(m.matches(&get("/health/")));
        assert!(m.matches(&get("/health")));
        assert!(RequestMatcher::new(None, "/health/").matches(&get("/health")));
    }

    #[test]
    fn method_none_matches_any_method() {
        let m = RequestMatcher::new(None, "/x");
        for method in [HttpMethod::Get, HttpMethod::Post, HttpMethod::Delete] {
            assert!(m.matches(&Request::new(method, "/x")));
        }
        let post = RequestMatcher::new(Some(HttpMethod::Post), "/x");
        assert!(!post.matches(&get("/x")));
    }

    #[test]
    fn invalid_pattern_is_flagged_and_never_matches() {
        for (pattern, path) in [
            ("/users/{id}/secret", "/users/{id}/secret"),
            ("/files/*.pdf", "/files/*.pdf"),
            ("/a?", "/a?"),
            ("***", "***"),
        ] {
            let m = RequestMatcher::new(None, pattern);
            assert!(!m.is_valid(), "{pattern:?}");
            assert!(!m.matches(&get(path)), "{pattern:?}");
        }
        for pattern in ["/a/*/b", "/a/**", "/**", "*", ""] {
            assert!(RequestMatcher::new(None, pattern).is_valid(), "{pattern:?}");
        }
    }

    #[test]
    fn head_does_not_match_get_matcher() {
        let m = RequestMatcher::new(Some(HttpMethod::Get), "/x");
        assert!(m.matches(&get("/x")));
        assert!(!m.matches(&Request::new(HttpMethod::Head, "/x")));
    }
}
