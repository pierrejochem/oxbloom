//! Lazy beans: created on first request, once, under the container-wide reentrant creation
//! lock that `shutdown` shares.
//!
//! The concurrency tests run under [`within_5s`], a watchdog that fails the test instead of
//! letting a deadlock hang the suite.

use std::any::type_name;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use oxbloom_di::{Container, ContainerBuilder, DiError, ErrorSource, Resolver, ShutdownReport};

/// Runs `f` on its own thread and fails the test when it does not finish within 5 s.
fn within_5s<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(catch_unwind(AssertUnwindSafe(f)));
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(value)) => value,
        Ok(Err(panic)) => resume_unwind(panic),
        Err(_) => panic!("deadlock: the test did not finish within 5 s"),
    }
}

/// Counts provider runs.
#[derive(Clone, Default)]
struct Counter(Arc<AtomicUsize>);

impl Counter {
    fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

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
}

fn strings(events: &[&str]) -> Vec<String> {
    events.iter().map(|e| (*e).to_owned()).collect()
}

struct A;
struct B;
struct L;
struct M;
struct E;
struct P;
struct Q;
struct X;
struct Y;

trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
}
impl Plugin for A {
    fn name(&self) -> &'static str {
        "a"
    }
}
impl Plugin for L {
    fn name(&self) -> &'static str {
        "l"
    }
}
impl Plugin for M {
    fn name(&self) -> &'static str {
        "m"
    }
}

#[test]
fn container_stays_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Container>();
}

#[test]
fn lazy_bean_not_created_at_build() {
    let runs = Counter::default();
    let r = runs.clone();
    let c = ContainerBuilder::new()
        .factory(move |_: &Resolver| {
            r.bump();
            Ok(L)
        })
        .lazy()
        .build()
        .unwrap();
    assert_eq!(runs.get(), 0);
    let first = c.get::<L>().unwrap();
    let second = c.get::<L>().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(runs.get(), 1);
}

#[test]
fn lazy_modifier_before_registration_is_invalid() {
    let err = ContainerBuilder::new().lazy().build().err();
    assert_eq!(
        err,
        Some(DiError::Invalid {
            message: "`lazy` must follow a registration".to_owned()
        })
    );
}

