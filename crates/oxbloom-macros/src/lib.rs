//! Attribute macros for Oxbloom: controllers (`#[controller]` and the route attributes),
//! dependency injection (`#[component]`, `#[configuration]`, `#[bean]`, used through
//! `oxbloom_di`, and `#[auto_configuration]`, used through `oxbloom_boot`), scheduled tasks and
//! retry (`#[scheduling]`, `#[scheduled]` and `#[retryable]`, used through `oxbloom_tasks`), and
//! method security (`#[method_security]` with `#[pre_authorize]`, `#[post_authorize]`,
//! `#[pre_filter]` and `#[post_filter]`, used through `oxbloom_security`).
//!
//! Controllers:
//! `#[controller("/base")]` goes on an `impl` block. Methods annotated with
//! `#[get]`, `#[post]`, `#[put]`, `#[patch]`, `#[delete]`, `#[head]` or
//! `#[options]` become routes. The macro adds
//! `fn routes(self: Arc<Self>) -> Vec<oxbloom_web::Route>`.
//!
//! Handler signature: `async fn name(&self, req: Request) -> impl IntoResponse`.

mod auto_configuration;
mod component;
mod configuration;
mod method_security;
mod options;
mod retryable;
mod scheduling;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Error, FnArg, ImplItem, ItemImpl, LitStr, parse_macro_input};

const METHODS: [(&str, &str); 7] = [
    ("get", "Get"),
    ("post", "Post"),
    ("put", "Put"),
    ("patch", "Patch"),
    ("delete", "Delete"),
    ("head", "Head"),
    ("options", "Options"),
];

/// Marks an `impl` block as a controller with a base path.
///
/// Adds `fn routes(self: Arc<Self>) -> Vec<Route>` and implements `Controller`. A method path of
/// `""` means the base path itself; other method paths must start with `/`. Routes that share a
/// path position must use the same parameter name.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_web::{App, HttpMethod, IntoResponse, Request, controller};
///
/// struct Hello;
///
/// #[controller("/api/hello")]
/// impl Hello {
///     #[get("")]
///     async fn index(&self, _req: Request) -> impl IntoResponse { "hello" }
///
///     #[get("/{name}")]
///     async fn greet(&self, req: Request) -> impl IntoResponse {
///         format!("Hello, {}!", req.param("name").unwrap_or("stranger"))
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = App::new().routes(Arc::new(Hello).routes())?;
/// let res = app.handle(Request::new(HttpMethod::Get, "/api/hello/Ada")).await;
/// assert_eq!(res.body(), "Hello, Ada!");
/// # Ok(())
/// # }
/// ```
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    let base = parse_macro_input!(args as LitStr);
    let item = parse_macro_input!(input as ItemImpl);
    expand_controller(base, item)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

macro_rules! route_attr {
    ($($name:ident => $doc:expr;)*) => {$(
        #[doc = $doc]
        #[proc_macro_attribute]
        pub fn $name(args: TokenStream, _input: TokenStream) -> TokenStream {
            let args = TokenStream2::from(args);
            Error::new(
                args.span(),
                concat!("#[", stringify!($name), "] is only valid on methods inside a #[controller] impl block"),
            )
            .into_compile_error()
            .into()
        }
    )*};
}
route_attr! {
        get => concat!(
"Route attribute; only valid on methods inside a `#[controller]` impl.\n\n# Examples\n\n```\n",
"use std::sync::Arc;\nuse oxbloom_web::{App, HttpMethod, IntoResponse, Request, controller};\n\n",
"struct Items;\n\n#[controller(\"/items\")]\nimpl Items {\n    #[get(\"\")]\n    async fn list(&self, _req: Request) -> impl IntoResponse { \"all items\" }\n}\n\n",
"# #[tokio::main(flavor = \"current_thread\")]\n# async fn main() -> Result<(), Box<dyn std::error::Error>> {\n",
"let app = App::new().routes(Arc::new(Items).routes())?;\n",
"let res = app.handle(Request::new(HttpMethod::Get, \"/items\")).await;\n",
"assert_eq!(res.status(), 200);\n# Ok(())\n# }\n```"
);
        post => concat!(
"Route attribute; only valid on methods inside a `#[controller]` impl.\n\n# Examples\n\n```\n",
"use std::sync::Arc;\nuse oxbloom_web::{App, HttpMethod, IntoResponse, Request, controller};\n\n",
"struct Items;\n\n#[controller(\"/items\")]\nimpl Items {\n    #[post(\"\")]\n    async fn create(&self, _req: Request) -> impl IntoResponse { (http::StatusCode::CREATED, \"made\") }\n}\n\n",
"# #[tokio::main(flavor = \"current_thread\")]\n# async fn main() -> Result<(), Box<dyn std::error::Error>> {\n",
"let app = App::new().routes(Arc::new(Items).routes())?;\n",
"let res = app.handle(Request::new(HttpMethod::Post, \"/items\")).await;\n",
"assert_eq!(res.status(), 201);\n# Ok(())\n# }\n```"
);
        put => concat!(
"Route attribute; only valid on methods inside a `#[controller]` impl.\n\n# Examples\n\n```\n",
"use std::sync::Arc;\nuse oxbloom_web::{App, HttpMethod, IntoResponse, Request, controller};\n\n",
"struct Items;\n\n#[controller(\"/items\")]\nimpl Items {\n    #[put(\"/{id}\")]\n    async fn replace(&self, _req: Request) -> impl IntoResponse { http::StatusCode::NO_CONTENT }\n}\n\n",
"# #[tokio::main(flavor = \"current_thread\")]\n# async fn main() -> Result<(), Box<dyn std::error::Error>> {\n",
"let app = App::new().routes(Arc::new(Items).routes())?;\n",
"let res = app.handle(Request::new(HttpMethod::Put, \"/items/1\")).await;\n",
"assert_eq!(res.status(), 204);\n# Ok(())\n# }\n```"
);
        patch => concat!(
"Route attribute; only valid on methods inside a `#[controller]` impl.\n\n# Examples\n\n```\n",
"use std::sync::Arc;\nuse oxbloom_web::{App, HttpMethod, IntoResponse, Request, controller};\n\n",
"struct Items;\n\n#[controller(\"/items\")]\nimpl Items {\n    #[patch(\"/{id}\")]\n    async fn update(&self, _req: Request) -> impl IntoResponse { \"patched\" }\n}\n\n",
"# #[tokio::main(flavor = \"current_thread\")]\n# async fn main() -> Result<(), Box<dyn std::error::Error>> {\n",
"let app = App::new().routes(Arc::new(Items).routes())?;\n",
"let res = app.handle(Request::new(HttpMethod::Patch, \"/items/1\")).await;\n",
"assert_eq!(res.status(), 200);\n# Ok(())\n# }\n```"
);
        delete => concat!(
"Route attribute; only valid on methods inside a `#[controller]` impl.\n\n# Examples\n\n```\n",
"use std::sync::Arc;\nuse oxbloom_web::{App, HttpMethod, IntoResponse, Request, controller};\n\n",
"struct Items;\n\n#[controller(\"/items\")]\nimpl Items {\n    #[delete(\"/{id}\")]\n    async fn remove(&self, _req: Request) -> impl IntoResponse { http::StatusCode::NO_CONTENT }\n}\n\n",
"# #[tokio::main(flavor = \"current_thread\")]\n# async fn main() -> Result<(), Box<dyn std::error::Error>> {\n",
"let app = App::new().routes(Arc::new(Items).routes())?;\n",
"let res = app.handle(Request::new(HttpMethod::Delete, \"/items/1\")).await;\n",
"assert_eq!(res.status(), 204);\n# Ok(())\n# }\n```"
);
        head => concat!(
"Route attribute; only valid on methods inside a `#[controller]` impl.\n\n# Examples\n\n```\n",
"use std::sync::Arc;\nuse oxbloom_web::{App, HttpMethod, IntoResponse, Request, controller};\n\n",
"struct Items;\n\n#[controller(\"/items\")]\nimpl Items {\n    #[head(\"/{id}\")]\n    async fn exists(&self, _req: Request) -> impl IntoResponse { http::StatusCode::OK }\n}\n\n",
"# #[tokio::main(flavor = \"current_thread\")]\n# async fn main() -> Result<(), Box<dyn std::error::Error>> {\n",
"let app = App::new().routes(Arc::new(Items).routes())?;\n",
"let res = app.handle(Request::new(HttpMethod::Head, \"/items/1\")).await;\n",
"assert_eq!(res.status(), 200);\n# Ok(())\n# }\n```"
);
        options => concat!(
"Route attribute; only valid on methods inside a `#[controller]` impl.\n\n# Examples\n\n```\n",
"use std::sync::Arc;\nuse oxbloom_web::{App, HttpMethod, IntoResponse, Request, controller};\n\n",
"struct Items;\n\n#[controller(\"/items\")]\nimpl Items {\n    #[options(\"/{id}\")]\n    async fn opts(&self, _req: Request) -> impl IntoResponse { http::StatusCode::NO_CONTENT }\n}\n\n",
"# #[tokio::main(flavor = \"current_thread\")]\n# async fn main() -> Result<(), Box<dyn std::error::Error>> {\n",
"let app = App::new().routes(Arc::new(Items).routes())?;\n",
"let res = app.handle(Request::new(HttpMethod::Options, \"/items/1\")).await;\n",
"assert_eq!(res.status(), 204);\n# Ok(())\n# }\n```"
);
}

