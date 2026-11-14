//! `#[scheduling]` and its `#[scheduled]` methods: `impl Scheduling` returning one task per
//! method.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::meta::ParseNestedMeta;
use syn::{Error, FnArg, Ident, ImplItem, ItemImpl, LitStr, ReceiverKind, ReturnType, Type};

/// The keys `#[scheduled(..)]` accepts.
const TRIGGERS: [&str; 3] = ["cron", "fixed_rate", "fixed_delay"];

/// Whether `attr` is `#[scheduled]` (also `#[oxbloom_tasks::scheduled]` /
/// `#[oxbloom_macros::scheduled]`).
fn is_scheduled(attr: &syn::Attribute) -> bool {
    let segments = &attr.path().segments;
    let last_is_scheduled = segments.last().is_some_and(|s| s.ident == "scheduled");
    last_is_scheduled
        && (segments.len() == 1
            || (segments.len() == 2
                && (segments[0].ident == "oxbloom_tasks" || segments[0].ident == "oxbloom_macros")))
}

/// The attributes of an impl item that is not a method, to reject `#[scheduled]` on it.
fn attrs_of(item: &ImplItem) -> &[syn::Attribute] {
    match item {
        ImplItem::Const(c) => &c.attrs,
        ImplItem::Type(t) => &t.attrs,
        ImplItem::Macro(m) => &m.attrs,
        _ => &[],
    }
}

/// The arguments of one `#[scheduled(..)]` attribute.
#[derive(Default)]
struct Args {
    /// The trigger key (`cron`, `fixed_rate` or `fixed_delay`) and its value.
    trigger: Option<(Ident, LitStr)>,
    initial_delay: Option<LitStr>,
    zone: Option<(Ident, LitStr)>,
    name: Option<LitStr>,
}

impl Args {
    fn parse(&mut self, meta: ParseNestedMeta<'_>, method: &Ident) -> syn::Result<()> {
        let Some(key) = meta.path.get_ident().cloned() else {
            return Err(meta.error(unknown_key(method)));
        };
        let key_name = key.to_string();
        let value = || -> syn::Result<LitStr> {
            meta.value().and_then(|v| v.parse::<LitStr>()).map_err(|_| {
                Error::new(
                    key.span(),
                    format!(
                        "`{key_name}` on the scheduled method `{method}` takes a string, \
                         e.g. `{key_name} = \"..\"`"
                    ),
                )
            })
        };
        let repeated = || {
            Error::new(
                key.span(),
                format!("`{key_name}` is given twice on the scheduled method `{method}`"),
            )
        };
        if TRIGGERS.contains(&key_name.as_str()) {
            if let Some((first, _)) = &self.trigger {
                return Err(Error::new(
                    key.span(),
                    format!(
                        "the scheduled method `{method}` has two triggers, `{first}` and \
                         `{key_name}`; use exactly one of `cron`, `fixed_rate` or `fixed_delay`"
                    ),
                ));
            }
            self.trigger = Some((key.clone(), value()?));
            return Ok(());
        }
        match key_name.as_str() {
            "initial_delay" => {
                if self.initial_delay.is_some() {
                    return Err(repeated());
                }
                self.initial_delay = Some(value()?);
            }
            "zone" => {
                if self.zone.is_some() {
                    return Err(repeated());
                }
                self.zone = Some((key.clone(), value()?));
            }
            "name" => {
                if self.name.is_some() {
                    return Err(repeated());
                }
                let name = value()?;
                if name.value().is_empty() {
                    return Err(Error::new(
                        name.span(),
                        format!("the name of the scheduled method `{method}` cannot be empty"),
                    ));
                }
                self.name = Some(name);
            }
            _ => return Err(Error::new(key.span(), unknown_key(method))),
        }
        Ok(())
    }
}

fn unknown_key(method: &Ident) -> String {
    format!(
        "unknown key on the scheduled method `{method}`; the keys are `cron`, `fixed_rate`, \
         `fixed_delay`, `initial_delay`, `zone` and `name`"
    )
}