#[test]
fn lazy_bean_created_once_under_concurrent_first_access() {
    within_5s(|| {
        let runs = Counter::default();
        let r = runs.clone();
        let c = ContainerBuilder::new()
            .factory(move |_: &Resolver| {
                r.bump();
                // Keep the creation in flight while the other threads arrive.
                thread::sleep(Duration::from_millis(20));
                Ok(L)
            })
            .lazy()
            .build()
            .unwrap();
        let start = Barrier::new(16);
        let handles: Vec<Arc<L>> = thread::scope(|s| {
            let spawned: Vec<_> = (0..16)
                .map(|_| {
                    s.spawn(|| {
                        start.wait();
                        c.get::<L>().unwrap()
                    })
                })
                .collect();
            spawned.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(runs.get(), 1);
        assert!(handles.iter().all(|h| Arc::ptr_eq(h, &handles[0])));
    });
}

#[test]
fn eager_dependent_forces_lazy_creation_at_build() {
    let log = Log::default();
    let (l1, l2) = (log.clone(), log.clone());
    let c = ContainerBuilder::new()
        .factory(move |_: &Resolver| {
            l1.push("new L");
            Ok(L)
        })
        .lazy()
        .factory(move |r: &Resolver| {
            r.get::<L>()?;
            l2.push("new E");
            Ok(E)
        })
        .build()
        .unwrap();
    assert_eq!(log.events(), strings(&["new L", "new E"]));
    c.get::<L>().unwrap();
    assert_eq!(log.events(), strings(&["new L", "new E"]));
}

#[test]
fn lazy_failure_appears_at_first_use_and_is_cached() {
    let runs = Counter::default();
    let r = runs.clone();
    let c = ContainerBuilder::new()
        .factory(move |_: &Resolver| -> Result<L, DiError> {
            r.bump();
            Err(DiError::creation::<L>("no connection"))
        })
        .lazy()
        // A lazy bean's missing dependency is reported at first use, too.
        .factory(|r: &Resolver| {
            r.get::<B>()?;
            Ok(M)
        })
        .lazy()
        // So is a failing post_construct.
        .instance(A)
        .lazy()
        .post_construct(|_: &A| Err(DiError::creation::<A>("not ready")))
        .build()
        .unwrap();
    assert_eq!(runs.get(), 0);
    let expected = DiError::Creation {
        type_name: type_name::<L>().to_owned(),
        message: "no connection".to_owned(),
        source: ErrorSource::none(),
    };
    assert_eq!(c.get::<L>().err(), Some(expected.clone()));
    assert_eq!(c.get::<L>().err(), Some(expected));
    assert_eq!(runs.get(), 1);

    let missing = DiError::Missing {
        requested: type_name::<B>().to_owned(),
        required_by: vec![type_name::<M>().to_owned()],
    };
    assert_eq!(c.get::<M>().err(), Some(missing.clone()));
    assert_eq!(c.get::<M>().err(), Some(missing));
    assert!(matches!(
        c.get::<A>(),
        Err(DiError::Creation { ref message, .. }) if message == "not ready"
    ));
}

#[test]
fn option_of_failing_lazy_bean_is_an_error_not_none() {
    let c = ContainerBuilder::new()
        .factory(|_: &Resolver| -> Result<L, DiError> { Err(DiError::creation::<L>("down")) })
        .lazy()
        // Injection: a lazy consumer asking for `Option<Arc<L>>`.
        .factory(|r: &Resolver| {
            r.get_opt::<L>()?;
            Ok(M)
        })
        .lazy()
        .build()
        .unwrap();
    assert!(matches!(c.get_opt::<L>(), Err(DiError::Creation { .. })));
    assert!(matches!(c.get::<M>(), Err(DiError::Creation { .. })));

    // An eager consumer forces the lazy bean at build, and the build fails.
    let err = ContainerBuilder::new()
        .factory(|_: &Resolver| -> Result<L, DiError> { Err(DiError::creation::<L>("down")) })
        .lazy()
        .factory(|r: &Resolver| {
            r.get_opt::<L>()?;
            Ok(M)
        })
        .build()
        .err();
    assert!(matches!(err, Some(DiError::Creation { .. })), "{err:?}");
}

#[test]
fn cycle_through_lazy_beans_is_cycle() {
    let c = ContainerBuilder::new()
        .factory(|r: &Resolver| {
            r.get::<B>()?;
            Ok(A)
        })
        .lazy()
        .factory(|r: &Resolver| {
            r.get::<A>()?;
            Ok(B)
        })
        .lazy()
        .build()
        .unwrap();
    let cycle = DiError::Cycle {
        path: vec![
            type_name::<A>().to_owned(),
            type_name::<B>().to_owned(),
            type_name::<A>().to_owned(),
        ],
    };
    assert_eq!(c.get::<A>().err(), Some(cycle.clone()));
    // Not cached: the next request walks the cycle again.
    assert_eq!(c.get::<A>().err(), Some(cycle));

    // An eager bean reaching the cycle fails the build.
    let err = ContainerBuilder::new()
        .factory(|r: &Resolver| {
            r.get::<B>()?;
            Ok(A)
        })
        .factory(|r: &Resolver| {
            r.get::<A>()?;
            Ok(B)
        })
        .lazy()
        .build()
        .err();
    assert!(matches!(err, Some(DiError::Cycle { .. })), "{err:?}");
}

/// A container whose provider calls back into the container itself (`Container::get`) from a
/// lazy provider: the creation lock is reentrant and the cycle check spans both requests.
#[test]
fn lazy_provider_may_call_back_into_the_container() {
    within_5s(|| {
        let slot: Arc<OnceLock<Arc<Container>>> = Arc::new(OnceLock::new());
        let (s1, s2) = (Arc::clone(&slot), Arc::clone(&slot));
        let c = Arc::new(
            ContainerBuilder::new()
                .factory(move |_: &Resolver| {
                    s1.get().unwrap().get::<B>()?;
                    Ok(A)
                })
                .lazy()
                .instance(B)
                .lazy()
                .factory(move |_: &Resolver| {
                    s2.get().unwrap().get::<X>()?;
                    Ok(X)
                })
                .lazy()
                .build()
                .unwrap(),
        );
        assert!(slot.set(Arc::clone(&c)).is_ok());
        assert!(c.get::<A>().is_ok());
        assert_eq!(
            c.get::<X>().err(),
            Some(DiError::Cycle {
                path: vec![type_name::<X>().to_owned(), type_name::<X>().to_owned()]
            })
        );
    });
}

#[test]
fn two_threads_with_crossing_lazy_chains_do_not_deadlock() {
    within_5s(|| {
        for _ in 0..20 {
            let (p_runs, q_runs) = (Counter::default(), Counter::default());
            let (p, q) = (p_runs.clone(), q_runs.clone());
            // X needs P then Q; Y needs Q then P. Per-bean locks taken in chain order would
            // deadlock here; the container-wide lock serializes the chains instead.
            let c = ContainerBuilder::new()
                .factory(|r: &Resolver| {
                    r.get::<P>()?;
                    r.get::<Q>()?;
                    Ok(X)
                })
                .lazy()
                .factory(|r: &Resolver| {
                    r.get::<Q>()?;
                    r.get::<P>()?;
                    Ok(Y)
                })
                .lazy()
                .factory(move |_: &Resolver| {
                    p.bump();
                    thread::sleep(Duration::from_millis(2));
                    Ok(P)
                })
                .lazy()
                .factory(move |_: &Resolver| {
                    q.bump();
                    thread::sleep(Duration::from_millis(2));
                    Ok(Q)
                })
                .lazy()
                .build()
                .unwrap();
            let start = Barrier::new(2);
            thread::scope(|s| {
                let x = s.spawn(|| {
                    start.wait();
                    c.get::<X>().map(drop)
                });
                let y = s.spawn(|| {
                    start.wait();
                    c.get::<Y>().map(drop)
                });
                assert_eq!(x.join().unwrap(), Ok(()));
                assert_eq!(y.join().unwrap(), Ok(()));
            });
            assert_eq!((p_runs.get(), q_runs.get()), (1, 1));
        }
    });
}

#[test]
fn shutdown_waits_for_inflight_lazy_creation_then_destroys_it() {
    within_5s(|| {
        let log = Log::default();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let (l1, l2) = (log.clone(), log.clone());
        let c = ContainerBuilder::new()
            .factory(move |_: &Resolver| {
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
                l1.push("new L");
                Ok(L)
            })
            .lazy()
            .pre_destroy(move |_: &L| l2.push("destroy L"))
            .build()
            .unwrap();
        let shut_down = AtomicBool::new(false);
        thread::scope(|s| {
            let creating = s.spawn(|| c.get::<L>().map(drop));
            started_rx.recv().unwrap();
            let closing = s.spawn(|| {
                let report = c.shutdown();
                shut_down.store(true, Ordering::SeqCst);
                report
            });
            // `shutdown` waits for the creation in flight.
            thread::sleep(Duration::from_millis(100));
            assert!(!shut_down.load(Ordering::SeqCst));
            release_tx.send(()).unwrap();
            assert_eq!(creating.join().unwrap(), Ok(()));
            let report = closing.join().unwrap();
            assert_eq!(report.destroyed, 1);
            assert!(report.failures.is_empty());
        });
        assert_eq!(log.events(), strings(&["new L", "destroy L"]));
        assert_eq!(c.get::<L>().err(), Some(DiError::ShutDown));
    });
}

#[test]
fn lazy_bean_destroyed_only_if_created() {
    let log = Log::default();
    let destroy = |name: &'static str| {
        let log = log.clone();
        move || log.push(format!("destroy {name}"))
    };
    let (da, de, dl, dm) = (destroy("A"), destroy("E"), destroy("L"), destroy("M"));
    let c = ContainerBuilder::new()
        .instance(L)
        .lazy()
        .pre_destroy(move |_: &L| dl())
        .instance(A)
        .pre_destroy(move |_: &A| da())
        .instance(M)
        .lazy()
        .pre_destroy(move |_: &M| dm())
        .instance(E)
        .pre_destroy(move |_: &E| de())
        .build()
        .unwrap();
    // L is created after A and E: it is destroyed first, at its real position. M never is.
    c.get::<L>().unwrap();
    let report = c.shutdown();
    assert_eq!(report.destroyed, 3);
    assert_eq!(
        log.events(),
        strings(&["destroy L", "destroy E", "destroy A"])
    );
}

#[test]
fn get_after_shutdown_creates_no_lazy_bean() {
    let runs = Counter::default();
    let r = runs.clone();
    let c = ContainerBuilder::new()
        .factory(move |_: &Resolver| {
            r.bump();
            Ok(L)
        })
        .lazy()
        .provides(|l: Arc<L>| -> Arc<dyn Plugin> { l })
        .build()
        .unwrap();
    c.shutdown();
    assert_eq!(c.get::<L>().err(), Some(DiError::ShutDown));
    assert_eq!(c.get_opt::<L>().err(), Some(DiError::ShutDown));
    assert_eq!(c.get_all::<dyn Plugin>().err(), Some(DiError::ShutDown));
    assert_eq!(runs.get(), 0);
}

#[test]
fn get_all_includes_lazy_providers_creating_them() {
    let runs = Counter::default();
    let (r1, r2) = (runs.clone(), runs.clone());
    let c = ContainerBuilder::new()
        .instance(A)
        .provides(|a: Arc<A>| -> Arc<dyn Plugin> { a })
        .factory(move |_: &Resolver| {
            r1.bump();
            Ok(L)
        })
        .lazy()
        .provides(|l: Arc<L>| -> Arc<dyn Plugin> { l })
        .order(-1)
        .factory(move |_: &Resolver| {
            r2.bump();
            Ok(M)
        })
        .lazy()
        .provides(|m: Arc<M>| -> Arc<dyn Plugin> { m })
        .build()
        .unwrap();
    assert_eq!(runs.get(), 0);
    let names: Vec<_> = c
        .get_all::<dyn Plugin>()
        .unwrap()
        .iter()
        .map(|p| p.name())
        .collect();
    assert_eq!(names, ["l", "a", "m"]);
    assert_eq!(runs.get(), 2);
    // The same instances as a single request.
    let all = c.get_all::<dyn Plugin>().unwrap();
    assert!(Arc::ptr_eq(&all[0], &c.get_all::<dyn Plugin>().unwrap()[0]));
    assert_eq!(runs.get(), 2);

    // A failing lazy provider fails the collection.
    let c = ContainerBuilder::new()
        .instance(A)
        .provides(|a: Arc<A>| -> Arc<dyn Plugin> { a })
        .factory(|_: &Resolver| -> Result<L, DiError> { Err(DiError::creation::<L>("down")) })
        .lazy()
        .provides(|l: Arc<L>| -> Arc<dyn Plugin> { l })
        .build()
        .unwrap();
    assert!(matches!(
        c.get_all::<dyn Plugin>(),
        Err(DiError::Creation { .. })
    ));
}

#[test]
fn panicking_lazy_provider_leaves_the_container_usable() {
    let fail = Arc::new(AtomicBool::new(true));
    let f = Arc::clone(&fail);
    let c = ContainerBuilder::new()
        .factory(move |r: &Resolver| {
            r.get::<B>()?;
            if f.load(Ordering::SeqCst) {
                panic!("first attempt");
            }
            Ok(A)
        })
        .lazy()
        .instance(B)
        .lazy()
        .build()
        .unwrap();
    // (The panic message is printed to stderr; that is expected.)
    let caught = catch_unwind(AssertUnwindSafe(|| c.get::<A>()));
    assert!(caught.is_err());
    // The panic is not cached and the aborted chain left no trace: no lock held, no stale
    // stack entry (which would look like a cycle).
    fail.store(false, Ordering::SeqCst);
    within_5s(move || assert!(c.get::<A>().is_ok()));
}

#[test]
fn providers_of_settled_beans_are_dropped_after_build() {
    let eager_capture = Arc::new(());
    let lazy_capture = Arc::new(());
    let (e, l) = (Arc::clone(&eager_capture), Arc::clone(&lazy_capture));
    let c = ContainerBuilder::new()
        .instance(A)
        .factory(move |_: &Resolver| {
            let _ = &e;
            Ok(E)
        })
        .factory(move |_: &Resolver| {
            let _ = &l;
            Ok(L)
        })
        .lazy()
        .build()
        .unwrap();
    // `instance` keeps no extra handle: the container's and ours.
    assert_eq!(Arc::strong_count(&c.get::<A>().unwrap()), 2);
    // The eager factory is gone; the lazy one is kept until it can run.
    assert_eq!(Arc::strong_count(&eager_capture), 1);
    assert_eq!(Arc::strong_count(&lazy_capture), 2);
    c.get::<L>().unwrap();
    assert_eq!(Arc::strong_count(&lazy_capture), 2);
}

/// Shutdown from inside a lazy provider, on the creating thread: the rest of the chain creates
/// nothing, the provider's own instance is not handed out, and nothing created is left
/// undestroyed.
#[test]
fn shutdown_inside_a_lazy_provider_stops_the_chain() {
    within_5s(|| {
        struct Early;
        struct Late;
        struct Outer;
        let slot: Arc<OnceLock<Arc<Container>>> = Arc::new(OnceLock::new());
        let log = Log::default();
        let late_runs = Counter::default();
        let (s, l1, l2, l3, l4, l5) = (
            Arc::clone(&slot),
            log.clone(),
            log.clone(),
            log.clone(),
            log.clone(),
            log.clone(),
        );
        let late = late_runs.clone();
        let c = Arc::new(
            ContainerBuilder::new()
                .factory(move |r: &Resolver| {
                    r.get::<Early>()?;
                    let report = s.get().unwrap().shutdown();
                    l1.push(format!("shutdown destroyed {}", report.destroyed));
                    let late = r.get::<Late>().err();
                    l1.push(format!("late: {late:?}"));
                    Ok(Outer) // the provider ignores the refusal
                })
                .lazy()
                .post_construct(move |_: &Outer| {
                    l2.push("outer constructed");
                    Ok(())
                })
                .pre_destroy(move |_: &Outer| l3.push("outer destroyed"))
                .factory(|_: &Resolver| Ok(Early))
                .lazy()
                .pre_destroy(move |_: &Early| l4.push("early destroyed"))
                .factory(move |_: &Resolver| {
                    late.bump();
                    Ok(Late)
                })
                .lazy()
                .pre_destroy(move |_: &Late| l5.push("late destroyed"))
                .build()
                .unwrap(),
        );
        assert!(slot.set(Arc::clone(&c)).is_ok());

        assert_eq!(c.get::<Outer>().err(), Some(DiError::ShutDown));
        assert_eq!(late_runs.get(), 0);
        assert_eq!(
            log.events(),
            strings(&[
                "early destroyed",
                "shutdown destroyed 1",
                "late: Some(ShutDown)",
            ])
        );
        assert_eq!(c.get::<Outer>().err(), Some(DiError::ShutDown));
        assert_eq!(c.get::<Late>().err(), Some(DiError::ShutDown));
        assert_eq!(c.shutdown(), ShutdownReport::default());
        assert_eq!(late_runs.get(), 0);
    });
}

/// Shutdown from a lazy bean's `post_construct` hook: `shutdown` cannot see the bean yet, so its
/// `pre_destroy` hooks run right after the hook returns, and the bean is not handed out.
#[test]
fn shutdown_inside_post_construct_destroys_the_bean() {
    within_5s(|| {
        struct Early;
        struct Late;
        struct Outer;
        let slot: Arc<OnceLock<Arc<Container>>> = Arc::new(OnceLock::new());
        let log = Log::default();
        let late_runs = Counter::default();
        let (s, l1, l2, l3) = (Arc::clone(&slot), log.clone(), log.clone(), log.clone());
        let late = late_runs.clone();
        let c = Arc::new(
            ContainerBuilder::new()
                .factory(|r: &Resolver| {
                    r.get::<Early>()?;
                    Ok(Outer)
                })
                .lazy()
                .post_construct(move |_: &Outer| {
                    let report = s.get().unwrap().shutdown();
                    l1.push(format!("shutdown destroyed {}", report.destroyed));
                    Ok(())
                })
                .pre_destroy(move |_: &Outer| l2.push("outer destroyed"))
                .instance(Early)
                .lazy()
                .pre_destroy(move |_: &Early| l3.push("early destroyed"))
                .factory(move |_: &Resolver| {
                    late.bump();
                    Ok(Late)
                })
                .lazy()
                .build()
                .unwrap(),
        );
        assert!(slot.set(Arc::clone(&c)).is_ok());

        assert_eq!(c.get::<Outer>().err(), Some(DiError::ShutDown));
        assert_eq!(
            log.events(),
            strings(&["early destroyed", "shutdown destroyed 1", "outer destroyed"])
        );
        assert_eq!(c.get::<Outer>().err(), Some(DiError::ShutDown));
        assert_eq!(c.get::<Late>().err(), Some(DiError::ShutDown));
        assert_eq!(late_runs.get(), 0);
        assert_eq!(c.shutdown(), ShutdownReport::default());
    });
}
