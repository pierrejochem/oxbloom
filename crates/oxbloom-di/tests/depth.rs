//! Dependency depth limit. Creation is recursive, so the resolver caps the depth of its stack at
//! 256 and reports `TooDeep` instead of overflowing the thread stack. The chains below run on a
//! 256 KB thread, so a regression in per-level stack use fails here rather than in production.

// The component chain nests 256 `Arc` fields; the `Send + Sync` check walks all of them.
#![recursion_limit = "1024"]

use std::any::type_name;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use oxbloom_di::{ContainerBuilder, DiError, Resolver, component, configuration};

/// One link of a chain: `Link<N>` depends on `Link<N + 1>` while `N + 1 < shape.len`.
struct Link<const N: usize>;

/// How a generated chain behaves.
#[derive(Clone, Copy)]
struct Shape {
    len: usize,
    /// `Link<0>` ignores the error of its dependency instead of failing.
    swallow_at_top: bool,
    /// The last link also asks for `Deep` and records the outcome in `Seen`.
    probe_deep: bool,
    /// Every link is registered `lazy`.
    lazy: bool,
}

impl Shape {
    fn plain(len: usize) -> Self {
        Shape {
            len,
            swallow_at_top: false,
            probe_deep: false,
            lazy: false,
        }
    }
}

/// Requested from the bottom of a chain, then again from the build loop.
struct Deep;

/// Errors recorded by the chain, shared with the test through a clone.
#[derive(Default, Clone)]
struct Seen(Arc<Mutex<Vec<DiError>>>);

fn link<const N: usize, const NEXT: usize>(b: ContainerBuilder, shape: Shape) -> ContainerBuilder {
    if N >= shape.len {
        return b;
    }
    let len = shape.len;
    let b = if !shape.swallow_at_top && !shape.probe_deep {
        // The plain closure: its frame is the per-level baseline the stack tests measure.
        b.factory(move |r: &Resolver| {
            if NEXT < len {
                r.get::<Link<NEXT>>()?;
            }
            Ok(Link::<N>)
        })
    } else {
        probing_link::<N, NEXT>(b, shape)
    };
    if shape.lazy { b.lazy() } else { b }
}

/// A link that swallows its dependency's error at the top or probes `Deep` at the bottom.
fn probing_link<const N: usize, const NEXT: usize>(
    b: ContainerBuilder,
    shape: Shape,
) -> ContainerBuilder {
    let len = shape.len;
    b.factory(move |r: &Resolver| {
        if NEXT < len {
            let next = r.get::<Link<NEXT>>();
            if !(N == 0 && shape.swallow_at_top) {
                next?;
            }
        } else if shape.probe_deep
            && let Err(e) = r.get::<Deep>()
        {
            r.get::<Seen>()?.0.lock().unwrap().push(e);
        }
        Ok(Link::<N>)
    })
}

/// Registers `Link<0>` .. `Link<299>` (those below `len`) in ascending order, so building
/// `Link<0>` first walks the whole chain depth-first.
macro_rules! links {
    ($b:ident, $len:expr; $($h:literal)*) => {
        $( links!(@tens $b, $len, $h; 0 1 2 3 4 5 6 7 8 9); )*
    };
    (@tens $b:ident, $len:expr, $h:literal; $($t:literal)*) => {
        $( links!(@units $b, $len, $h, $t; 0 1 2 3 4 5 6 7 8 9); )*
    };
    (@units $b:ident, $len:expr, $h:literal, $t:literal; $($u:literal)*) => {
        $( $b = link::<{ $h * 100 + $t * 10 + $u }, { $h * 100 + $t * 10 + $u + 1 }>($b, $len); )*
    };
}

/// How a `.bean` chain is generated: `len` links, each taking `Arc<Link<N + 1>>` and, when
/// `wide`, the three leaves too (four parameters, like the `#[component]` chain below).
#[derive(Clone, Copy)]
struct BeanShape {
    len: usize,
    wide: bool,
}

/// `Link<N>` registered through `.bean`.
fn bean_link<const N: usize, const NEXT: usize>(
    b: ContainerBuilder,
    shape: BeanShape,
) -> ContainerBuilder {
    match (N < shape.len, NEXT < shape.len, shape.wide) {
        (false, _, _) => b,
        (true, true, false) => b.bean(|_: Arc<Link<NEXT>>| Link::<N>),
        (true, true, true) => {
            b.bean(|_: Arc<Link<NEXT>>, _: Arc<LeafA>, _: Arc<LeafB>, _: Arc<LeafC>| Link::<N>)
        }
        (true, false, false) => b.bean(|| Link::<N>),
        (true, false, true) => b.bean(|_: Arc<LeafA>, _: Arc<LeafB>, _: Arc<LeafC>| Link::<N>),
    }
}