/// `allow_empty`: a method path of `""` means "the controller base path".
fn validate_path(lit: &LitStr, allow_empty: bool) -> syn::Result<String> {
    let p = lit.value();
    if !p.starts_with('/') && !(allow_empty && p.is_empty()) {
        return Err(Error::new(lit.span(), "route path must start with '/'"));
    }
    Ok(p)
}

fn join(base: &str, sub: &str) -> String {
    let base = base.trim_end_matches('/');
    let sub = if sub == "/" { "" } else { sub };
    let joined = format!("{base}{sub}");
    if joined.is_empty() {
        "/".into()
    } else {
        joined
    }
}

fn expand_controller(base: LitStr, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    if item.trait_.is_some() {
        return Err(Error::new(
            item.span(),
            "#[controller] requires an inherent impl",
        ));
    }
    let base_path = validate_path(&base, false)?;
    let mut routes: Vec<(String, String, syn::Ident, TokenStream2)> = Vec::new();

    for it in &mut item.items {
        let ImplItem::Fn(f) = it else { continue };
        let mut found: Option<(&str, LitStr)> = None;
        let mut kept = Vec::new();
        for attr in std::mem::take(&mut f.attrs) {
            let hit = METHODS
                .iter()
                .find(|(n, _)| attr.path().is_ident(n))
                .map(|(_, variant)| *variant);
            match hit {
                Some(variant) => {
                    if found.is_some() {
                        return Err(Error::new(
                            attr.span(),
                            "only one route attribute per method",
                        ));
                    }
                    found = Some((variant, attr.parse_args::<LitStr>()?));
                }
                None => kept.push(attr),
            }
        }
        f.attrs = kept;
        let Some((variant, lit)) = found else {
            continue;
        };

        let sig = &f.sig;
        let ok_self = matches!(sig.inputs.first(), Some(FnArg::Receiver(r))
            if matches!(r.kind, syn::ReceiverKind::Reference(_, _, None)));
        if sig.asyncness.is_none() || !ok_self || sig.inputs.len() != 2 {
            return Err(Error::new(
                sig.span(),
                "route handler must be `async fn name(&self, req: Request) -> impl IntoResponse`",
            ));
        }
        let path = join(&base_path, &validate_path(&lit, true)?);
        if routes.iter().any(|(v, p, ..)| v == variant && *p == path) {
            return Err(Error::new(
                lit.span(),
                format!("duplicate route {variant} {path}"),
            ));
        }
        let ident = sig.ident.clone();
        let variant_id = syn::Ident::new(variant, lit.span());
        routes.push((
            variant.to_owned(),
            path.clone(),
            ident.clone(),
            quote! {{
                let this = ::std::sync::Arc::clone(&self);
                ::oxbloom_web::Route::new(
                    ::oxbloom_web::HttpMethod::#variant_id,
                    #path,
                    move |req| {
                        let this = ::std::sync::Arc::clone(&this);
                        async move {
                            ::oxbloom_web::IntoResponse::into_response(this.#ident(req).await)
                        }
                    },
                )
            }},
        ));
    }

    let self_ty = &item.self_ty;
    let (impl_generics, _, where_clause) = item.generics.split_for_impl();
    let route_exprs = routes.iter().map(|r| &r.3);
    Ok(quote! {
        #item

        impl #impl_generics #self_ty #where_clause {
            /// Routes generated by `#[controller]`.
            pub fn routes(self: ::std::sync::Arc<Self>) -> ::std::vec::Vec<::oxbloom_web::Route> {
                ::std::vec![#(#route_exprs),*]
            }
        }

        impl #impl_generics ::oxbloom_web::Controller for #self_ty #where_clause {
            fn routes(self: ::std::sync::Arc<Self>) -> ::std::vec::Vec<::oxbloom_web::Route> {
                // Path call resolves to the inherent `routes` above, not this trait method.
                <#self_ty>::routes(self)
            }
        }
    })
}

