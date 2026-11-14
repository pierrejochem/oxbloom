//! Compile-fail checks for `#[scheduling]` and `#[scheduled]`.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/scheduling/*.rs");
}