macro_rules! bean_links {
    ($b:ident, $len:expr; $($h:literal)*) => {
        $( bean_links!(@tens $b, $len, $h; 0 1 2 3 4 5 6 7 8 9); )*
    };
    (@tens $b:ident, $len:expr, $h:literal; $($t:literal)*) => {
        $( bean_links!(@units $b, $len, $h, $t; 0 1 2 3 4 5 6 7 8 9); )*
    };
    (@units $b:ident, $len:expr, $h:literal, $t:literal; $($u:literal)*) => {
        $( $b = bean_link::<{ $h * 100 + $t * 10 + $u }, { $h * 100 + $t * 10 + $u + 1 }>($b, $len); )*
    };
}

fn bean_chain(len: usize, wide: bool) -> ContainerBuilder {
    assert!(len <= 300, "bean_chain() generates at most 300 links");
    let shape = BeanShape { len, wide };
    let mut b = ContainerBuilder::new()
        .instance(LeafA)
        .instance(LeafB)
        .instance(LeafC);
    bean_links!(b, shape; 0 1 2);
    b
}

fn chain(len: usize) -> ContainerBuilder {
    chain_of(ContainerBuilder::new(), Shape::plain(len))
}

fn chain_of(mut b: ContainerBuilder, shape: Shape) -> ContainerBuilder {
    assert!(shape.len <= 300, "chain_of() generates at most 300 links");
    links!(b, shape; 0 1 2);
    b
}

// --- a chain of `#[component]` structs with four `Arc` fields each ------------------------------

struct LeafA;
struct LeafB;
struct LeafC;

/// `#[component]` struct `$a` needs `$b` and the three leaves; the last one needs only the leaves.
macro_rules! components {
    ([$($a:ident)*] [$($b:ident)*] $last:ident) => {
        $(
            #[component]
            #[allow(dead_code)]
            struct $a {
                next: Arc<$b>,
                a: Arc<LeafA>,
                b: Arc<LeafB>,
                c: Arc<LeafC>,
            }
        )*
        #[component]
        #[allow(dead_code)]
        struct $last {
            a: Arc<LeafA>,
            b: Arc<LeafB>,
            c: Arc<LeafC>,
        }

        /// Registers the leaves and the 256 components, top of the chain first.
        fn component_chain() -> ContainerBuilder {
            ContainerBuilder::new()
                .instance(LeafA)
                .instance(LeafB)
                .instance(LeafC)
                $(.component::<$a>())*
                .component::<$last>()
        }

        /// 256 `#[bean]` methods: `Bean<$a>` needs `Bean<$b>`, the last one nothing.
        #[configuration]
        #[allow(non_snake_case)]
        impl Narrow {
            $(
                #[bean]
                fn $a(&self, _next: Arc<Bean<$b>>) -> Bean<$a> {
                    Bean::new()
                }
            )*
            #[bean]
            fn $last(&self) -> Bean<$last> {
                Bean::new()
            }
        }

        /// 256 `#[bean]` methods that also take the three leaves (four parameters besides
        /// `&self`, like the `#[component]` chain).
        #[configuration]
        #[allow(non_snake_case)]
        impl Wide {
            $(
                #[bean]
                fn $a(
                    &self,
                    _next: Arc<Bean<$b>>,
                    _a: Arc<LeafA>,
                    _b: Arc<LeafB>,
                    _c: Arc<LeafC>,
                ) -> Bean<$a> {
                    Bean::new()
                }
            )*
            #[bean]
            fn $last(&self, _a: Arc<LeafA>, _b: Arc<LeafB>, _c: Arc<LeafC>) -> Bean<$last> {
                Bean::new()
            }
        }
    };
}

// --- a chain of `#[bean]` methods of a `#[configuration]` ---------------------------------------

/// The bean made by the `#[bean]` method named after the component `T`.
struct Bean<T>(PhantomData<fn() -> T>);

impl<T> Bean<T> {
    fn new() -> Self {
        Bean(PhantomData)
    }
}

struct Narrow;
struct Wide;

