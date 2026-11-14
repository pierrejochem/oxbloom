//! Compile-fail checks for the public API.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
    t.compile_fail("tests/ui/autoconfig/*.rs");
    // rustc lists the crate's own `AutoConfiguration` impls in these diagnostics, so the
    // expected text holds only when no feature adds one.
    #[cfg(not(any(feature = "security", feature = "web")))]
    t.compile_fail("tests/ui/autoconfig_no_impls/*.rs");
}