pub(crate) fn expand(args: TokenStream2, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    if !args.is_empty() {
        return Err(Error::new_spanned(
            &args,
            "#[scheduling] takes no arguments",
        ));
    }
    if let Some((path, _)) = &item.trait_ {
        return Err(Error::new_spanned(
            path,
            "#[scheduling] requires an inherent impl such as `impl Reports { .. }`, not a trait \
             impl",
        ));
    }
    let type_name = type_name(&item.self_ty)?;

    let mut tasks = Vec::new();
    let mut errors: Option<Error> = None;
    let mut push = |error: Error| match &mut errors {
        Some(all) => all.combine(error),
        None => errors = Some(error),
    };
    for it in &mut item.items {
        let ImplItem::Fn(f) = it else {
            if let Some(attr) = attrs_of(it).iter().find(|a| is_scheduled(a)) {
                push(Error::new_spanned(
                    attr,
                    "#[scheduled] can only be used on methods of a #[scheduling] impl",
                ));
            }
            continue;
        };
        let (scheduled, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut f.attrs)
            .into_iter()
            .partition(is_scheduled);
        f.attrs = kept;
        let Some(attr) = scheduled.first() else {
            continue;
        };
        if let Some(second) = scheduled.get(1) {
            push(Error::new_spanned(
                second,
                format!(
                    "the method `{}` has two #[scheduled] attributes; use one per method",
                    f.sig.ident
                ),
            ));
            continue;
        }
        match task(&type_name, attr, &f.sig) {
            Ok(task) => tasks.push(task),
            Err(error) => push(error),
        }
    }
    if let Some(errors) = errors {
        return Err(errors);
    }

    let self_ty = &item.self_ty;
    let (impl_generics, _, where_clause) = item.generics.split_for_impl();
    let this = Ident::new("__oxbloom_this", Span::mixed_site());
    let tasks = tasks.iter().map(|task| task(&this));
    Ok(quote! {
        #item

        impl #impl_generics ::oxbloom_tasks::Scheduling for #self_ty #where_clause {
            fn scheduled(
                self: ::std::sync::Arc<Self>,
            ) -> ::std::vec::Vec<::oxbloom_tasks::Task> {
                let #this = self;
                ::std::vec![#(#tasks),*]
            }
        }
    })
}

/// The last segment of the impl's type, for the default task name `Type::method`.
fn type_name(ty: &Type) -> syn::Result<String> {
    match ty {
        Type::Path(p) if p.qself.is_none() => {
            if let Some(last) = p.path.segments.last() {
                return Ok(last.ident.to_string());
            }
        }
        Type::Group(g) => return type_name(&g.elem),
        Type::Paren(p) => return type_name(&p.elem),
        _ => {}
    }
    Err(Error::new_spanned(
        ty,
        "#[scheduling] requires an impl of a named type such as `impl Reports { .. }`",
    ))
}

/// The generated expression of one task, given the identifier of the `Arc<Self>` to clone.
type TaskTokens = Box<dyn Fn(&Ident) -> TokenStream2>;