/// The leaves, the configuration instance and its 256 `#[bean]` methods.
fn configuration_chain(wide: bool) -> ContainerBuilder {
    let b = ContainerBuilder::new()
        .instance(LeafA)
        .instance(LeafB)
        .instance(LeafC);
    if wide {
        b.instance(Wide).configuration::<Wide>()
    } else {
        b.instance(Narrow).configuration::<Narrow>()
    }
}

components!(
    [C000 C001 C002 C003 C004 C005 C006 C007 C008 C009 C010 C011 C012 C013 C014 C015 C016 C017
     C018 C019 C020 C021 C022 C023 C024 C025 C026 C027 C028 C029 C030 C031 C032 C033 C034 C035
     C036 C037 C038 C039 C040 C041 C042 C043 C044 C045 C046 C047 C048 C049 C050 C051 C052 C053
     C054 C055 C056 C057 C058 C059 C060 C061 C062 C063 C064 C065 C066 C067 C068 C069 C070 C071
     C072 C073 C074 C075 C076 C077 C078 C079 C080 C081 C082 C083 C084 C085 C086 C087 C088 C089
     C090 C091 C092 C093 C094 C095 C096 C097 C098 C099 C100 C101 C102 C103 C104 C105 C106 C107
     C108 C109 C110 C111 C112 C113 C114 C115 C116 C117 C118 C119 C120 C121 C122 C123 C124 C125
     C126 C127 C128 C129 C130 C131 C132 C133 C134 C135 C136 C137 C138 C139 C140 C141 C142 C143
     C144 C145 C146 C147 C148 C149 C150 C151 C152 C153 C154 C155 C156 C157 C158 C159 C160 C161
     C162 C163 C164 C165 C166 C167 C168 C169 C170 C171 C172 C173 C174 C175 C176 C177 C178 C179
     C180 C181 C182 C183 C184 C185 C186 C187 C188 C189 C190 C191 C192 C193 C194 C195 C196 C197
     C198 C199 C200 C201 C202 C203 C204 C205 C206 C207 C208 C209 C210 C211 C212 C213 C214 C215
     C216 C217 C218 C219 C220 C221 C222 C223 C224 C225 C226 C227 C228 C229 C230 C231 C232 C233
     C234 C235 C236 C237 C238 C239 C240 C241 C242 C243 C244 C245 C246 C247 C248 C249 C250 C251
     C252 C253 C254]
    [C001 C002 C003 C004 C005 C006 C007 C008 C009 C010 C011 C012 C013 C014 C015 C016 C017 C018
     C019 C020 C021 C022 C023 C024 C025 C026 C027 C028 C029 C030 C031 C032 C033 C034 C035 C036
     C037 C038 C039 C040 C041 C042 C043 C044 C045 C046 C047 C048 C049 C050 C051 C052 C053 C054
     C055 C056 C057 C058 C059 C060 C061 C062 C063 C064 C065 C066 C067 C068 C069 C070 C071 C072
     C073 C074 C075 C076 C077 C078 C079 C080 C081 C082 C083 C084 C085 C086 C087 C088 C089 C090
     C091 C092 C093 C094 C095 C096 C097 C098 C099 C100 C101 C102 C103 C104 C105 C106 C107 C108
     C109 C110 C111 C112 C113 C114 C115 C116 C117 C118 C119 C120 C121 C122 C123 C124 C125 C126
     C127 C128 C129 C130 C131 C132 C133 C134 C135 C136 C137 C138 C139 C140 C141 C142 C143 C144
     C145 C146 C147 C148 C149 C150 C151 C152 C153 C154 C155 C156 C157 C158 C159 C160 C161 C162
     C163 C164 C165 C166 C167 C168 C169 C170 C171 C172 C173 C174 C175 C176 C177 C178 C179 C180
     C181 C182 C183 C184 C185 C186 C187 C188 C189 C190 C191 C192 C193 C194 C195 C196 C197 C198
     C199 C200 C201 C202 C203 C204 C205 C206 C207 C208 C209 C210 C211 C212 C213 C214 C215 C216
     C217 C218 C219 C220 C221 C222 C223 C224 C225 C226 C227 C228 C229 C230 C231 C232 C233 C234
     C235 C236 C237 C238 C239 C240 C241 C242 C243 C244 C245 C246 C247 C248 C249 C250 C251 C252
     C253 C254 C255]
    C255
);

fn on_stack<R: Send + 'static>(kb: usize, f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(kb * 1024)
        .spawn(f)
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked")
}

