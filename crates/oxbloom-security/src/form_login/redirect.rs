//! The redirect-target rule shared by the saved request and every configured URL.

/// The longest redirect target, in bytes.
const MAX_TARGET_BYTES: usize = 2048;

/// Whether `target` is a safe local redirect target.
///
/// A target is safe when it starts with a single `/`, does not start with `//` or `/\`, holds no
/// control character and no backslash, has no `://` before the first `?` and is at most 2048
/// bytes long. `/%5Cx` is a literal path and is accepted; only a real backslash is rejected.
///
/// # Examples
///
/// ```
/// use oxbloom_security::is_safe_redirect_target;
///
/// assert!(is_safe_redirect_target("/"));
/// assert!(is_safe_redirect_target("/a?u=http://b"));
/// assert!(!is_safe_redirect_target("https://evil.example/"));
/// assert!(!is_safe_redirect_target("//evil.example"));
/// assert!(!is_safe_redirect_target("/a\\b"));
/// ```
pub fn is_safe_redirect_target(target: &str) -> bool {
    if target.len() > MAX_TARGET_BYTES || !target.starts_with('/') || target.starts_with("//") {
        return false;
    }
    if target.chars().any(|c| c.is_control() || c == '\\') {
        return false;
    }
    let path = target.split('?').next().unwrap_or(target);
    !path.contains("://")
}

/// The path of a URL or target, without its query.
pub(super) fn path_of(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}
