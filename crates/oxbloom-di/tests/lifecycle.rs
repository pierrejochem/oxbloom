//! Lifecycle hooks (`post_construct`, `pre_destroy`, the `Lifecycle` trait) and
//! `Container::shutdown`.

use std::any::type_name;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oxbloom_di::{
    ContainerBuilder, DestroyFailure, DiError, ErrorSource, Lifecycle, Resolver, ShutdownReport,
};

/// An event log shared by providers and hooks.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, event: impl Into<String>) {
        self.0.lock().unwrap().push(event.into());
    }

    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    /// Only the `destroy ..` events, in order.
    fn destroyed(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter(|e| e.starts_with("destroy"))
            .collect()
    }
}

struct A;
struct B;
struct C;
struct D;
struct E;
struct F;
struct G;
struct S;

/// Adds a `factory` for `T` that first requests `Dep` (when given), then logs `new <name>`.
macro_rules! bean {
    ($b:expr, $log:expr, $t:ident) => {{
        let log = $log.clone();
        $b.factory(move |_: &Resolver| {
            log.push(concat!("new ", stringify!($t)));
            Ok($t)
        })
    }};
    ($b:expr, $log:expr, $t:ident needs $($dep:ident),+) => {{
        let log = $log.clone();
        $b.factory(move |r: &Resolver| {
            $( r.get::<$dep>()?; )+
            log.push(concat!("new ", stringify!($t)));
            Ok($t)
        })
    }};
}

/// Adds logging `post_construct` and `pre_destroy` hooks for `T` (the last registration).
macro_rules! hooks {
    ($b:expr, $log:expr, $t:ident) => {{
        let (pc, pd) = ($log.clone(), $log.clone());
        $b.post_construct(move |_: &$t| {
            pc.push(concat!("pc ", stringify!($t)));
            Ok(())
        })
        .pre_destroy(move |_: &$t| pd.push(concat!("destroy ", stringify!($t))))
    }};
}

fn strings(events: &[&str]) -> Vec<String> {
    events.iter().map(|e| (*e).to_owned()).collect()
}

#[test]
fn post_construct_runs_after_dependencies_and_before_dependents() {
    let log = Log::default();
    let b = ContainerBuilder::new();
    let b = hooks!(bean!(b, log, A needs B), log, A);
    let b = hooks!(bean!(b, log, B needs C), log, B);
    let b = hooks!(bean!(b, log, C), log, C);
    b.build().unwrap();
    assert_eq!(
        log.events(),
        strings(&["new C", "pc C", "new B", "pc B", "new A", "pc A"])
    );
}

#[test]
fn failing_post_construct_aborts_build_with_creation_naming_type() {
    let log = Log::default();
    let b = bean!(ContainerBuilder::new(), log, A needs B);
    let b = bean!(b, log, B).post_construct(|_: &B| Err(DiError::creation::<B>("not ready")));
    let err = b.build().err();
    assert_eq!(
        err,
        Some(DiError::Creation {
            type_name: type_name::<B>().to_owned(),
            message: "not ready".to_owned(),
            source: ErrorSource::none(),
        })
    );
    // A never received B.
    assert_eq!(log.events(), strings(&["new B"]));

    // A hook error that is not `Creation` becomes `Creation` for the hook's type.
    let err = bean!(ContainerBuilder::new(), log, C)
        .post_construct(|_: &C| Err(DiError::ShutDown))
        .build()
        .err();
    assert!(
        matches!(err, Some(DiError::Creation { ref type_name, ref message, .. })
            if type_name == std::any::type_name::<C>() && message.contains("post_construct")),
        "{err:?}"
    );
}