#[test]
fn deep_chain_257_is_too_deep_on_a_256kb_stack() {
    let err = on_stack(256, || chain(300).build().err());
    let path = vec![
        type_name::<Link<0>>().to_owned(),
        type_name::<Link<1>>().to_owned(),
        type_name::<Link<2>>().to_owned(),
        type_name::<Link<3>>().to_owned(),
        type_name::<Link<4>>().to_owned(),
        type_name::<Link<5>>().to_owned(),
        type_name::<Link<6>>().to_owned(),
        type_name::<Link<7>>().to_owned(),
    ];
    assert_eq!(err, Some(DiError::TooDeep { limit: 256, path }));
}

#[test]
fn chain_of_256_builds() {
    let ok = on_stack(256, || {
        let c = chain(256).build()?;
        c.get::<Link<0>>()?;
        c.get::<Link<255>>()?;
        Ok::<_, DiError>(())
    });
    assert_eq!(ok, Ok(()));
}

#[test]
fn chain_of_257_is_too_deep() {
    let err = on_stack(256, || chain(257).build().err());
    assert!(matches!(err, Some(DiError::TooDeep { limit: 256, ref path }) if path.len() == 8));
}

#[test]
fn too_deep_carries_type_names_only() {
    // Hygiene (see `tests/hygiene.rs`): a value registered next to the chain never shows up.
    const MARKER: &str = "MARKER-7f3a-secret";
    struct Secret(#[allow(dead_code)] String);
    let err = on_stack(256, || {
        let b = ContainerBuilder::new().instance(Secret(MARKER.to_owned()));
        chain_of(b, Shape::plain(257)).build().err()
    })
    .unwrap();
    assert!(matches!(err, DiError::TooDeep { .. }), "{err:?}");
    assert!(!format!("{err}").contains(MARKER));
    assert!(!format!("{err:?}").contains(MARKER));
}

#[test]
fn swallowed_too_deep_error_does_not_fail_the_build() {
    // Link<256> is refused at depth 257; Link<1>..Link<255> fail with TooDeep and Link<0> ignores
    // it. TooDeep is not cached, so the build loop then builds Link<1> from depth 1.
    let shape = Shape {
        len: 257,
        swallow_at_top: true,
        probe_deep: false,
        lazy: false,
    };
    let built = on_stack(8 * 1024, move || {
        let c = chain_of(ContainerBuilder::new(), shape).build()?;
        c.get::<Link<1>>()?;
        c.get::<Link<255>>()?;
        c.get::<Link<256>>()?;
        Ok::<_, DiError>(())
    });
    assert_eq!(built, Ok(()));
}

#[test]
fn entry_reachable_deep_and_shallow_is_built_when_shallow() {
    // Link<255> is at depth 256 and asks for Deep (depth 257): refused. The build loop later asks
    // for Deep from depth 1, which succeeds.
    let seen = Seen::default();
    let recorded = seen.clone();
    let shape = Shape {
        len: 256,
        swallow_at_top: false,
        probe_deep: true,
        lazy: false,
    };
    let built = on_stack(8 * 1024, move || {
        let b = ContainerBuilder::new().instance(seen);
        let c = chain_of(b, shape).factory(|_| Ok(Deep)).build()?;
        c.get::<Deep>().map(drop)
    });
    assert_eq!(built, Ok(()));
    let errors = recorded.0.lock().unwrap();
    assert!(
        matches!(errors.as_slice(), [DiError::TooDeep { limit: 256, .. }]),
        "{errors:?}"
    );
}

#[test]
fn lazy_chain_of_256_is_created_by_the_first_get_on_a_256kb_stack() {
    let lazy = |len| Shape {
        lazy: true,
        ..Shape::plain(len)
    };
    let ok = on_stack(256, move || {
        let c = chain_of(ContainerBuilder::new(), lazy(256)).build()?;
        c.get::<Link<0>>()?;
        c.get::<Link<255>>().map(drop)
    });
    assert_eq!(ok, Ok(()));
    // Too deep is reported at first use, and not cached.
    let err = on_stack(256, move || {
        let c = chain_of(ContainerBuilder::new(), lazy(257))
            .build()
            .unwrap();
        let first = c.get::<Link<0>>().err();
        assert_eq!(c.get::<Link<0>>().err(), first);
        first
    });
    assert!(matches!(err, Some(DiError::TooDeep { limit: 256, ref path }) if path.len() == 8));
}

#[test]
fn component_chain_256_deep_builds_on_a_512kb_stack() {
    let built = on_stack(512, || {
        let c = component_chain().build()?;
        c.get::<C000>()?;
        c.get::<C255>().map(drop)
    });
    assert_eq!(built, Ok(()));
}

#[test]
fn bean_chain_256_deep_builds_on_a_512kb_stack() {
    for wide in [false, true] {
        let built = on_stack(512, move || {
            let c = bean_chain(256, wide).build()?;
            c.get::<Link<0>>()?;
            c.get::<Link<255>>().map(drop)
        });
        assert_eq!(built, Ok(()), "wide: {wide}");
    }
}

#[test]
fn configuration_bean_chain_256_deep_builds_on_a_512kb_stack() {
    for wide in [false, true] {
        let built = on_stack(512, move || {
            let c = configuration_chain(wide).build()?;
            c.get::<Bean<C000>>()?;
            c.get::<Bean<C255>>().map(drop)
        });
        assert_eq!(built, Ok(()), "wide: {wide}");
    }
}

#[test]
fn bean_chain_257_is_too_deep() {
    let err = on_stack(512, || bean_chain(257, false).build().err());
    assert!(matches!(err, Some(DiError::TooDeep { limit: 256, ref path }) if path.len() == 8));
}

/// Manual probe for the smallest stack that survives a 256-deep build:
/// `OXBLOOM_DI_STACK_KB=<kb> cargo test -p oxbloom-di --test depth -- --ignored probe`.
/// A stack that is too small aborts the test process with a stack overflow.
#[test]
#[ignore = "manual stack-size probe; set OXBLOOM_DI_STACK_KB"]
fn probe_stack_for_256_deep_build() {
    let kb: usize = std::env::var("OXBLOOM_DI_STACK_KB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    let built = on_stack(kb, || chain(256).build().map(drop));
    assert_eq!(built, Ok(()));
}

/// Manual probe for the 256-deep chain of four-field `#[component]` structs:
/// `OXBLOOM_DI_STACK_KB=<kb> cargo test -p oxbloom-di --test depth -- --ignored probe`.
#[test]
#[ignore = "manual stack-size probe; set OXBLOOM_DI_STACK_KB"]
fn probe_stack_for_256_deep_component_build() {
    let kb: usize = std::env::var("OXBLOOM_DI_STACK_KB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    let built = on_stack(kb, || component_chain().build().map(drop));
    assert_eq!(built, Ok(()));
}

/// Manual probe for the 256-deep chain of `.bean` factories with one `Arc` parameter each:
/// `OXBLOOM_DI_STACK_KB=<kb> cargo test -p oxbloom-di --test depth -- --ignored probe`.
#[test]
#[ignore = "manual stack-size probe; set OXBLOOM_DI_STACK_KB"]
fn probe_stack_for_256_deep_bean_build() {
    probe_bean_chain(false);
}

/// Manual probe for the 256-deep chain of four-parameter `.bean` factories:
/// `OXBLOOM_DI_STACK_KB=<kb> cargo test -p oxbloom-di --test depth -- --ignored probe`.
#[test]
#[ignore = "manual stack-size probe; set OXBLOOM_DI_STACK_KB"]
fn probe_stack_for_256_deep_wide_bean_build() {
    probe_bean_chain(true);
}

fn probe_bean_chain(wide: bool) {
    let kb: usize = std::env::var("OXBLOOM_DI_STACK_KB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    let built = on_stack(kb, move || bean_chain(256, wide).build().map(drop));
    assert_eq!(built, Ok(()));
}

/// Manual probe for the 256-deep chain of `#[bean]` methods with one `Arc` parameter each:
/// `OXBLOOM_DI_STACK_KB=<kb> cargo test -p oxbloom-di --test depth -- --ignored probe`.
#[test]
#[ignore = "manual stack-size probe; set OXBLOOM_DI_STACK_KB"]
fn probe_stack_for_256_deep_configuration_build() {
    probe_configuration_chain(false);
}

/// Manual probe for the 256-deep chain of `#[bean]` methods with four parameters each:
/// `OXBLOOM_DI_STACK_KB=<kb> cargo test -p oxbloom-di --test depth -- --ignored probe`.
#[test]
#[ignore = "manual stack-size probe; set OXBLOOM_DI_STACK_KB"]
fn probe_stack_for_256_deep_wide_configuration_build() {
    probe_configuration_chain(true);
}

fn probe_configuration_chain(wide: bool) {
    let kb: usize = std::env::var("OXBLOOM_DI_STACK_KB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    let built = on_stack(kb, move || configuration_chain(wide).build().map(drop));
    assert_eq!(built, Ok(()));
}
