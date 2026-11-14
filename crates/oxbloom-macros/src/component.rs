//! `#[component]`: `impl Component` for a struct of injected fields.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Error, Ident};

use crate::options::Options;

/// How an injected field or parameter is resolved.
#[derive(Clone, Copy)]
enum Shape {
    /// `Arc<T>` / `Arc<dyn T>`: `Resolver::get`.
    One,
    /// `Option<Arc<..>>`: `Resolver::get_opt`.
    Opt,
    /// `Vec<Arc<..>>`: `Resolver::get_all`.
    All,
}

/// The single generic type argument of `ty` when its last path segment is `name`.
fn generic_arg<'t>(ty: &'t syn::Type, name: &str) -> Option<&'t syn::Type> {
    let syn::Type::Path(p) = ty else { return None };
    if p.qself.is_some() {
        return None;
    }
    let last = p.path.segments.last()?;
    if last.ident != name {
        return None;
    }
    let syn::PathArguments::AngleBracketed(a) = &last.arguments else {
        return None;
    };
    match (a.args.len(), a.args.first()) {
        (1, Some(syn::GenericArgument::Type(t))) => Some(t),
        _ => None,
    }
}

fn shape(ty: &syn::Type) -> Option<Shape> {
    if generic_arg(ty, "Arc").is_some() {
        return Some(Shape::One);
    }
    let arc = |inner: Option<&syn::Type>| inner.and_then(|t| generic_arg(t, "Arc")).is_some();
    if arc(generic_arg(ty, "Option")) {
        Some(Shape::Opt)
    } else if arc(generic_arg(ty, "Vec")) {
        Some(Shape::All)
    } else {
        None
    }
}

/// `#[cfg]` / `#[cfg_attr]` among `attrs`, which a macro cannot evaluate.
pub(crate) fn cfg_attr(attrs: &[syn::Attribute]) -> Option<&syn::Attribute> {
    attrs
        .iter()
        .find(|a| a.path().is_ident("cfg") || a.path().is_ident("cfg_attr"))
}

pub(crate) fn expand(options: Options, item: syn::Item) -> syn::Result<TokenStream2> {
    let syn::Item::Struct(s) = &item else {
        return Err(Error::new(
            item.span(),
            "#[component] can only be used on structs",
        ));
    };
    if !s.generics.params.is_empty() {
        return Err(Error::new(
            s.generics.span(),
            "#[component] does not support generic structs",
        ));
    }
    // Mixed-site and `__oxbloom_`-prefixed: mixed-site hygiene keeps fields and user locals
    // apart, the prefix keeps unit structs and consts in scope (which hygiene does not
    // separate) from turning a binding into a pattern.
    let site = Span::mixed_site();
    let (r, v, e) = (
        Ident::new("__oxbloom_r", site),
        Ident::new("__oxbloom_v", site),
        Ident::new("__oxbloom_e", site),
    );
    let value = match &s.fields {
        syn::Fields::Unit => quote! { ::core::result::Result::Ok(Self) },
        syn::Fields::Named(fields) => {
            let mut inits = Vec::new();
            for f in &fields.named {
                if let Some(attr) = cfg_attr(&f.attrs) {
                    return Err(Error::new_spanned(
                        attr,
                        "#[component] does not support `#[cfg]` on fields; make the dependency \
                         an `Option<Arc<..>>` or write `impl Component` by hand",
                    ));
                }
                let method = match shape(&f.ty) {
                    Some(Shape::One) => quote! { get },
                    Some(Shape::Opt) => quote! { get_opt },
                    Some(Shape::All) => quote! { get_all },
                    None => {
                        return Err(Error::new(
                            f.ty.span(),
                            "#[component] fields must be `Arc<T>`, `Arc<dyn Trait>`, \
                             `Option<Arc<..>>` or `Vec<Arc<..>>` (injected dependencies)",
                        ));
                    }
                };
                let name = f.ident.as_ref().expect("named field");
                // Each field is initialised in place, in declaration order, without a local
                // named after the field: `let #name = ..` would be a pattern if a unit struct or
                // const named like the field is in scope. `match` instead of `?`: in debug builds every temporary of
                // `?` (`branch`, `from_residual`) gets its own stack slot, and `create` sits on
                // the recursion path of a deep build.
                inits.push(quote! {
                    #name: match #r.#method() {
                        ::core::result::Result::Ok(#v) => #v,
                        ::core::result::Result::Err(#e) => return ::core::result::Result::Err(#e),
                    }
                });
            }
            quote! { ::core::result::Result::Ok(Self { #(#inits),* }) }
        }
        syn::Fields::Unnamed(_) => {
            return Err(Error::new(
                s.span(),
                "#[component] requires a struct with named fields or a unit struct",
            ));
        }
    };
    let name = &s.ident;
    let configure = options.has_modifiers().then(|| {
        let modifiers = options.modifiers(&quote! { Self });
        let builder = Ident::new("__oxbloom_builder", site);
        quote! {
            fn configure(
                #builder: ::oxbloom_di::ContainerBuilder,
            ) -> ::oxbloom_di::ContainerBuilder {
                #builder #modifiers
            }
        }
    });
    Ok(quote! {
        #item

        impl ::oxbloom_di::Component for #name {
            fn create(
                #r: &::oxbloom_di::Resolver,
            ) -> ::core::result::Result<Self, ::oxbloom_di::DiError> {
                #value
            }
            #configure
        }
    })
}
