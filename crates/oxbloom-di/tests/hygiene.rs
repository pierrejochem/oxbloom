//! Error hygiene: developer text and values never reach an error except `Creation.message`.
//!
//! A marker string is put into bean values, captured by providers and passed through factory
//! error messages. It may appear only in `Creation.message`; every other variant, every source
//! label and every `DestroyFailure` must be free of it, in both `Display` and `Debug`. The
//! `TooDeep` case needs a 257-deep chain and lives in `tests/depth.rs`.

use std::sync::Arc;

use oxbloom_di::{Container, ContainerBuilder, DiError, Resolver};

const MARKER: &str = "MARKER-7f3a-secret";

/// A bean whose value holds the marker.
struct Secret(String);

fn secret() -> Secret {
    Secret(MARKER.to_owned())
}

/// Asserts that `e` is not `Creation` and that neither its `Display` nor its `Debug` shows the
/// marker.
fn assert_clean(e: &DiError) {
    assert!(!matches!(e, DiError::Creation { .. }), "{e:?}");
    assert!(!format!("{e}").contains(MARKER), "Display leaks: {e}");
    assert!(!format!("{e:?}").contains(MARKER), "Debug leaks: {e:?}");
}

/// Asserts that `e` is `Creation` with the marker in its message and nowhere else.
fn assert_creation_carries_marker(e: &DiError) {
    match e {
        DiError::Creation {
            type_name, message, ..
        } => {
            assert!(message.contains(MARKER), "{message}");
            assert!(!type_name.contains(MARKER), "{type_name}");
        }
        other => panic!("expected Creation, got {other:?}"),
    }
}

#[test]
fn marker_appears_only_in_creation_message() {
    struct Failing;
    struct Hooked;
    struct Fallible;

    // A factory error message, a `post_construct` error message and a `try_bean` error that is
    // not `Creation` (wrapped as `bean failed: {error}`).
    let factory = ContainerBuilder::new()
        .instance(secret())
        .factory(|r: &Resolver| -> Result<Failing, DiError> {
            let s = r.get::<Secret>()?;
            Err(DiError::creation::<Failing>(format!(
                "cannot start: {}",
                s.0
            )))
        })
        .build()
        .err()
        .unwrap();
    assert_creation_carries_marker(&factory);

    let hook = ContainerBuilder::new()
        .instance(Hooked)
        .post_construct(|_: &Hooked| Err(DiError::creation::<Hooked>(MARKER)))
        .build()
        .err()
        .unwrap();
    assert_creation_carries_marker(&hook);

    let wrapped = ContainerBuilder::new()
        .try_bean(|| -> Result<Fallible, DiError> {
            Err(DiError::Invalid {
                message: MARKER.to_owned(),
            })
        })
        .build()
        .err()
        .unwrap();
    assert_creation_carries_marker(&wrapped);
}

#[test]
fn other_variants_and_labels_never_carry_the_marker() {
    trait Store: Send + Sync {}
    struct A(#[allow(dead_code)] String);
    impl Store for A {}
    struct B(#[allow(dead_code)] String);
    impl Store for B {}
    struct Needs;
    struct Loop1;
    struct Loop2;

    let captured = MARKER.to_owned();

    // Missing: a provider capturing the marker asks for an unregistered type.
    let missing = ContainerBuilder::new()
        .factory(move |r: &Resolver| -> Result<Needs, DiError> {
            let _ = &captured;
            r.get::<u128>()?;
            Ok(Needs)
        })
        .build()
        .err()
        .unwrap();
    assert!(matches!(missing, DiError::Missing { .. }));
    assert_clean(&missing);

    // Ambiguous: two providers of one trait whose values hold the marker; candidates are labels.
    let c = ContainerBuilder::new()
        .instance(A(MARKER.to_owned()))
        .provides(|a: Arc<A>| -> Arc<dyn Store> { a })
        .bean(|| B(MARKER.to_owned()))
        .provides(|b: Arc<B>| -> Arc<dyn Store> { b })
        .build()
        .unwrap();
    let ambiguous = c.get::<dyn Store>().err().unwrap();
    assert!(matches!(ambiguous, DiError::Ambiguous { .. }));
    assert_clean(&ambiguous);

    // Cycle.
    let cycle = ContainerBuilder::new()
        .factory(|r: &Resolver| -> Result<Loop1, DiError> {
            r.get::<Loop2>()?;
            Ok(Loop1)
        })
        .factory(|r: &Resolver| -> Result<Loop2, DiError> {
            r.get::<Loop1>()?;
            Ok(Loop2)
        })
        .build()
        .err()
        .unwrap();
    assert!(matches!(cycle, DiError::Cycle { .. }));
    assert_clean(&cycle);

    // Duplicate: two instances whose values hold the marker; sources are labels.
    let duplicate = ContainerBuilder::new()
        .instance(secret())
        .bean(secret)
        .build()
        .err()
        .unwrap();
    assert!(matches!(duplicate, DiError::Duplicate { .. }));
    assert_clean(&duplicate);

    // Invalid: a modifier whose type does not match a registration holding the marker.
    let invalid = ContainerBuilder::new()
        .instance(secret())
        .post_construct(|_: &A| Ok(()))
        .build()
        .err()
        .unwrap();
    assert!(matches!(invalid, DiError::Invalid { .. }));
    assert_clean(&invalid);

    // ShutDown.
    let c: Container = ContainerBuilder::new().instance(secret()).build().unwrap();
    c.shutdown();
    let shut_down = c.get::<Secret>().err().unwrap();
    assert_eq!(shut_down, DiError::ShutDown);
    assert_clean(&shut_down);
}

#[test]
fn destroy_failures_carry_fixed_text_only() {
    let c = ContainerBuilder::new()
        .instance(secret())
        .pre_destroy(|s: &Secret| panic!("{}", s.0))
        .build()
        .unwrap();
    std::panic::set_hook(Box::new(|_| {}));
    let report = c.shutdown();
    let _ = std::panic::take_hook();
    assert_eq!(report.failures.len(), 1);
    let failure = &report.failures[0];
    assert!(!failure.type_name.contains(MARKER));
    assert_eq!(failure.message, "pre_destroy panicked");
    assert!(!format!("{report:?}").contains(MARKER));
}