/// Checks one `#[scheduled]` method and returns its task expression.
fn task(type_name: &str, attr: &syn::Attribute, sig: &syn::Signature) -> syn::Result<TaskTokens> {
    let method = sig.ident.clone();
    let mut args = Args::default();
    match &attr.meta {
        syn::Meta::List(_) => attr.parse_nested_meta(|meta| args.parse(meta, &method))?,
        syn::Meta::Path(_) => {}
        syn::Meta::NameValue(nv) => {
            return Err(Error::new_spanned(
                nv,
                format!(
                    "#[scheduled] on `{method}` takes its keys in parentheses, e.g. \
                     `#[scheduled(fixed_rate = \"30s\")]`"
                ),
            ));
        }
    }
    let Some((trigger_key, trigger_value)) = args.trigger else {
        return Err(Error::new_spanned(
            attr,
            format!(
                "the scheduled method `{method}` has no trigger; add exactly one of `cron`, \
                 `fixed_rate` or `fixed_delay`"
            ),
        ));
    };
    if let Some((zone_key, _)) = &args.zone
        && trigger_key != "cron"
    {
        return Err(Error::new(
            zone_key.span(),
            format!(
                "`zone` only applies to a `cron` trigger, and the scheduled method `{method}` \
                 uses `{trigger_key}`"
            ),
        ));
    }
    check_signature(sig)?;

    let spec = match trigger_key.to_string().as_str() {
        "cron" => {
            let zone = match &args.zone {
                Some((_, zone)) => quote! { ::std::option::Option::Some(#zone) },
                None => quote! { ::std::option::Option::None },
            };
            quote! { ::oxbloom_tasks::__private::Spec::Cron(#trigger_value, #zone) }
        }
        "fixed_rate" => quote! { ::oxbloom_tasks::__private::Spec::FixedRate(#trigger_value) },
        _ => quote! { ::oxbloom_tasks::__private::Spec::FixedDelay(#trigger_value) },
    };
    let name = match &args.name {
        Some(name) => name.value(),
        None => format!("{type_name}::{method}"),
    };
    let initial_delay = match &args.initial_delay {
        Some(delay) => quote! { ::std::option::Option::Some(#delay) },
        None => quote! { ::std::option::Option::None },
    };
    let is_async = sig.asyncness.is_some();
    let span = sig.ident.span();
    Ok(Box::new(move |this: &Ident| {
        let this_run = Ident::new("__oxbloom_run", Span::mixed_site());
        let body = if is_async {
            quote_spanned! {span=>
                ::std::boxed::Box::pin(async move {
                    ::oxbloom_tasks::__private::RunResult::into_run_result(
                        #this_run.#method().await,
                    )
                })
            }
        } else {
            quote_spanned! {span=>
                ::oxbloom_tasks::__private::blocking(move || {
                    ::oxbloom_tasks::__private::RunResult::into_run_result(#this_run.#method())
                })
            }
        };
        quote! {{
            let #this_run = ::std::sync::Arc::clone(&#this);
            ::oxbloom_tasks::__private::task(
                ::std::string::String::from(#name),
                #spec,
                #initial_delay,
                move || {
                    let #this_run = ::std::sync::Arc::clone(&#this_run);
                    #body
                },
            )
        }}
    }))
}

/// A scheduled method takes only `&self` and returns `()` or `Result<(), E>`.
fn check_signature(sig: &syn::Signature) -> syn::Result<()> {
    let method = &sig.ident;
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        return Err(Error::new_spanned(
            &sig.generics,
            format!("the scheduled method `{method}` cannot be generic"),
        ));
    }
    if let Some(variadic) = &sig.variadic {
        return Err(Error::new_spanned(
            variadic,
            format!("the scheduled method `{method}` cannot be variadic"),
        ));
    }
    let receiver_message = format!(
        "the scheduled method `{method}` must take `&self`: the scheduler calls it on the shared \
         instance"
    );
    match sig.inputs.first() {
        Some(FnArg::Receiver(rec)) => {
            let by_ref = rec.mutability.is_none()
                && match &rec.kind {
                    ReceiverKind::Reference(_, None, None) => true,
                    ReceiverKind::Typed(_, ty) => is_ref_self(ty),
                    _ => false,
                };
            if !by_ref {
                return Err(Error::new_spanned(rec, receiver_message));
            }
        }
        _ => return Err(Error::new(method.span(), receiver_message)),
    }
    if let Some(extra) = sig.inputs.iter().nth(1) {
        return Err(Error::new_spanned(
            extra,
            format!(
                "the scheduled method `{method}` takes no arguments besides `&self`; the \
                 scheduler has nothing to pass"
            ),
        ));
    }
    if let ReturnType::Type(_, ty) = &sig.output
        && !is_unit(ty)
        && !is_result(ty)
    {
        return Err(Error::new_spanned(
            ty,
            format!(
                "the scheduled method `{method}` must return `()` or `Result<(), E>` where \
                 `E: Display`"
            ),
        ));
    }
    Ok(())
}

/// Whether `ty` is `&Self` (no lifetime, not `mut`).
fn is_ref_self(ty: &Type) -> bool {
    let Type::Reference(r) = ty else {
        return false;
    };
    r.lifetime.is_none()
        && r.mutability.is_none()
        && matches!(&*r.elem, Type::Path(p) if p.qself.is_none() && p.path.is_ident("Self"))
}

fn is_unit(ty: &Type) -> bool {
    match ty {
        Type::Tuple(t) => t.elems.is_empty(),
        Type::Paren(p) => is_unit(&p.elem),
        Type::Group(g) => is_unit(&g.elem),
        _ => false,
    }
}

/// Whether `ty` is a path ending in `Result<..>`: `Result<(), E>`, or an alias such as
/// `io::Result<()>`. That the success type is `()` and the error is `Display` is checked by the
/// compiler through `RunResult`.
fn is_result(ty: &Type) -> bool {
    match ty {
        Type::Path(p) if p.qself.is_none() => p.path.segments.last().is_some_and(|last| {
            last.ident == "Result"
                && matches!(&last.arguments, syn::PathArguments::AngleBracketed(a) if (1..=2).contains(&a.args.len()))
        }),
        Type::Paren(p) => is_result(&p.elem),
        Type::Group(g) => is_result(&g.elem),
        _ => false,
    }
}
