//! The public surface of the form login settings and the redirect-target rule.

use oxbloom_security::{FormLogin, SecurityBuildError, is_safe_redirect_target};

#[test]
fn redirect_target_accepts_and_rejects() {
    for ok in [
        "/",
        "/a",
        "/a?b=c",
        "/a/b?x=1&y=2",
        "/%5Cx",
        "/a?u=http://b",
    ] {
        assert!(is_safe_redirect_target(ok), "{ok}");
    }
    for bad in [
        "",
        "a",
        "//x",
        "/\\x",
        "https://x",
        "/a\\b",
        "/a\tb",
        "/a\rb",
        "/a\nb",
        "/a\0b",
        "/a://b",
        "/a\u{7f}b",
    ] {
        assert!(!is_safe_redirect_target(bad), "{bad:?}");
    }
    let at_limit = format!("/{}", "a".repeat(2047));
    assert_eq!(at_limit.len(), 2048);
    assert!(is_safe_redirect_target(&at_limit));
    assert!(!is_safe_redirect_target(&format!("{at_limit}a")));
}

#[test]
fn form_login_defaults() {
    let d = FormLogin::default();
    assert_eq!(d.login_page, "/login");
    assert_eq!(d.login_processing_url, "/login");
    assert_eq!(d.logout_url, "/logout");
    assert_eq!(d.default_success_url, "/");
    assert_eq!(d.failure_url, "/login?error");
    assert_eq!(d.logout_success_url, "/login?logout");
    assert_eq!(d.username_field, "username");
    assert_eq!(d.password_field, "password");
    assert!(d.login_page_enabled);
    assert_eq!(d.validate(), Ok(()));
}

#[test]
fn form_login_setters() {
    let f = FormLogin::default()
        .login_page("/signin")
        .login_processing_url("/signin/do")
        .logout_url("/signout")
        .default_success_url("/home")
        .failure_url("/signin?error")
        .logout_success_url("/signin?logout")
        .username_field("user")
        .password_field("pass_word-1")
        .login_page_enabled(false);
    assert_eq!(f.login_page, "/signin");
    assert_eq!(f.login_processing_url, "/signin/do");
    assert_eq!(f.logout_url, "/signout");
    assert_eq!(f.default_success_url, "/home");
    assert_eq!(f.failure_url, "/signin?error");
    assert_eq!(f.logout_success_url, "/signin?logout");
    assert_eq!(f.username_field, "user");
    assert_eq!(f.password_field, "pass_word-1");
    assert!(!f.login_page_enabled);
    assert_eq!(f.clone().validate(), Ok(()));
}

#[test]
fn validate_rejects_bad_urls_and_names() {
    let bad = SecurityBuildError::InvalidFormLoginUrl;
    let d = FormLogin::default;
    assert_eq!(d().login_page("login").validate(), Err(bad));
    assert_eq!(d().login_processing_url("//x").validate(), Err(bad));
    assert_eq!(d().logout_url("https://x").validate(), Err(bad));
    assert_eq!(d().default_success_url("").validate(), Err(bad));
    assert_eq!(d().failure_url("/a\\b").validate(), Err(bad));
    assert_eq!(d().logout_success_url("/a\nb").validate(), Err(bad));
    assert_eq!(d().username_field("").validate(), Err(bad));
    assert_eq!(d().password_field("pa ss").validate(), Err(bad));
    assert_eq!(d().username_field("a=b").validate(), Err(bad));
    assert_eq!(d().password_field("username").validate(), Err(bad));
    // The logout URL must differ from the login page and the processing URL.
    assert_eq!(d().logout_url("/login").validate(), Err(bad));
    assert_eq!(d().logout_url("/login?x").validate(), Err(bad));
    assert_eq!(d().login_processing_url("/logout").validate(), Err(bad));
    // The login page and the processing URL may be equal.
    assert_eq!(
        d().login_page("/a").login_processing_url("/a").validate(),
        Ok(())
    );
}

#[test]
fn build_error_texts_are_fixed() {
    assert_eq!(
        SecurityBuildError::FormLoginWithoutProvider.to_string(),
        "form_login configured without a password provider"
    );
    assert_eq!(
        SecurityBuildError::InvalidFormLoginUrl.to_string(),
        "invalid form login url or field name"
    );
}