#[test]
fn failed_post_construct_is_cached_and_runs_once() {
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&runs);
    let err = ContainerBuilder::new()
        // A swallows B's failure; the build loop then reaches B and gets the cached error.
        .factory(|r: &Resolver| {
            let _ = r.get::<B>();
            Ok(A)
        })
        .factory(|_| Ok(B))
        .post_construct(move |_: &B| {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(DiError::creation::<B>("not ready"))
        })
        .build()
        .err();
    assert!(matches!(err, Some(DiError::Creation { .. })), "{err:?}");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_build_destroys_built_beans_in_reverse_order() {
    let log = Log::default();
    let b = ContainerBuilder::new();
    let b = hooks!(bean!(b, log, B needs C), log, B);
    let b = hooks!(bean!(b, log, C), log, C);
    let b = hooks!(bean!(b, log, D), log, D);
    // F is created, but its post_construct fails: its pre_destroy must not run.
    let b = bean!(b, log, F)
        .post_construct(|_: &F| Err(DiError::creation::<F>("boom")))
        .pre_destroy({
            let log = log.clone();
            move |_: &F| log.push("destroy F")
        });
    // G comes after the failure and is never created.
    let b = hooks!(bean!(b, log, G), log, G);
    let err = b.build().err();
    assert!(matches!(err, Some(DiError::Creation { .. })), "{err:?}");
    assert_eq!(
        log.destroyed(),
        strings(&["destroy D", "destroy B", "destroy C"])
    );
    assert!(!log.events().contains(&"new G".to_owned()));
}

#[test]
fn failed_build_cleanup_includes_earlier_created_lazy_free_beans_only_once() {
    // Creation order: S (by A), A, E (by F); F fails its post_construct while S is shared by A
    // and F. Cleanup runs once per created bean, in reverse creation order.
    let log = Log::default();
    let b = ContainerBuilder::new();
    let b = hooks!(bean!(b, log, A needs S), log, A);
    let b = hooks!(bean!(b, log, S), log, S);
    let b = bean!(b, log, F needs S, E).post_construct(|_: &F| Err(DiError::creation::<F>("x")));
    let b = hooks!(bean!(b, log, E), log, E);
    let err = b.build().err();
    assert!(matches!(err, Some(DiError::Creation { .. })), "{err:?}");
    assert_eq!(
        log.events(),
        strings(&[
            "new S",
            "pc S",
            "new A",
            "pc A",
            "new E",
            "pc E",
            "new F",
            "destroy E",
            "destroy A",
            "destroy S",
        ])
    );
}

#[test]
fn failed_build_cleanup_catches_panicking_pre_destroy() {
    let log = Log::default();
    let b = hooks!(bean!(ContainerBuilder::new(), log, A), log, A);
    let b = bean!(b, log, B).pre_destroy(|_: &B| panic!("pre_destroy of B"));
    let b = bean!(b, log, C).post_construct(|_: &C| Err(DiError::creation::<C>("x")));
    let err = b.build().err();
    assert!(matches!(err, Some(DiError::Creation { .. })), "{err:?}");
    assert_eq!(log.destroyed(), strings(&["destroy A"]));
}

#[test]
fn shutdown_destroys_in_reverse_creation_order() {
    let log = Log::default();
    let b = ContainerBuilder::new();
    let b = hooks!(bean!(b, log, D), log, D);
    let b = hooks!(bean!(b, log, A needs B), log, A);
    let b = hooks!(bean!(b, log, B needs C), log, B);
    let b = hooks!(bean!(b, log, C), log, C);
    // A bean without hooks is not counted.
    let c = b.instance(7u32).build().unwrap();
    assert!(log.destroyed().is_empty());
    let report = c.shutdown();
    assert_eq!(
        log.destroyed(),
        strings(&["destroy A", "destroy B", "destroy C", "destroy D"])
    );
    assert_eq!(report.destroyed, 4);
    assert!(report.failures.is_empty());
}

#[test]
fn shutdown_is_idempotent() {
    let log = Log::default();
    let c = hooks!(bean!(ContainerBuilder::new(), log, A), log, A)
        .build()
        .unwrap();
    assert_eq!(c.shutdown().destroyed, 1);
    let again = c.shutdown();
    assert_eq!(again, ShutdownReport::default());
    assert_eq!(log.destroyed(), strings(&["destroy A"]));
}

#[test]
fn panicking_pre_destroy_is_caught_and_reported_and_shutdown_continues() {
    let log = Log::default();
    let b = hooks!(bean!(ContainerBuilder::new(), log, A), log, A);
    let b = bean!(b, log, B).pre_destroy(|_: &B| panic!("secret value 42"));
    let b = hooks!(bean!(b, log, C), log, C);
    let report = b.build().unwrap().shutdown();
    assert_eq!(log.destroyed(), strings(&["destroy C", "destroy A"]));
    assert_eq!(report.destroyed, 2);
    assert_eq!(report.failures.len(), 1);
    let failure: &DestroyFailure = &report.failures[0];
    assert_eq!(failure.type_name, type_name::<B>());
    assert_eq!(failure.message, "pre_destroy panicked");
}

