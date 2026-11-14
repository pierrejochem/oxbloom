//! `#[auto_configuration(..)]`: the `#[configuration]` expansion plus
//! `impl ::oxbloom_boot::AutoConfiguration` with the class-level conditions and the ordering.

use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, quote_spanned};
use syn::meta::ParseNestedMeta;
use syn::spanned::Spanned;
use syn::{Error, ItemImpl, Token};

use crate::configuration::expand_impl;
use crate::options::{Condition, parse_condition_type, parse_on_property};

const EXPECTED: &str = "expected `after(Config, ..)`, `before(Config, ..)`, `on_bean(T)`, \
                        `on_missing_bean(T)`, `on_property(\"key\")` or \
                        `on_property(\"key\", \"value\")`";

/// The parsed options of one `#[auto_configuration(..)]`.
#[derive(Default)]
pub(crate) struct ClassOptions {
    after: Option<Vec<syn::Type>>,
    before: Option<Vec<syn::Type>>,
    /// In source order; ANDed by `Conditions`.
    conditions: Vec<Condition>,
}

impl ClassOptions {
    /// Parses one option; every misuse is a spanned error.
    pub(crate) fn parse(&mut self, meta: ParseNestedMeta) -> syn::Result<()> {
        let Some(ident) = meta.path.get_ident().map(ToString::to_string) else {
            return Err(meta.error(format!("unknown #[auto_configuration] option; {EXPECTED}")));
        };
        let duplicate = |meta: &ParseNestedMeta| {
            meta.error(format!("duplicate #[auto_configuration] option `{ident}`"))
        };
        match ident.as_str() {
            "after" | "before" => {
                let slot = if ident == "after" {
                    &mut self.after
                } else {
                    &mut self.before
                };
                if slot.is_some() {
                    return Err(duplicate(&meta));
                }
                *slot = Some(parse_types(&meta, &ident)?);
            }
            "on_bean" => {
                let ty = parse_condition_type(&meta, "on_bean")?
                    .ok_or_else(|| meta.error("`on_bean` needs a type, e.g. `on_bean(Pool)`"))?;
                self.conditions.push(Condition::Bean(ty));
            }
            "on_missing_bean" => {
                let ty = parse_condition_type(&meta, "on_missing_bean")?.ok_or_else(|| {
                    meta.error(
                        "`on_missing_bean` on #[auto_configuration] needs a type, e.g. \
                         `on_missing_bean(Pool)`: there is no return type to default to",
                    )
                })?;
                self.conditions
                    .push(Condition::MissingBean(Some(ty), meta.path.span()));
            }
            "on_property" => self.conditions.push(parse_on_property(&meta)?),
            _ => {
                return Err(meta.error(format!("unknown #[auto_configuration] option; {EXPECTED}")));
            }
        }
        Ok(())
    }
}

/// `after(A, B)` / `before(C)`: at least one type, a trailing comma allowed.
fn parse_types(meta: &ParseNestedMeta, name: &str) -> syn::Result<Vec<syn::Type>> {
    let usage =
        format!("`{name}` takes one or more configuration types, e.g. `{name}(DataSourceConfig)`");
    if !meta.input.peek(syn::token::Paren) {
        return Err(meta.error(usage));
    }
    let content;
    let parens = syn::parenthesized!(content in meta.input);
    let types = content
        .parse_terminated(<syn::Type as syn::parse::Parse>::parse, Token![,])
        .map_err(|e| Error::new(e.span(), &usage))?;
    if types.is_empty() {
        return Err(Error::new(parens.span.join(), usage));
    }
    for ty in &types {
        if matches!(ty, syn::Type::ImplTrait(_) | syn::Type::TraitObject(_)) {
            return Err(Error::new_spanned(ty, usage));
        }
    }
    Ok(types.into_iter().collect())
}

pub(crate) fn expand(options: ClassOptions, item: ItemImpl) -> syn::Result<TokenStream2> {
    let configuration = expand_impl("auto_configuration", item.clone())?;
    let self_ty = &item.self_ty;

    let conditions = (!options.conditions.is_empty()).then(|| {
        let calls = options.conditions.iter().map(|c| c.modifier(None));
        quote! {
            fn conditions() -> ::oxbloom_boot::Conditions {
                ::oxbloom_boot::Conditions::new() #(#calls)*
            }
        }
    });
    let list = |name: &str, types: &Option<Vec<syn::Type>>| {
        types.as_ref().map(|types| {
            let name = syn::Ident::new(name, proc_macro2::Span::call_site());
            // Spanned at the type, so an unknown or unsized type points there.
            let refs = types
                .iter()
                .map(|ty| quote_spanned! {ty.span()=> ::oxbloom_boot::ConfigRef::of::<#ty>() });
            quote! {
                fn #name() -> ::std::vec::Vec<::oxbloom_boot::ConfigRef> {
                    ::std::vec![#(#refs),*]
                }
            }
        })
    };
    let after = list("after", &options.after);
    let before = list("before", &options.before);

    Ok(quote! {
        #configuration

        impl ::oxbloom_boot::AutoConfiguration for #self_ty {
            #conditions
            #after
            #before
        }
    })
}
