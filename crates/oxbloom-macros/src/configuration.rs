//! `#[configuration]` and its `#[bean]` methods: `impl Configuration` registering one bean per
//! method.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Error, FnArg, Ident, ImplItem, ItemImpl, ReceiverKind, ReturnType, Safety};

use crate::component::cfg_attr;
use crate::options::{Kind, Options};

/// Whether `attr` is `#[bean]` (also `#[oxbloom_di::bean]` / `#[oxbloom_macros::bean]`).
fn is_bean(attr: &syn::Attribute) -> bool {
    let segments = &attr.path().segments;
    let last_is_bean = segments.last().is_some_and(|s| s.ident == "bean");
    last_is_bean
        && (segments.len() == 1
            || (segments.len() == 2
                && (segments[0].ident == "oxbloom_di" || segments[0].ident == "oxbloom_macros")))
}

/// The attributes of an impl item that is not a method, to reject `#[bean]` on it.
fn attrs_of(item: &ImplItem) -> &[syn::Attribute] {
    match item {
        ImplItem::Const(c) => &c.attrs,
        ImplItem::Type(t) => &t.attrs,
        ImplItem::Macro(m) => &m.attrs,
        ImplItem::Fn(f) => &f.attrs,
        _ => &[],
    }
}

pub(crate) fn expand(args: TokenStream2, item: ItemImpl) -> syn::Result<TokenStream2> {
    if !args.is_empty() {
        return Err(Error::new_spanned(
            &args,
            "#[configuration] takes no arguments",
        ));
    }
    expand_impl("configuration", item)
}

/// The `impl Configuration` expansion shared by `#[configuration]` and `#[auto_configuration]`:
/// the item with its `#[bean]` attributes consumed, then the impl. `macro_name` names the attribute
/// in the messages about the impl itself.
pub(crate) fn expand_impl(macro_name: &str, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    if let Some((path, _)) = &item.trait_ {
        return Err(Error::new_spanned(
            path,
            format!(
                "#[{macro_name}] requires an inherent impl such as `impl AppConfig {{ .. }}`, not a \
                 trait impl"
            ),
        ));
    }
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(Error::new_spanned(
            &item.generics,
            format!("#[{macro_name}] does not support generic impls"),
        ));
    }

    let site = Span::mixed_site();
    let builder = Ident::new("__oxbloom_builder", site);
    let mut registrations = Vec::new();
    for it in &mut item.items {
        let ImplItem::Fn(f) = it else {
            if let Some(attr) = attrs_of(it).iter().find(|a| is_bean(a)) {
                let article = if macro_name.starts_with(['a', 'e', 'i', 'o', 'u']) {
                    "an"
                } else {
                    "a"
                };
                return Err(Error::new_spanned(
                    attr,
                    format!(
                        "#[bean] can only be used on methods of {article} #[{macro_name}] impl"
                    ),
                ));
            }
            continue;
        };
        let (beans, kept): (Vec<_>, Vec<_>) =
            std::mem::take(&mut f.attrs).into_iter().partition(is_bean);
        f.attrs = kept;
        let Some(attr) = beans.first() else { continue };
        // A `#[cfg]` on the method also guards its registration: a method compiled out takes
        // its bean with it instead of leaving a call to a missing method.
        let cfgs: Vec<_> = f.attrs.iter().filter_map(registration_cfg).collect();
        if let Some(second) = beans.get(1) {
            return Err(Error::new_spanned(second, "duplicate #[bean] attribute"));
        }
        let mut options = Options::new(Kind::Bean);
        match &attr.meta {
            syn::Meta::Path(_) => {}
            syn::Meta::List(_) => attr.parse_nested_meta(|meta| options.parse(meta))?,
            syn::Meta::NameValue(nv) => {
                return Err(Error::new_spanned(
                    nv,
                    "#[bean] takes options in parentheses, e.g. `#[bean(lazy, primary)]`",
                ));
            }
        }
        let registration = bean(&builder, &f.sig, &options)?;
        registrations.push(quote! { #(#cfgs)* let #builder = #registration; });
    }

    let self_ty = &item.self_ty;
    Ok(quote! {
        #item

        impl ::oxbloom_di::Configuration for #self_ty {
            fn register(
                #builder: ::oxbloom_di::ContainerBuilder,
            ) -> ::oxbloom_di::ContainerBuilder {
                #(#registrations)*
                #builder
            }
        }
    })
}