/// Generates `impl ::oxbloom_di::Component` for a struct whose fields are all injected.
///
/// Each field is resolved from the container by its shape:
///
/// | Field type | Resolved with | Meaning |
/// |---|---|---|
/// | `Arc<T>`, `Arc<dyn Trait>` | `Resolver::get` | exactly one provider (or one `primary`) |
/// | `Option<Arc<..>>` | `Resolver::get_opt` | `None` when nothing provides it |
/// | `Vec<Arc<..>>` | `Resolver::get_all` | every provider, by `order` then registration |
///
/// Unit structs (`struct S;`) and structs without fields are accepted. Options:
///
/// - `provides(dyn A, dyn B)`: also register the component as each listed trait object.
/// - `primary`: prefer this component when a single value of one of its keys has several
///   providers.
/// - `order = n`: its position (an `i32`, default `0`) in `Vec` / `get_all` requests.
/// - `lazy`: create it on first request instead of during `build()`.
/// - `lifecycle`: run the hooks of its `impl Lifecycle` (`post_construct` after creation,
///   `pre_destroy` on `Container::shutdown`).
/// - `profile = "dev"`, `profile = "!prod"` or `profile = ["dev", "test"]`: keep it only when
///   the profile is active (`!`: inactive; a list: any of them), as `ContainerBuilder::profile`.
///
/// The options become `Component::configure`, which `ContainerBuilder::component` applies right
/// after registering the component. Shapes are recognised by the last path segment (`Arc`,
/// `Option`, `Vec`), so type aliases of them are not. The generated code refers to the
/// `oxbloom_di` crate by that name, so the dependency must not be renamed in `Cargo.toml`.
/// Enums, tuple structs, generic structs, unknown or repeated options, `provides` with a type that is not `dyn Trait`, empty profile
/// names, fields of another shape and `#[cfg]` on fields are compile errors; so are
/// `provides(dyn Trait)` on a struct that does not implement `Trait` and `lifecycle` on one
/// without `impl Lifecycle`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{ContainerBuilder, component};
///
/// #[component]
/// struct UserRepo {}
///
/// #[component]
/// struct UserService { repo: Arc<UserRepo> }
///
/// let container = ContainerBuilder::new()
///     .component::<UserService>()   // order does not matter
///     .component::<UserRepo>()
///     .build()?;
/// let service = container.get::<UserService>()?;
/// assert!(Arc::ptr_eq(&service.repo, &container.get::<UserRepo>()?));
/// # Ok::<(), oxbloom_di::DiError>(())
/// ```
///
/// Trait objects, collections and optional dependencies:
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{ContainerBuilder, DiError, component};
///
/// trait Notifier: Send + Sync { fn send(&self) -> &'static str; }
///
/// #[component(provides(dyn Notifier), primary)]
/// struct Mail;
/// impl Notifier for Mail { fn send(&self) -> &'static str { "mail" } }
///
/// #[component(provides(dyn Notifier), order = -1)]
/// struct Sms;
/// impl Notifier for Sms { fn send(&self) -> &'static str { "sms" } }
///
/// struct Metrics;
///
/// #[component]
/// struct Alerts {
///     main: Arc<dyn Notifier>,             // the primary provider
///     all: Vec<Arc<dyn Notifier>>,         // every provider, `order` first
///     metrics: Option<Arc<Metrics>>,       // not registered: None
/// }
///
/// let c = ContainerBuilder::new()
///     .component::<Alerts>()
///     .component::<Mail>()
///     .component::<Sms>()
///     .build()?;
/// let alerts = c.get::<Alerts>()?;
/// assert_eq!(alerts.main.send(), "mail");
/// let all: Vec<_> = alerts.all.iter().map(|n| n.send()).collect();
/// assert_eq!(all, ["sms", "mail"]);
/// assert!(alerts.metrics.is_none());
/// # Ok::<(), DiError>(())
/// ```
///
/// Lifecycle hooks, lazy creation and profiles:
///
/// ```
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use oxbloom_di::{ContainerBuilder, DiError, Lifecycle, component};
///
/// #[component(lifecycle, lazy)]
/// struct Scheduler;
/// static STARTED: AtomicBool = AtomicBool::new(false);
/// impl Lifecycle for Scheduler {
///     fn post_construct(&self) -> Result<(), DiError> {
///         STARTED.store(true, Ordering::SeqCst);
///         Ok(())
///     }
/// }
///
/// #[component(profile = "dev")]
/// struct DemoData;
///
/// let c = ContainerBuilder::new()
///     .active_profiles(["prod"])
///     .component::<Scheduler>()
///     .component::<DemoData>()
///     .build()?;
/// assert!(!STARTED.load(Ordering::SeqCst)); // lazy: not created yet
/// c.get::<Scheduler>()?;
/// assert!(STARTED.load(Ordering::SeqCst));  // created, then `post_construct` ran
/// assert!(matches!(c.get::<DemoData>(), Err(DiError::Missing { .. }))); // "dev" is inactive
/// # Ok::<(), DiError>(())
/// ```
#[proc_macro_attribute]
pub fn component(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut options = options::Options::new(options::Kind::Component);
    let parser = syn::meta::parser(|meta| options.parse(meta));
    parse_macro_input!(args with parser);
    let item = parse_macro_input!(input as syn::Item);
    component::expand(options, item)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Generates `impl ::oxbloom_di::Configuration` for an inherent `impl` block whose `#[bean]`
/// methods create beans, for third-party types and values that need code to build.
///
/// Each `#[bean]` method becomes one registration, in declaration order. Its bean is the
/// method's return type; creating it resolves the configuration itself (`Arc<Self>`, so the
/// configuration type must be registered too, e.g. as a `#[component]` or `.instance(..)`),
/// then each further parameter by its shape (`Arc<T>`, `Arc<dyn Trait>`, `Option<Arc<..>>`,
/// `Vec<Arc<..>>`), then calls the method. Its source label in `Duplicate` and `Ambiguous`
/// errors is `Type::method`. Methods without `#[bean]` are left alone.
///
/// `#[bean]` options:
///
/// - `fallible`: the method returns `Result<T, DiError>` and the bean is `T`; an `Err` fails
///   the bean's creation like `ContainerBuilder::try_bean`. There is no guessing from the
///   return type: without `fallible`, a `Result` is the bean.
/// - `provides(dyn A, ..)`, `primary`, `order = n`, `lazy`, `profile = ..`: as on
///   `#[component]`.
/// - `on_bean(T)`, `on_missing_bean`, `on_missing_bean(T)`, `on_property("key")`,
///   `on_property("key", "value")`: conditions on the registration; see [`macro@bean`].
///
/// A `#[bean]` method takes `&self` (also written `self: &Self`). A `#[cfg(..)]` on it, or a
/// `#[cfg_attr(pred, cfg(..))]`, also applies to its registration: a method compiled out
/// registers no bean. A `cfg` nested more deeply (`#[cfg_attr(a, cfg_attr(b, cfg(..)))]`) is
/// not copied and fails to compile (E0599) when the method is compiled out; write it as
/// `#[cfg_attr(all(a, b), cfg(..))]`.
///
/// `#[bean]` methods cannot take lifecycle hooks: the `lifecycle` option is rejected. Register
/// the resource as an `instance` or `factory` followed by `.lifecycle::<T>()`, or add
/// `.pre_destroy(..)` right after `.bean(..)`.
///
/// Use one `#[configuration]` impl per type: each generates `impl Configuration` for it, so a
/// second one is a conflicting-implementations error; put all bean methods of a type in one
/// block. The generated code refers to the `oxbloom_di` crate by that name, so the dependency
/// must not be renamed in `Cargo.toml` (the same holds for `#[component]`).
///
/// Generic impls, trait impls, `#[bean]` outside a `#[configuration]` impl, methods that do
/// not take `&self` (or are `async`, `unsafe` or generic), parameters that are not injectable
/// shapes, `impl Trait` parameters or return types, a missing return type, `#[cfg]` on
/// parameters, and unknown or repeated options are compile errors.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{ContainerBuilder, DiError, component, configuration};
///
/// // Third-party types: no `#[component]` possible.
/// struct Pool { url: String }
/// trait Store: Send + Sync { fn name(&self) -> String; }
/// struct PgStore { pool: Arc<Pool> }
/// impl Store for PgStore { fn name(&self) -> String { format!("pg at {}", self.pool.url) } }
///
/// #[component]
/// struct Persistence;
///
/// #[configuration]
/// impl Persistence {
///     #[bean]
///     fn pool(&self) -> Pool {
///         Pool { url: "postgres://db/app".into() }
///     }
///
///     #[bean(fallible, provides(dyn Store))]
///     fn store(&self, pool: Arc<Pool>) -> Result<PgStore, DiError> {
///         if pool.url.is_empty() {
///             return Err(DiError::creation::<PgStore>("no database url"));
///         }
///         Ok(PgStore { pool })
///     }
/// }
///
/// let c = ContainerBuilder::new()
///     .component::<Persistence>()
///     .configuration::<Persistence>()
///     .build()?;
/// assert_eq!(c.get::<dyn Store>()?.name(), "pg at postgres://db/app");
///
/// // The configuration itself must be registered: its beans need it.
/// let err = ContainerBuilder::new().configuration::<Persistence>().build();
/// assert!(matches!(err, Err(DiError::Missing { .. })));
/// # Ok::<(), DiError>(())
/// ```
///
/// Bean options, with settings held by the configuration:
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{ContainerBuilder, DiError, configuration};
///
/// trait Mailer: Send + Sync { fn kind(&self) -> &'static str; }
/// struct Smtp { host: String }
/// impl Mailer for Smtp { fn kind(&self) -> &'static str { "smtp" } }
/// struct Console;
/// impl Mailer for Console { fn kind(&self) -> &'static str { "console" } }
///
/// struct MailConfig { host: &'static str }
///
/// #[configuration]
/// impl MailConfig {
///     #[bean(provides(dyn Mailer), profile = "prod", lazy)]
///     fn smtp(&self) -> Smtp { Smtp { host: self.host.to_owned() } }
///
///     #[bean(provides(dyn Mailer), profile = "!prod")]
///     fn console(&self) -> Console { Console }
/// }
///
/// let c = ContainerBuilder::new()
///     .instance(MailConfig { host: "mail.example.org" })
///     .configuration::<MailConfig>()
///     .build()?;
/// assert_eq!(c.get::<dyn Mailer>()?.kind(), "console"); // no profile active
///
/// let c = ContainerBuilder::new()
///     .active_profiles(["prod"])
///     .instance(MailConfig { host: "mail.example.org" })
///     .configuration::<MailConfig>()
///     .build()?;
/// assert_eq!(c.get::<dyn Mailer>()?.kind(), "smtp");
/// assert_eq!(c.get::<Smtp>()?.host, "mail.example.org");
/// # Ok::<(), DiError>(())
/// ```
#[proc_macro_attribute]
pub fn configuration(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    configuration::expand(args.into(), item)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Generates `impl ::oxbloom_di::Configuration` (exactly as [`macro@configuration`]) and
/// `impl ::oxbloom_boot::AutoConfiguration` for an inherent `impl` block: a configuration that
/// an application adds as a whole, with conditions decided once for all of its beans and a place
/// in the order of the auto-configurations. Use it through `oxbloom_boot::auto_configuration`.
///
/// The `#[bean]` methods and their options work as in [`macro@configuration`], including
/// `fallible`, `lazy`, `#[cfg]` and the method-level conditions. Options of the attribute:
///
/// - `after(A, B, ..)`: `A` and `B` are processed before this configuration
///   (`AutoConfiguration::after`).
/// - `before(C, ..)`: this configuration is processed before `C` (`AutoConfiguration::before`).
/// - `on_bean(T)`, `on_missing_bean(T)` (also `dyn Trait`), `on_property("key")`,
///   `on_property("key", "value")`: the class-level conditions (`AutoConfiguration::conditions`,
///   built with `Conditions`). `on_property("key")` matches whenever the key is present, also
///   with the value `false`; for a switch, write `on_property("key", "true")`, which compares
///   exactly and case-sensitively. They are ANDed in source order and decided once, before the first
///   bean of the configuration, against the registrations kept before it; when they fail, every
///   bean of the configuration is skipped. Its own beans therefore never hide each other.
///
/// `after` and `before` take at least one type each and may appear once; conditions may repeat.
/// Without options only the default `AutoConfiguration` methods are used. `on_missing_bean`
/// needs a type here: unlike a `#[bean]` method, the configuration has no return type to stand
/// for. Generic impls, trait impls, unknown or repeated options, an empty `after()` or
/// `before()`, a non-type argument and an empty `on_property` key are compile errors.
///
/// The type must also implement `oxbloom_di::Component`, usually through `#[component]` on the
/// struct (a unit struct works; fields are injected as usual); without it the generated
/// `AutoConfiguration` impl fails with rustc's unsatisfied `Component` bound. The application
/// registers the instance with `component::<Self>()` as the first registration of the
/// configuration's group, so each `#[bean]` method gets its `Arc<Self>`, and the instance is
/// skipped together with the group when the class-level conditions fail.
///
/// The generated code names `::oxbloom_boot` and `::oxbloom_di` by absolute path, so the crate
/// using the macro must depend on both under those names (not renamed in `Cargo.toml`).
///
/// # Examples
///
/// ```
/// use std::any::type_name;
/// use oxbloom_boot::{AutoConfiguration, ConfigRef, Configuration, auto_configuration};
/// use oxbloom_di::{ContainerBuilder, DiError, component};
///
/// struct Pool { size: usize }
/// struct Cache { size: usize }
///
/// #[component]
/// pub struct DataSourceAutoConfig;
///
/// #[auto_configuration]
/// impl DataSourceAutoConfig {
///     #[bean]
///     fn pool(&self) -> Pool { Pool { size: 4 } }
/// }
///
/// #[component]
/// pub struct CacheAutoConfig;
///
/// // After the data source, only with `cache.enabled` set and no user `Cache`.
/// #[auto_configuration(
///     after(DataSourceAutoConfig),
///     on_property("cache.enabled"),
///     on_missing_bean(Cache)
/// )]
/// impl CacheAutoConfig {
///     #[bean]
///     fn cache(&self) -> Cache { Cache { size: 64 } }
/// }
///
/// assert_eq!(CacheAutoConfig::after(), [ConfigRef::of::<DataSourceAutoConfig>()]);
///
/// // Each one as a group with its class-level conditions, holding the instance and then the
/// // beans, as `oxbloom-boot` registers it. No environment: `cache.enabled` is absent, so the
/// // whole cache configuration is skipped, its instance included.
/// let c = ContainerBuilder::new()
///     .group(type_name::<DataSourceAutoConfig>(), DataSourceAutoConfig::conditions(), |b| {
///         DataSourceAutoConfig::register(b.component::<DataSourceAutoConfig>())
///     })
///     .group(type_name::<CacheAutoConfig>(), CacheAutoConfig::conditions(), |b| {
///         CacheAutoConfig::register(b.component::<CacheAutoConfig>())
///     })
///     .build()?;
/// assert_eq!(c.get::<Pool>()?.size, 4);
/// assert!(c.get_opt::<Cache>()?.is_none());
/// assert!(c.get_opt::<CacheAutoConfig>()?.is_none());
/// # Ok::<(), DiError>(())
/// ```
///
/// A class-level `on_missing_bean` needs its type:
///
/// ```compile_fail
/// use oxbloom_boot::auto_configuration;
/// use oxbloom_di::component;
///
/// #[component]
/// struct Defaults;
///
/// #[auto_configuration(on_missing_bean)]
/// impl Defaults {}
/// ```
#[proc_macro_attribute]
pub fn auto_configuration(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut options = auto_configuration::ClassOptions::default();
    let parser = syn::meta::parser(|meta| options.parse(meta));
    parse_macro_input!(args with parser);
    let item = parse_macro_input!(input as ItemImpl);
    auto_configuration::expand(options, item)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Marks a method of a `#[configuration]` impl as a bean factory; see
/// [`macro@configuration`] for the options. `#[configuration]` consumes it, so used anywhere
/// else it is a compile error.
///
/// # Conditions
///
/// These options keep the bean only when a condition holds. Each becomes the builder modifier
/// of the same name on the method's registration, and `build()` evaluates them once, before any
/// bean is created:
///
/// - `on_bean(T)`: a registration kept before this one provides `T` (`.on_bean::<T>()`).
/// - `on_missing_bean`: no registration kept before this one provides the bean's own type, the
///   return type (for a `fallible` bean, the `T` of `Result<T, DiError>`). This is the back-off
///   rule for defaults: the application registers its own bean first and the default skips
///   itself.
/// - `on_missing_bean(T)`: the same for an explicit type, also `dyn Trait`
///   (`.on_missing_bean::<T>()`).
/// - `on_property("key")`: the container's `Environment` has the property (`.on_property(..)`),
///   whatever its value. It also matches when the value is `false`, unlike Spring's
///   `@ConditionalOnProperty`; for a switch, write `on_property("key", "true")`.
/// - `on_property("key", "value")`: the property equals `"value"` exactly and case-sensitively
///   (`.on_property_value(..)`), so `TRUE` does not match `"true"`, unlike Spring's
///   `havingValue`. The expected value is part of the source code: never put a secret there.
///
/// Bare `on_missing_bean` guards the method's concrete return type only, also next to
/// `provides(dyn Trait)`. If the application provides `dyn Trait` through a bean of another
/// type, the default does not back off: both beans stay and a single `Arc<dyn Trait>` request is
/// `Ambiguous`. To back off for the trait, write `on_missing_bean(dyn Trait)`:
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{ContainerBuilder, DiError, configuration};
///
/// trait Clock: Send + Sync { fn now(&self) -> u64; }
/// struct SystemClock;
/// impl Clock for SystemClock { fn now(&self) -> u64 { 1 } }
/// struct FixedClock;
/// impl Clock for FixedClock { fn now(&self) -> u64 { 42 } }
///
/// struct Wrong;
/// #[configuration]
/// impl Wrong {
///     // Guards `SystemClock`, not `dyn Clock`.
///     #[bean(provides(dyn Clock), on_missing_bean)]
///     fn clock(&self) -> SystemClock { SystemClock }
/// }
///
/// struct Right;
/// #[configuration]
/// impl Right {
///     #[bean(provides(dyn Clock), on_missing_bean(dyn Clock))]
///     fn clock(&self) -> SystemClock { SystemClock }
/// }
///
/// let user = || {
///     ContainerBuilder::new()
///         .instance(FixedClock)
///         .provides(|c: Arc<FixedClock>| -> Arc<dyn Clock> { c })
/// };
/// let c = user().instance(Wrong).configuration::<Wrong>().build()?;
/// assert!(matches!(c.get::<dyn Clock>(), Err(DiError::Ambiguous { .. })));
///
/// let c = user().instance(Right).configuration::<Right>().build()?;
/// assert_eq!(c.get::<dyn Clock>()?.now(), 42);
/// # Ok::<(), DiError>(())
/// ```
///
/// The options may repeat and are combined with AND, evaluated in source order. A skipped bean
/// is invisible: it is not a candidate, its method never runs, and a dependency on it is
/// `Missing`. A bean only sees registrations made before it, including the earlier beans of its
/// own `#[configuration]`. A `#[cfg]` that removes the method removes its conditions too. An
/// empty key, a third `on_property` argument, `on_bean` without a type and a non-type argument
/// are compile errors.
///
/// ```
/// use std::collections::HashMap;
/// use oxbloom_di::{ContainerBuilder, DiError, Environment, configuration};
///
/// struct Props(HashMap<&'static str, &'static str>);
/// impl Environment for Props {
///     fn property(&self, key: &str) -> Option<String> {
///         self.0.get(key).map(|v| v.to_string())
///     }
/// }
///
/// struct Cache { size: usize }
/// struct Metrics;
///
/// struct Defaults;
///
/// #[configuration]
/// impl Defaults {
///     // Backs off when the application registered its own `Cache`.
///     #[bean(on_missing_bean)]
///     fn cache(&self) -> Cache { Cache { size: 16 } }
///
///     // Only with `metrics.export = "on"` and a `Cache` kept before it.
///     #[bean(on_bean(Cache), on_property("metrics.export", "on"))]
///     fn metrics(&self) -> Metrics { Metrics }
/// }
///
/// let props = || Props(HashMap::from([("metrics.export", "on")]));
/// let c = ContainerBuilder::new()
///     .environment(props())
///     .instance(Defaults)
///     .configuration::<Defaults>()
///     .build()?;
/// assert_eq!(c.get::<Cache>()?.size, 16);
/// assert!(c.get_opt::<Metrics>()?.is_some());
///
/// // The application's own `Cache` wins, and no property means no `Metrics`.
/// let c = ContainerBuilder::new()
///     .instance(Cache { size: 1024 })
///     .instance(Defaults)
///     .configuration::<Defaults>()
///     .build()?;
/// assert_eq!(c.get::<Cache>()?.size, 1024);
/// assert!(c.get_opt::<Metrics>()?.is_none());
/// let skipped: Vec<_> = c
///     .condition_report()
///     .records()
///     .iter()
///     .filter(|r| !r.matched())
///     .map(|r| r.condition())
///     .collect();
/// assert_eq!(skipped, ["on_missing_bean", "on_property_value"]);
/// # Ok::<(), DiError>(())
/// ```
///
/// # Examples
///
/// ```
/// use oxbloom_di::{ContainerBuilder, DiError, configuration};
///
/// struct Limits { max: u32 }
/// struct Defaults;
///
/// #[configuration]
/// impl Defaults {
///     #[bean(order = 1)]
///     fn limits(&self) -> Limits { Limits { max: 10 } }
/// }
///
/// let c = ContainerBuilder::new()
///     .instance(Defaults)
///     .configuration::<Defaults>()
///     .build()?;
/// assert_eq!(c.get::<Limits>()?.max, 10);
/// # Ok::<(), DiError>(())
/// ```
#[proc_macro_attribute]
pub fn bean(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input = TokenStream2::from(input);
    // The call site of an attribute macro is the attribute itself.
    let error = Error::new(
        proc_macro2::Span::call_site(),
        "#[bean] is only valid on methods inside a #[configuration] impl block",
    )
    .into_compile_error();
    // The item stays, so the misplaced attribute is the only error.
    quote! { #error #input }.into()
}

/// Generates `impl ::oxbloom_tasks::Scheduling` for an inherent `impl` block whose
/// `#[scheduled(..)]` methods run on a schedule.
///
/// `fn scheduled(self: Arc<Self>) -> Vec<Task>` returns one task per `#[scheduled]` method, in
/// declaration order; each run calls the method on the shared instance. Methods without
/// `#[scheduled]` are left alone. Use it through `oxbloom_tasks::scheduling`; the generated
/// code refers to the `oxbloom_tasks` crate by that name, so the dependency must not be renamed
/// in `Cargo.toml`.
///
/// `#[scheduled(..)]` keys, all string literals:
///
/// - exactly one trigger: `cron = "0 0 3 * * *"` (six fields, seconds first),
///   `fixed_rate = "30s"` (measured from the start of the last run) or `fixed_delay = "30s"`
///   (measured from its end); durations are a whole number with `ms`, `s`, `m`, `h` or `d`;
/// - `initial_delay = "5s"`: postpones the first run;
/// - `zone = "Europe/Amsterdam"`: the IANA zone of a `cron` trigger; without it the trigger
///   uses the scheduler's zone (`SchedulerConfig::zone`, `tasks.scheduling.zone` in boot, UTC by
///   default);
/// - `name = ".."`: the task name in logs and in `Scheduler::tasks` (default `Type::method`).
///
/// A method takes only `&self` and returns `()` or `Result<(), E>` where `E: Display`; an
/// `Err` is a failed run, logged with `E`'s `to_string()`. An `async fn` is awaited on the
/// runtime; a plain `fn` runs in `tokio::task::spawn_blocking`, so blocking work does not stall
/// the other tasks.
///
/// The literals are parsed when `scheduled()` runs. A value that does not parse (a bad cron
/// expression, zone or duration, or a zero `fixed_rate` or `fixed_delay`) gives
/// `Task::invalid`, and `Scheduler::try_new` fails with a `TaskError` naming the task and the
/// key, never the value.
///
/// Trait impls, two triggers or none, `zone` without `cron`, unknown or repeated keys,
/// methods that take anything besides `&self` (or are generic), a return type other than `()`
/// or `Result<..>`, and `#[scheduled]` outside a `#[scheduling]` impl are compile errors.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// use oxbloom_tasks::{Scheduling, Trigger, scheduling};
///
/// struct Reports;
///
/// #[scheduling]
/// impl Reports {
///     #[scheduled(cron = "0 0 3 * * *", zone = "Europe/Amsterdam")]
///     async fn nightly(&self) -> Result<(), std::io::Error> {
///         Ok(())
///     }
///
///     #[scheduled(fixed_rate = "30s", initial_delay = "5s", name = "refresh")]
///     fn refresh(&self) {}
/// }
///
/// let tasks = Arc::new(Reports).scheduled();
/// assert_eq!(tasks[0].name(), "Reports::nightly");
/// assert_eq!(tasks[1].name(), "refresh");
/// assert_eq!(tasks[1].trigger(), Some(&Trigger::fixed_rate(Duration::from_secs(30))?));
/// assert_eq!(tasks[1].delay(), Some(Duration::from_secs(5)));
/// # Ok::<(), oxbloom_tasks::TriggerError>(())
/// ```
///
/// A value that does not parse fails when the scheduler is built:
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_core::{Clock, SystemClock};
/// use oxbloom_tasks::{Scheduler, SchedulerConfig, Scheduling, scheduling};
///
/// struct Jobs;
///
/// #[scheduling]
/// impl Jobs {
///     #[scheduled(fixed_delay = "soon")]
///     async fn poll(&self) {}
/// }
///
/// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
/// let err = Scheduler::try_new(Arc::new(Jobs).scheduled(), SchedulerConfig::default(), clock)
///     .unwrap_err();
/// assert_eq!(err.task(), "Jobs::poll");
/// assert_eq!(err.key(), "fixed_delay");
/// assert!(!err.to_string().contains("soon"));
/// ```
#[proc_macro_attribute]
pub fn scheduling(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    scheduling::expand(args.into(), item)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Marks a method of a `#[scheduling]` impl as a scheduled task; see [`macro@scheduling`] for
/// the keys. Anywhere else it is a compile error.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_tasks::{Scheduling, scheduling};
///
/// struct Cleanup;
///
/// #[scheduling]
/// impl Cleanup {
///     #[scheduled(fixed_delay = "1m")]
///     async fn remove_done(&self) {}
/// }
///
/// assert_eq!(Arc::new(Cleanup).scheduled()[0].name(), "Cleanup::remove_done");
/// ```
#[proc_macro_attribute]
pub fn scheduled(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input = TokenStream2::from(input);
    // The call site of an attribute macro is the attribute itself.
    let error = Error::new(
        proc_macro2::Span::call_site(),
        "#[scheduled] is only valid on methods inside a #[scheduling] impl block",
    )
    .into_compile_error();
    // The item stays, so the misplaced attribute is the only error.
    quote! { #error #input }.into()
}

/// Runs the body of an `async` method again when it returns `Err`, with fixed or exponential
/// backoff and jitter between the attempts.
///
/// The method keeps its signature. Its body becomes a call to
/// `oxbloom_tasks::RetryPolicy::run` with the method name as the label, so each retry logs a
/// `warn` event and running out of attempts logs one `error` event on target `oxbloom::tasks`.
/// When the attempts run out, the last error is returned; a panic is never retried. Use it
/// through `oxbloom_tasks::retryable`; the generated code refers to the `oxbloom_tasks` crate by
/// that name, so the dependency must not be renamed in `Cargo.toml`.
///
/// Keys, all optional:
///
/// - `max_attempts = 3`: how many times the body runs at most, counting the first call;
/// - `backoff = "exponential"` or `"fixed"`: retry *n* (from 1) waits
///   `initial * multiplier^(n-1)`, or always `initial`;
/// - `initial = "100ms"` and `max_delay = "30s"`: the first delay and the cap on every delay;
///   durations are a whole number with `ms`, `s`, `m`, `h` or `d`;
/// - `multiplier = 2.0`: the factor of exponential backoff, a number of at least 1;
/// - `jitter = "full"`, `"equal"` or `"none"`: full jitter waits a random time up to the
///   delay, equal jitter half the delay plus a random half;
/// - `retry_if = "path::to::is_transient"`: a `fn(&E) -> bool`; when it returns `false`, the
///   error comes back at once.
///
/// A key that the attribute omits is read from `oxbloom_tasks::defaults()` every time the
/// method is called, so the `tasks.retry.*` keys of boot apply to it. A key that is present is
/// fixed at compile time.
///
/// Every attempt runs the original body again. The method must be `async`, return
/// `Result<T, E>` (or an alias such as `io::Result<T>`) with `E: Display`, and take `&self` or
/// no receiver. The body may borrow the arguments but not move them out: clone an argument
/// that it needs to own. A `return` inside the body ends that attempt, and `?` works as in any
/// other method.
///
/// `#[retryable]` must be the topmost attribute on its method. An attribute above it that
/// rewrites the body, such as `#[tracing::instrument]`, can move the arguments into the retry
/// closure and give a confusing error. It cannot be used inside an `#[async_trait]` impl:
/// `async_trait` expands first and turns the method into a plain `fn`, so `#[retryable]` then
/// reports that the method must be `async`.
///
/// `#[retryable]` combines with `#[scheduled]` on a method of a `#[scheduling]` impl, in either
/// order: one scheduled run then includes all its attempts, and a trigger that fires while the
/// retries still run is skipped.
///
/// A plain `fn`, a return type that is not a `Result`, a receiver other than `&self`, a `mut`
/// argument, generic parameters, `max_attempts = 0`, an unknown `backoff` or `jitter` name, a
/// duration that does not parse, `initial` greater than `max_delay`, a `multiplier` below 1 or
/// not finite, a `retry_if` that is not a path in a string, and unknown or repeated keys are
/// compile errors that name the method.
///
/// # Examples
///
/// ```
/// use std::sync::atomic::{AtomicU32, Ordering};
/// use oxbloom_tasks::retryable;
///
/// #[derive(Debug, PartialEq)]
/// enum PaymentError {
///     Unavailable,
///     Declined,
/// }
///
/// impl std::fmt::Display for PaymentError {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         f.write_str(match self {
///             PaymentError::Unavailable => "payment service unavailable",
///             PaymentError::Declined => "card declined",
///         })
///     }
/// }
///
/// fn is_transient(error: &PaymentError) -> bool {
///     *error == PaymentError::Unavailable
/// }
///
/// struct Payments {
///     calls: AtomicU32,
/// }
///
/// impl Payments {
///     #[retryable(max_attempts = 5, backoff = "exponential", initial = "200ms", max_delay = "10s", jitter = "full", retry_if = "is_transient")]
///     async fn charge(&self, id: u64) -> Result<String, PaymentError> {
///         let attempt = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
///         if attempt < 3 {
///             return Err(PaymentError::Unavailable);
///         }
///         Ok(format!("receipt {id}"))
///     }
///
///     #[retryable(max_attempts = 3, retry_if = "is_transient")]
///     async fn refund(&self, _id: u64) -> Result<(), PaymentError> {
///         self.calls.fetch_add(1, Ordering::SeqCst);
///         Err(PaymentError::Declined)
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build()?.block_on(async {
/// let payments = Payments { calls: AtomicU32::new(0) };
/// assert_eq!(payments.charge(7).await, Ok("receipt 7".to_owned()));
/// assert_eq!(payments.calls.load(Ordering::SeqCst), 3);
///
/// // A declined card is not transient, so it is not retried.
/// payments.calls.store(0, Ordering::SeqCst);
/// assert_eq!(payments.refund(7).await, Err(PaymentError::Declined));
/// assert_eq!(payments.calls.load(Ordering::SeqCst), 1);
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// On a scheduled method, the attempts of one run delay the next run:
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_tasks::{Scheduling, retryable, scheduling};
///
/// struct Mirror;
///
/// #[scheduling]
/// impl Mirror {
///     #[scheduled(fixed_rate = "1m")]
///     #[retryable(max_attempts = 4, initial = "1s")]
///     async fn pull(&self) -> Result<(), std::io::Error> {
///         Ok(())
///     }
/// }
///
/// assert_eq!(Arc::new(Mirror).scheduled()[0].name(), "Mirror::pull");
/// ```
#[proc_macro_attribute]
pub fn retryable(args: TokenStream, input: TokenStream) -> TokenStream {
    retryable::expand(args.into(), input.into()).into()
}

/// Checks the caller's `Authentication` around a method body. Put it on an inherent `impl`
/// block and give each secured method one or more of these attributes:
///
/// - `#[pre_authorize(<rule>)]` checks the rule before the body runs;
/// - `#[post_authorize(<rule>)]` checks the rule after the body returned `Ok(value)`; a denial
///   drops the value and returns `Err`, and an `Err` from the body skips the check and passes
///   through;
/// - `#[pre_filter(arg = ids, custom = keep)]` keeps only the elements of the `Vec<T>`
///   parameter `ids` (taken by value) for which `keep(auth, &element)` returns true;
/// - `#[post_filter(custom = keep)]` does the same for the collection in an `Ok` result (a
///   `Vec<T>`, or any other type with a `retain` method that takes `FnMut(&T) -> bool`, such as
///   `HashSet<T>`); an `Err` passes through.
///
/// Only `method_security` needs an import: inside its `impl` block it handles the four method
/// attributes itself, so `pre_authorize` and the others need no `use`. Use it through
/// `oxbloom_security::method_security`; the generated code refers to the `oxbloom_security`
/// crate by that name, so the dependency must not be renamed in `Cargo.toml`. The rules are
/// checked against `SecurityContext::current()`, which the security filter chain sets for every
/// request; outside any `SecurityContext::scope` the caller is anonymous. The context is read
/// once per call. For an `async` method the check runs when the returned future is first
/// polled, not when the method is called, so await the future inside the caller's scope.
/// Methods without an attribute stay as written.
///
/// Rules, all checked at compile time:
///
/// - `role = "ADMIN"` and `any_role("ADMIN", "AUDITOR")`: the caller holds `ROLE_ADMIN`, or
///   one of the roles;
/// - `authority = "accounts:read"` and `any_authority("a", "b")`: the exact authority, or one
///   of them;
/// - `authenticated` (also a caller restored from a remember-me cookie) and
///   `fully_authenticated` (not a remembered caller);
/// - `permit_all` and `deny_all`;
/// - `all(<rule>, ..)`, `any(<rule>, ..)` and `not(<rule>)` to combine rules;
/// - `custom = check(arg1, arg2)` calls the synchronous function
///   `check(auth: Option<&Authentication>, &arg1, &arg2) -> bool`, where each name is a
///   parameter of the method that is bound to a plain name (not a pattern such as
///   `(a, b): (u32, u32)`). In `#[post_authorize]` the reserved name `returned` stands for
///   `&T`, a reference to the `Ok` value; there a parameter can be named only when the body did
///   not move it. The name `returned` is reserved even when the method has a parameter of that
///   name: in `#[pre_authorize]` naming it is a compile error, and in `#[post_authorize]` it
///   means the returned value, not the parameter.
///
/// The order of execution is fixed: `pre_filter`, `pre_authorize`, the body, `post_authorize`
/// and `post_filter`, whatever the order of the attributes. Filters never deny; they only
/// shrink the collection, possibly to an empty `Vec`.
///
/// A method with `#[pre_authorize]`, `#[post_authorize]` or `#[post_filter]`, `async` or not,
/// must return `Result<T, E>` with `E: From<oxbloom_security::AccessDenied>`, for example
/// `AccessDenied` itself or `oxbloom_web::HttpError` (401 for an anonymous caller, 403
/// otherwise); for `#[post_filter]`, `T` must have a `retain` method as described above. A
/// method with only `#[pre_filter]` may
/// return anything. When a rule fails, the method returns
/// `Err(AccessDenied::Unauthenticated.into())` for an anonymous caller and
/// `Err(AccessDenied::Forbidden.into())` for any other caller, and logs the method
/// (`Type::method`) and the principal at debug level on the target `oxbloom::security`.
/// Argument values and returned values are never logged.
///
/// With a post attribute the body is wrapped (in a closure for a synchronous method and in an
/// `async` block for an `async` one, both of the declared return type), so `return` and `?` in
/// the body leave only the body and the post checks still run. In an `async` method the macro
/// turns the body's own `return`s into `break`s out of a block of the declared return type, so
/// they keep its coercions (`return Ok(&self.name)` for `Result<&str, E>`). A `return` written
/// inside a macro call cannot be rewritten; it still leaves only the body, but without those
/// coercions. The return type of such a
/// method must therefore be written out; `impl Trait` is a compile error there.
///
/// It combines with `#[controller]` on the same block, in either order, so a handler can carry
/// both a route and a rule. A trait impl, a repeated attribute, an unknown rule or option, a
/// `custom` argument that is not a parameter of the method, `returned` outside
/// `#[post_authorize]`, an `arg` that is not a by-value `Vec<T>` parameter, and a method whose
/// return type is visibly not a `Result` are compile errors.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{
///     AccessDenied, Authentication, GrantedAuthority, SecurityContext, method_security,
/// };
///
/// /// True when the caller owns the account.
/// fn owns(auth: Option<&Authentication>, owner: &String) -> bool {
///     auth.is_some_and(|a| a.principal() == owner)
/// }
///
/// struct Accounts;
///
/// #[method_security]
/// impl Accounts {
///     #[pre_authorize(role = "ADMIN")]
///     async fn close(&self, id: u64) -> Result<u64, AccessDenied> {
///         Ok(id)
///     }
///
///     #[pre_authorize(any(role = "ADMIN", custom = owns(owner)))]
///     fn rename(&self, owner: String, name: &str) -> Result<String, AccessDenied> {
///         Ok(format!("{owner}: {name}"))
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let admin = Authentication::new("ada", [GrantedAuthority::new("ROLE_ADMIN")]);
/// let closed = SecurityContext::scope(SecurityContext::authenticated(admin), Accounts.close(7));
/// assert_eq!(closed.await, Ok(7));
///
/// let bob = SecurityContext::authenticated(Authentication::new("bob", []));
/// let renamed = SecurityContext::scope(bob.clone(), async {
///     Accounts.rename("bob".to_owned(), "savings")
/// });
/// assert_eq!(renamed.await.as_deref(), Ok("bob: savings"));
/// let denied = SecurityContext::scope(bob, Accounts.close(7));
/// assert_eq!(denied.await, Err(AccessDenied::Forbidden));
///
/// // Outside a scope the caller is anonymous.
/// assert_eq!(Accounts.close(7).await, Err(AccessDenied::Unauthenticated));
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// Checks after the body and filters, with no import for the method attributes:
///
/// ```
/// use oxbloom_security::{AccessDenied, Authentication, SecurityContext, method_security};
///
/// #[derive(Debug, PartialEq)]
/// struct Note {
///     owner: String,
///     text: String,
/// }
///
/// /// True when the caller wrote the note.
/// fn wrote(auth: Option<&Authentication>, note: &Note) -> bool {
///     auth.is_some_and(|a| a.principal() == note.owner)
/// }
///
/// /// True for the ids an authenticated caller may see: the even ones.
/// fn visible(auth: Option<&Authentication>, id: &u64) -> bool {
///     auth.is_some() && id.is_multiple_of(2)
/// }
///
/// struct Notes;
///
/// #[method_security]
/// impl Notes {
///     #[post_authorize(custom = wrote(returned))]
///     async fn open(&self, owner: &str) -> Result<Note, AccessDenied> {
///         if owner.is_empty() {
///             // An early return still passes through the check.
///             return Ok(Note { owner: "nobody".into(), text: String::new() });
///         }
///         Ok(Note { owner: owner.into(), text: "hello".into() })
///     }
///
///     #[pre_filter(arg = ids, custom = visible)]
///     fn delete(&self, ids: Vec<u64>) -> usize {
///         ids.len()
///     }
///
///     #[post_filter(custom = visible)]
///     fn list(&self) -> Result<Vec<u64>, AccessDenied> {
///         Ok(vec![1, 2, 3, 4])
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let bob = SecurityContext::authenticated(Authentication::new("bob", []));
/// let mine = SecurityContext::scope(bob.clone(), Notes.open("bob")).await;
/// assert_eq!(mine.map(|n| n.text).as_deref(), Ok("hello"));
/// let theirs = SecurityContext::scope(bob.clone(), Notes.open("alice")).await;
/// assert_eq!(theirs, Err(AccessDenied::Forbidden));
///
/// let deleted = SecurityContext::scope(bob.clone(), async { Notes.delete(vec![1, 2, 4]) });
/// assert_eq!(deleted.await, 2);
/// let listed = SecurityContext::scope(bob, async { Notes.list() }).await;
/// assert_eq!(listed, Ok(vec![2, 4]));
///
/// // A filter never denies: an anonymous caller gets an empty list.
/// assert_eq!(Notes.list(), Ok(vec![]));
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// On a controller, a denial becomes a 401 or 403 answer:
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_security::{SecurityContext, method_security};
/// use oxbloom_web::{App, HttpError, HttpMethod, Request, controller};
///
/// struct Admin;
///
/// #[controller("/admin")]
/// #[method_security]
/// impl Admin {
///     #[get("/stats")]
///     #[pre_authorize(role = "ADMIN")]
///     async fn stats(&self, _req: Request) -> Result<&'static str, HttpError> {
///         Ok("42 users")
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = App::new().routes(Arc::new(Admin).routes())?;
/// let res = SecurityContext::scope(
///     SecurityContext::anonymous(),
///     app.handle(Request::new(HttpMethod::Get, "/admin/stats")),
/// )
/// .await;
/// assert_eq!(res.status(), 401);
/// # Ok(())
/// # }
/// ```
#[proc_macro_attribute]
pub fn method_security(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    method_security::expand(args.into(), item)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Defines the method attributes of `#[method_security]`. Outside such an impl each one is a
/// compile error; the item stays, so the misplaced attribute is the only error.
macro_rules! method_security_attr {
    ($($(#[$doc:meta])* $name:ident;)*) => {$(
        $(#[$doc])*
        #[proc_macro_attribute]
        pub fn $name(_args: TokenStream, input: TokenStream) -> TokenStream {
            let input = TokenStream2::from(input);
            // The call site of an attribute macro is the attribute itself.
            let error = Error::new(
                proc_macro2::Span::call_site(),
                concat!("#[", stringify!($name), "] is only valid inside a #[method_security] impl"),
            )
            .into_compile_error();
            quote! { #error #input }.into()
        }
    )*};
}

method_security_attr! {
    /// Checks a rule before the method body runs; only valid on a method of a
    /// `#[method_security]` impl, which lists the rules.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{AccessDenied, Authentication, SecurityContext, method_security};
    ///
    /// struct Profile;
    ///
    /// #[method_security]
    /// impl Profile {
    ///     #[pre_authorize(fully_authenticated)]
    ///     fn change_password(&self) -> Result<(), AccessDenied> {
    ///         Ok(())
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let remembered = Authentication::new("ada", []).remembered();
    /// let ctx = SecurityContext::authenticated(remembered);
    /// let result = SecurityContext::scope(ctx, async { Profile.change_password() }).await;
    /// assert_eq!(result, Err(AccessDenied::Forbidden));
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pre_authorize;
    /// Checks a rule after the method body returned `Ok(value)`; only valid on a method of a
    /// `#[method_security]` impl, which lists the rules. In `custom = check(returned)` the
    /// name `returned` stands for `&value`. A denial drops the value and returns `Err`; an
    /// `Err` from the body skips the check.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{AccessDenied, Authentication, SecurityContext, method_security};
    ///
    /// /// True when the caller owns the returned account.
    /// fn owns(auth: Option<&Authentication>, owner: &String) -> bool {
    ///     auth.is_some_and(|a| a.principal() == owner)
    /// }
    ///
    /// struct Accounts;
    ///
    /// #[method_security]
    /// impl Accounts {
    ///     #[post_authorize(custom = owns(returned))]
    ///     fn owner_of(&self, id: u64) -> Result<String, AccessDenied> {
    ///         Ok(if id == 1 { "ada".into() } else { "bob".into() })
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let ada = SecurityContext::authenticated(Authentication::new("ada", []));
    /// let own = SecurityContext::scope(ada.clone(), async { Accounts.owner_of(1) }).await;
    /// assert_eq!(own.as_deref(), Ok("ada"));
    /// let other = SecurityContext::scope(ada, async { Accounts.owner_of(2) }).await;
    /// assert_eq!(other, Err(AccessDenied::Forbidden));
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    post_authorize;
    /// Keeps only the elements of a by-value `Vec<T>` parameter for which
    /// `custom(auth, &element)` returns true, before the method body runs; only valid on a
    /// method of a `#[method_security]` impl. It never denies.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Authentication, GrantedAuthority, SecurityContext, method_security};
    ///
    /// /// Administrators may close every account, other callers only ids below 100.
    /// fn may_close(auth: Option<&Authentication>, id: &u64) -> bool {
    ///     auth.is_some_and(|a| a.has_role("ADMIN") || *id < 100)
    /// }
    ///
    /// struct Accounts;
    ///
    /// #[method_security]
    /// impl Accounts {
    ///     #[pre_filter(arg = ids, custom = may_close)]
    ///     fn close_all(&self, ids: Vec<u64>) -> Vec<u64> {
    ///         ids
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let bob = SecurityContext::authenticated(Authentication::new("bob", []));
    /// let closed = SecurityContext::scope(bob, async { Accounts.close_all(vec![7, 700]) });
    /// assert_eq!(closed.await, [7]);
    /// let admin = Authentication::new("ada", [GrantedAuthority::new("ROLE_ADMIN")]);
    /// let ctx = SecurityContext::authenticated(admin);
    /// let closed = SecurityContext::scope(ctx, async { Accounts.close_all(vec![7, 700]) });
    /// assert_eq!(closed.await, [7, 700]);
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pre_filter;
    /// Keeps only the elements of the collection in an `Ok` result for which
    /// `custom(auth, &element)` returns true; only valid on a method of a `#[method_security]`
    /// impl. The collection is a `Vec<T>` or any other type with a `retain` method that takes
    /// `FnMut(&T) -> bool`, such as `HashSet<T>`. It never denies, and an `Err` passes through.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{AccessDenied, Authentication, SecurityContext, method_security};
    ///
    /// /// True for the accounts the caller owns.
    /// fn own(auth: Option<&Authentication>, owner: &&str) -> bool {
    ///     auth.is_some_and(|a| a.principal() == *owner)
    /// }
    ///
    /// struct Accounts;
    ///
    /// #[method_security]
    /// impl Accounts {
    ///     #[post_filter(custom = own)]
    ///     async fn owners(&self) -> Result<Vec<&'static str>, AccessDenied> {
    ///         Ok(vec!["ada", "bob", "ada"])
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let ada = SecurityContext::authenticated(Authentication::new("ada", []));
    /// let mine = SecurityContext::scope(ada, Accounts.owners()).await;
    /// assert_eq!(mine, Ok(vec!["ada", "ada"]));
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    post_filter;
}
