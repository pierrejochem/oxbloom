//! The typed source of `DiError::Creation`: kept through `try_bean`, `#[bean(fallible)]` and
//! `post_construct`, ignored by equality, never printed, and shared (not copied) on clone.

use std::any::type_name;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use oxbloom_di::{ContainerBuilder, DiError, ErrorSource, configuration};

#[derive(Debug)]
struct MyErr;

impl fmt::Display for MyErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("my error")
    }
}

impl Error for MyErr {}

/// The first error in the `source()` chain of `e` (including `e`) that is a `MyErr`.
fn find_my_err<'a>(e: &'a (dyn Error + 'static)) -> Option<&'a MyErr> {
    let mut current = Some(e);
    while let Some(err) = current {
        if let Some(found) = err.downcast_ref::<MyErr>() {
            return Some(found);
        }
        current = err.source();
    }
    None
}

fn source_of(e: &DiError) -> Option<&ErrorSource> {
    match e {
        DiError::Creation { source, .. } => Some(source),
        _ => None,
    }
}

#[test]
fn creation_from_keeps_message_and_source() {
    struct Db;
    let err = DiError::creation_from::<Db>(MyErr);
    match &err {
        DiError::Creation {
            type_name: t,
            message,
            source,
        } => {
            assert_eq!(t, type_name::<Db>());
            assert_eq!(message, "my error");
            assert!(source.is_some());
        }
        other => panic!("expected Creation, got {other:?}"),
    }
    let source = Error::source(&err).expect("source");
    assert!(source.downcast_ref::<MyErr>().is_some());
    assert!(find_my_err(&err).is_some());
    // `creation` sets an empty source.
    let plain = DiError::creation::<Db>("x");
    assert!(Error::source(&plain).is_none());
    assert!(matches!(&plain, DiError::Creation { source, .. } if !source.is_some()));
}

#[test]
fn equality_ignores_source() {
    struct Db;
    // Two `Creation` errors with the same type name and message but different source errors
    // are equal: `ErrorSource` always compares equal.
    let from_my_err = DiError::Creation {
        type_name: type_name::<Db>().to_owned(),
        message: "my error".to_owned(),
        source: ErrorSource::new(MyErr),
    };
    let from_fmt = DiError::Creation {
        type_name: type_name::<Db>().to_owned(),
        message: "my error".to_owned(),
        source: ErrorSource::new(fmt::Error),
    };
    assert_eq!(from_my_err, from_fmt);
    assert_eq!(DiError::creation_from::<Db>(MyErr), from_fmt);
    assert_eq!(ErrorSource::new(MyErr), ErrorSource::new(fmt::Error));
    // A held source and an empty one are equal too: presence is ignored.
    let without = DiError::Creation {
        type_name: type_name::<Db>().to_owned(),
        message: "my error".to_owned(),
        source: ErrorSource::none(),
    };
    assert_eq!(from_my_err, without);
    assert_eq!(DiError::creation_from::<Db>(MyErr), without);
    assert_eq!(ErrorSource::new(MyErr), ErrorSource::none());
    assert_eq!(ErrorSource::none(), ErrorSource::default());
    // The other fields still count.
    assert_ne!(
        DiError::creation_from::<Db>(MyErr),
        DiError::creation_from::<u8>(MyErr)
    );
    assert_ne!(
        DiError::creation_from::<Db>(MyErr),
        DiError::creation::<Db>("other message")
    );
}

struct Secretive;

impl fmt::Debug for Secretive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SECRET-MARKER-DEBUG")
    }
}

impl fmt::Display for Secretive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SECRET-MARKER-DISPLAY")
    }
}

impl Error for Secretive {}