/// The registration of one `#[bean]` method: `__private::bean` with an adapter closure that
/// resolves `Arc<Self>` and the parameters, then calls the method, plus the option modifiers.
fn bean(builder: &Ident, sig: &syn::Signature, options: &Options) -> syn::Result<TokenStream2> {
    check_signature(sig)?;
    let ret = match &sig.output {
        ReturnType::Type(_, ty) => ty,
        ReturnType::Default => {
            return Err(Error::new(
                sig.ident.span(),
                "a #[bean] method must return the bean, e.g. `fn pool(&self) -> Pool`",
            ));
        }
    };
    if let syn::Type::ImplTrait(_) = **ret {
        return Err(Error::new_spanned(
            ret,
            "a #[bean] method must name its return type: the type is the bean's key, so \
             `impl Trait` cannot be one; return the concrete type and add `provides(dyn Trait)`",
        ));
    }

    let site = Span::mixed_site();
    // Mixed-site and `__oxbloom_`-prefixed: mixed-site hygiene keeps user locals apart, the
    // prefix keeps unit structs and consts in scope (which hygiene does not separate) from
    // turning a binding into a pattern.
    let (r, this, v, e) = (
        Ident::new("__oxbloom_r", site),
        Ident::new("__oxbloom_this", site),
        Ident::new("__oxbloom_v", site),
        Ident::new("__oxbloom_e", site),
    );
    let mut injects = Vec::new();
    let mut locals = Vec::new();
    for (i, arg) in sig.inputs.iter().skip(1).enumerate() {
        let FnArg::Typed(arg) = arg else {
            return Err(Error::new(arg.span(), "unexpected receiver"));
        };
        if let Some(attr) = cfg_attr(&arg.attrs) {
            return Err(Error::new_spanned(
                attr,
                "#[bean] does not support `#[cfg]` on parameters; take an `Option<Arc<..>>` or \
                 put the `#[cfg]` on the method",
            ));
        }
        let ty = &arg.ty;
        if let syn::Type::ImplTrait(_) = **ty {
            return Err(Error::new_spanned(
                ty,
                "#[bean] parameters cannot be `impl Trait`; take `Arc<dyn Trait>`",
            ));
        }
        let local = format_ident!("__oxbloom_a{}", i, span = site);
        // The path is spanned at the parameter type: a type that is not an injectable shape is
        // reported there ("cannot be injected into a bean"). The `match` keeps the macro's span,
        // so lints about it (`question_mark`) do not fire in the user's code.
        let inject = quote_spanned! {ty.span()=> <#ty as ::oxbloom_di::Inject>::inject };
        injects.push(quote! {
            let #local = match #inject(#r) {
                ::core::result::Result::Ok(#v) => #v,
                ::core::result::Result::Err(#e) => return ::core::result::Result::Err(#e),
            };
        });
        locals.push(local);
    }

    let method = &sig.ident;
    let call = quote! { Self::#method(&*#this, #(#locals),*) };
    let (key, value) = if options.fallible.is_some() {
        let key = quote_spanned! {ret.span()=>
            <#ret as ::oxbloom_di::__private::Fallible>::Value
        };
        let into_result = quote_spanned! {ret.span()=>
            ::oxbloom_di::__private::Fallible::into_result
        };
        // The whole call is spanned at the return type, so a non-`Result` is reported there.
        let converted = quote_spanned! {ret.span()=>
            #into_result(Self::#method(&*#this, #(#locals),*))
        };
        let value = quote! {
            match #converted {
                ::core::result::Result::Ok(#v) => ::core::result::Result::Ok(#v),
                ::core::result::Result::Err(#e) => ::core::result::Result::Err(
                    ::oxbloom_di::__private::bean_failed::<#key>(#e),
                ),
            }
        };
        (key, value)
    } else {
        (
            quote! { #ret },
            quote! { ::core::result::Result::Ok(#call) },
        )
    };
    let name = method.to_string();
    let modifiers = options.modifiers(&key);
    Ok(quote! {
        ::oxbloom_di::__private::bean::<Self, #key, _>(
            #builder,
            #name,
            |#r: &::oxbloom_di::Resolver<'_>|
                -> ::core::result::Result<#key, ::oxbloom_di::DiError> {
                // `match` instead of `?`: the adapter sits on the recursion path of a deep build.
                let #this: ::std::sync::Arc<Self> = match #r.get::<Self>() {
                    ::core::result::Result::Ok(#v) => #v,
                    ::core::result::Result::Err(#e) => return ::core::result::Result::Err(#e),
                };
                #(#injects)*
                #value
            },
        ) #modifiers
    })
}

/// A `#[bean]` method is a plain `&self` (or `self: &Self`) method: not `async`, `unsafe` or
/// generic.
fn check_signature(sig: &syn::Signature) -> syn::Result<()> {
    if let Some(token) = &sig.asyncness {
        return Err(Error::new(
            token.span(),
            "a #[bean] method cannot be `async`; beans are created synchronously",
        ));
    }
    if let Safety::Unsafe(token) = &sig.safety {
        return Err(Error::new(
            token.span(),
            "a #[bean] method cannot be `unsafe`",
        ));
    }
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        return Err(Error::new_spanned(
            &sig.generics,
            "a #[bean] method cannot be generic; the bean's type must be known",
        ));
    }
    if let Some(variadic) = &sig.variadic {
        return Err(Error::new_spanned(
            variadic,
            "a #[bean] method cannot be variadic",
        ));
    }
    let by_ref = match sig.inputs.first() {
        Some(FnArg::Receiver(rec)) if rec.mutability.is_none() => match &rec.kind {
            ReceiverKind::Reference(_, None, None) => true,
            // `self: &Self` is `&self` spelled out.
            ReceiverKind::Typed(_, ty) => is_ref_self(ty),
            _ => false,
        },
        _ => false,
    };
    if !by_ref {
        let message = "a #[bean] method must take `&self`: the configuration component is \
                       resolved and the method called on it";
        return Err(match sig.inputs.first() {
            Some(FnArg::Receiver(rec)) => Error::new_spanned(rec, message),
            _ => Error::new(sig.ident.span(), message),
        });
    }
    Ok(())
}

/// Whether `ty` is `&Self` (no lifetime, not `mut`).
fn is_ref_self(ty: &syn::Type) -> bool {
    let syn::Type::Reference(r) = ty else {
        return false;
    };
    r.lifetime.is_none()
        && r.mutability.is_none()
        && matches!(&*r.elem, syn::Type::Path(p) if p.qself.is_none() && p.path.is_ident("Self"))
}

/// The part of a method attribute that also applies to its registration statement: `#[cfg(..)]`
/// as is, and `#[cfg_attr(pred, ..)]` reduced to the `cfg(..)` it adds (its other attributes,
/// such as `inline`, belong to the method only).
fn registration_cfg(attr: &syn::Attribute) -> Option<TokenStream2> {
    if attr.path().is_ident("cfg") {
        return Some(quote! { #attr });
    }
    if !attr.path().is_ident("cfg_attr") {
        return None;
    }
    let args = attr
        .parse_args_with(syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated)
        .ok()?;
    let mut args = args.into_iter();
    let predicate = args.next()?;
    let cfgs: Vec<_> = args.filter(|m| m.path().is_ident("cfg")).collect();
    (!cfgs.is_empty()).then(|| quote! { #[cfg_attr(#predicate, #(#cfgs),*)] })
}