trait Named: Send + Sync {
    fn name(&self) -> &'static str;
}
impl Named for A {
    fn name(&self) -> &'static str {
        "a"
    }
}

#[test]
fn get_after_shutdown_is_shut_down_but_handles_still_work() {
    let c = ContainerBuilder::new()
        .instance(A)
        .provides(|a: Arc<A>| -> Arc<dyn Named> { a })
        .pre_destroy(|_: &A| {})
        .build()
        .unwrap();
    let handle = c.get::<A>().unwrap();
    let named = c.get::<dyn Named>().unwrap();
    c.shutdown();
    assert_eq!(c.get::<A>().err(), Some(DiError::ShutDown));
    assert_eq!(c.get::<dyn Named>().err(), Some(DiError::ShutDown));
    assert_eq!(c.get::<u8>().err(), Some(DiError::ShutDown));
    assert_eq!(c.get_opt::<A>().err(), Some(DiError::ShutDown));
    assert_eq!(c.get_opt::<u8>().err(), Some(DiError::ShutDown));
    assert_eq!(c.get_all::<dyn Named>().err(), Some(DiError::ShutDown));
    assert_eq!(c.get_all::<u8>().err(), Some(DiError::ShutDown));
    assert_eq!(named.name(), "a");
    assert_eq!(handle.name(), "a");
}

#[test]
fn no_destroy_in_drop() {
    let log = Log::default();
    let c = hooks!(bean!(ContainerBuilder::new(), log, A), log, A)
        .build()
        .unwrap();
    drop(c);
    assert!(log.destroyed().is_empty());
}

struct Pool {
    log: Log,
}

impl Lifecycle for Pool {
    fn post_construct(&self) -> Result<(), DiError> {
        self.log.push("pool open");
        Ok(())
    }
    fn pre_destroy(&self) {
        self.log.push("destroy pool");
    }
}

struct Quiet;
impl Lifecycle for Quiet {}

#[test]
fn lifecycle_trait_registers_both_hooks() {
    let log = Log::default();
    let l = log.clone();
    let c = ContainerBuilder::new()
        .factory(move |_| Ok(Pool { log: l.clone() }))
        .lifecycle::<Pool>()
        .instance(Quiet)
        .lifecycle::<Quiet>()
        .build()
        .unwrap();
    assert_eq!(log.events(), strings(&["pool open"]));
    let report = c.shutdown();
    assert_eq!(log.events(), strings(&["pool open", "destroy pool"]));
    assert_eq!(report.destroyed, 2); // Quiet's default pre_destroy counts
}