#[test]
fn debug_does_not_print_source_content() {
    struct Db;
    let err = DiError::Creation {
        type_name: type_name::<Db>().to_owned(),
        message: "cannot connect".to_owned(),
        source: ErrorSource::new(Secretive),
    };
    let debug = format!("{err:?}");
    let display = format!("{err}");
    assert!(!debug.contains("SECRET-MARKER"), "{debug}");
    assert!(!display.contains("SECRET-MARKER"), "{display}");
    assert!(debug.contains("ErrorSource(..)"), "{debug}");
    // An empty source prints the same.
    let empty = format!("{:?}", DiError::creation::<Db>("cannot connect"));
    assert!(empty.contains("ErrorSource(..)"), "{empty}");
    assert_eq!(empty, debug);
    assert_eq!(
        display,
        format!("failed to create {}: cannot connect", type_name::<Db>())
    );
}

#[derive(Debug)]
struct Holder(#[allow(dead_code)] Arc<()>);

impl fmt::Display for Holder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("holder")
    }
}

impl Error for Holder {}

#[test]
fn clone_shares_the_arc() {
    struct Db;
    let marker = Arc::new(());
    let err = DiError::creation_from::<Db>(Holder(Arc::clone(&marker)));
    assert_eq!(Arc::strong_count(&marker), 2);
    let copies: Vec<DiError> = (0..5).map(|_| err.clone()).collect();
    // A deep copy would clone the `Holder` (and its `Arc<()>`); sharing keeps the count at 2.
    assert_eq!(Arc::strong_count(&marker), 2);
    for copy in &copies {
        let held = Error::source(copy)
            .unwrap()
            .downcast_ref::<Holder>()
            .unwrap();
        assert!(Arc::ptr_eq(&held.0, &marker));
    }
    drop(copies);
    drop(err);
    assert_eq!(Arc::strong_count(&marker), 1);
}

#[test]
fn try_bean_keeps_source_and_rewrites_type_name() {
    struct Pool;
    let err = ContainerBuilder::new()
        .try_bean(|| -> Result<Pool, DiError> { Err(DiError::creation_from::<MyErr>(MyErr)) })
        .build()
        .err()
        .unwrap();
    assert!(
        matches!(&err, DiError::Creation { type_name: t, message, .. }
            if t == type_name::<Pool>() && message == "my error"),
        "{err:?}"
    );
    assert!(source_of(&err).is_some_and(ErrorSource::is_some));
    assert!(find_my_err(&err).is_some());

    // Another variant becomes the fixed text with no source.
    let err = ContainerBuilder::new()
        .try_bean(|| -> Result<Pool, DiError> { Err(DiError::ShutDown) })
        .build()
        .err()
        .unwrap();
    assert!(source_of(&err).is_some_and(|s| !s.is_some()));
    assert!(Error::source(&err).is_none());
}

struct Settings;
struct Conn;

#[configuration]
impl Settings {
    #[bean(fallible)]
    fn conn(&self) -> Result<Conn, DiError> {
        Err(DiError::creation_from::<MyErr>(MyErr))
    }
}

#[test]
fn bean_fallible_keeps_source() {
    let err = ContainerBuilder::new()
        .instance(Settings)
        .configuration::<Settings>()
        .build()
        .err()
        .unwrap();
    assert!(
        matches!(&err, DiError::Creation { type_name: t, .. } if t == type_name::<Conn>()),
        "{err:?}"
    );
    assert!(find_my_err(&err).is_some());
}

#[test]
fn post_construct_error_keeps_source() {
    struct Hooked;
    let err = ContainerBuilder::new()
        .instance(Hooked)
        .post_construct(|_: &Hooked| Err(DiError::creation_from::<MyErr>(MyErr)))
        .build()
        .err()
        .unwrap();
    assert!(
        matches!(&err, DiError::Creation { type_name: t, .. } if t == type_name::<Hooked>()),
        "{err:?}"
    );
    assert!(find_my_err(&err).is_some());
}

/// The source must not grow `DiError`: every `Result<_, DiError>` frame on the recursive
/// creation path pays for its size (56 bytes before the source was added).
#[cfg(target_pointer_width = "64")]
#[test]
fn source_does_not_grow_di_error() {
    assert!(
        std::mem::size_of::<DiError>() <= 56,
        "DiError is {} bytes",
        std::mem::size_of::<DiError>()
    );
    assert_eq!(std::mem::size_of::<ErrorSource>(), 8);
}
