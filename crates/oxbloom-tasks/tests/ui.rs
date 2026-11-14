//! Compile checks for `#[scheduling]`, `#[scheduled]` and `#[retryable]`.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/scheduling/*.rs");
    t.compile_fail("tests/ui/retryable/*.rs");
    t.pass("tests/ui/retryable/pass/*.rs");
}