fn invalid_message(b: ContainerBuilder) -> String {
    match b.build().err() {
        Some(DiError::Invalid { message }) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn hook_modifiers_with_wrong_type_are_invalid() {
    let m = invalid_message(
        ContainerBuilder::new()
            .instance(1u8)
            .post_construct(|_: &u16| Ok(())),
    );
    assert!(
        m.contains("post_construct") && m.contains("u16") && m.contains("u8"),
        "{m}"
    );
    let m = invalid_message(
        ContainerBuilder::new()
            .instance(1u8)
            .pre_destroy(|_: &u16| {}),
    );
    assert!(m.contains("pre_destroy") && m.contains("u16"), "{m}");
    let m = invalid_message(ContainerBuilder::new().instance(1u8).lifecycle::<Quiet>());
    assert!(m.contains("lifecycle") && m.contains("Quiet"), "{m}");

    let m = invalid_message(ContainerBuilder::new().post_construct(|_: &u8| Ok(())));
    assert!(
        m.contains("`post_construct` must follow a registration"),
        "{m}"
    );
    let m = invalid_message(ContainerBuilder::new().pre_destroy(|_: &u8| {}));
    assert!(
        m.contains("`pre_destroy` must follow a registration"),
        "{m}"
    );
    let m = invalid_message(ContainerBuilder::new().lifecycle::<Quiet>());
    assert!(m.contains("`lifecycle` must follow a registration"), "{m}");
}

#[test]
fn several_hooks_on_one_registration_run_in_order() {
    let log = Log::default();
    let (l1, l2, l3, l4) = (log.clone(), log.clone(), log.clone(), log.clone());
    let c = ContainerBuilder::new()
        .instance(A)
        .post_construct(move |_: &A| {
            l1.push("pc 1");
            Ok(())
        })
        .post_construct(move |_: &A| {
            l2.push("pc 2");
            Ok(())
        })
        .pre_destroy(move |_: &A| l3.push("destroy 1"))
        .pre_destroy(move |_: &A| l4.push("destroy 2"))
        .build()
        .unwrap();
    assert_eq!(c.shutdown().destroyed, 1);
    assert_eq!(
        log.events(),
        strings(&["pc 1", "pc 2", "destroy 1", "destroy 2"])
    );
}

#[test]
fn post_construct_error_naming_another_type_is_reported_for_the_failing_bean() {
    // The hook of A returns a `Creation` naming B: the error names A, the bean whose
    // creation failed, and keeps the hook's message.
    let err = ContainerBuilder::new()
        .instance(A)
        .post_construct(|_: &A| Err(DiError::creation::<B>("B is not ready")))
        .build()
        .err();
    assert_eq!(
        err,
        Some(DiError::Creation {
            type_name: type_name::<A>().to_owned(),
            message: "B is not ready".to_owned(),
            source: ErrorSource::none(),
        })
    );
}

/// A container reachable from its own hooks.
type Slot = Arc<std::sync::OnceLock<Arc<oxbloom_di::Container>>>;

#[test]
fn pre_destroy_calling_get_sees_shut_down() {
    let slot: Slot = Arc::default();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (s, out) = (Arc::clone(&slot), Arc::clone(&seen));
    let c = Arc::new(
        ContainerBuilder::new()
            .instance(B)
            .lazy()
            .instance(A)
            .pre_destroy(move |_: &A| {
                let c = s.get().unwrap();
                let mut out = out.lock().unwrap();
                out.push(c.get::<A>().err());
                out.push(c.get::<B>().err()); // lazy, never created
            })
            .build()
            .unwrap(),
    );
    assert!(slot.set(Arc::clone(&c)).is_ok());
    assert_eq!(c.shutdown().destroyed, 1);
    assert_eq!(
        *seen.lock().unwrap(),
        [Some(DiError::ShutDown), Some(DiError::ShutDown)]
    );
}

#[test]
fn pre_destroy_calling_shutdown_does_not_deadlock() {
    let slot: Slot = Arc::default();
    let inner = Arc::new(Mutex::new(None));
    let (s, out) = (Arc::clone(&slot), Arc::clone(&inner));
    let log = Log::default();
    let b = ContainerBuilder::new()
        .instance(A)
        .pre_destroy(move |_: &A| {
            *out.lock().unwrap() = Some(s.get().unwrap().shutdown());
        });
    let c = Arc::new(hooks!(bean!(b, log, B), log, B).build().unwrap());
    assert!(slot.set(Arc::clone(&c)).is_ok());
    let (tx, rx) = std::sync::mpsc::channel();
    let closing = Arc::clone(&c);
    std::thread::spawn(move || {
        let _ = tx.send(closing.shutdown());
    });
    let report = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("deadlock: shutdown called from pre_destroy did not return");
    assert_eq!(report.destroyed, 2);
    // The nested call found the container already shut down.
    assert_eq!(*inner.lock().unwrap(), Some(ShutdownReport::default()));
    assert_eq!(log.destroyed(), strings(&["destroy B"]));
}

#[test]
fn failed_build_cleanup_destroys_an_earlier_created_lazy_bean_at_its_position() {
    // S and T are lazy. A forces S early; T is never requested. E is created by F, whose
    // post_construct then fails. Cleanup: reverse creation order, S at its real position.
    let log = Log::default();
    let b = ContainerBuilder::new();
    let b = hooks!(bean!(b, log, A needs S), log, A);
    let b = hooks!(bean!(b, log, S).lazy(), log, S);
    let b = hooks!(bean!(b, log, G).lazy(), log, G);
    let b = hooks!(bean!(b, log, D), log, D);
    let b = bean!(b, log, F needs S, E).post_construct(|_: &F| Err(DiError::creation::<F>("x")));
    let b = hooks!(bean!(b, log, E), log, E);
    let err = b.build().err();
    assert!(matches!(err, Some(DiError::Creation { .. })), "{err:?}");
    assert_eq!(
        log.events(),
        strings(&[
            "new S",
            "pc S",
            "new A",
            "pc A",
            "new D",
            "pc D",
            "new E",
            "pc E",
            "new F",
            "destroy E",
            "destroy D",
            "destroy A",
            "destroy S",
        ])
    );
}
