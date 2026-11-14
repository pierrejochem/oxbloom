//! The registration options shared by `#[component(..)]` and `#[bean(..)]`, and the builder
//! modifiers they become.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::meta::ParseNestedMeta;
use syn::spanned::Spanned;
use syn::{Error, LitStr, Token};

/// Which attribute the options belong to; decides the accepted options and the messages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Component,
    Bean,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Component => "component",
            Kind::Bean => "bean",
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Kind::Component => {
                "expected `provides(dyn Trait, ..)`, `primary`, `order = n`, `lazy`, `lifecycle` \
                 or `profile = \"name\"`"
            }
            Kind::Bean => {
                "expected `fallible`, `provides(dyn Trait, ..)`, `primary`, `order = n`, `lazy`, \
                 `profile = \"name\"`, `on_bean(T)`, `on_missing_bean`, `on_missing_bean(T)`, \
                 `on_property(\"key\")` or `on_property(\"key\", \"value\")`"
            }
        }
    }
}

/// The parsed options of one `#[component(..)]` or `#[bean(..)]`.
pub(crate) struct Options {
    kind: Kind,
    pub(crate) provides: Option<Vec<syn::Type>>,
    primary: Option<Span>,
    order: Option<syn::Expr>,
    lazy: Option<Span>,
    /// `#[component]` only: register the hooks of `impl Lifecycle for Self`.
    lifecycle: Option<Span>,
    /// `#[bean]` only: the method returns `Result<T, DiError>`.
    pub(crate) fallible: Option<Span>,
    profile: Option<Vec<LitStr>>,
    /// `#[bean]` only: the condition options in source order (ANDed by the builder).
    conditions: Vec<Condition>,
}

/// One condition option of `#[bean(..)]` or `#[auto_configuration(..)]`; each becomes one
/// modifier of the same name (on the builder, or on `Conditions`).
pub(crate) enum Condition {
    /// `on_bean(T)`.
    Bean(syn::Type),
    /// `on_missing_bean(T)`, or `on_missing_bean` (the bean's own key, spanned at the option).
    MissingBean(Option<syn::Type>, Span),
    /// `on_property("key")`.
    Property(LitStr),
    /// `on_property("key", "value")`.
    PropertyValue(LitStr, LitStr),
}

impl Options {
    pub(crate) fn new(kind: Kind) -> Self {
        Options {
            kind,
            provides: None,
            primary: None,
            order: None,
            lazy: None,
            lifecycle: None,
            fallible: None,
            profile: None,
            conditions: Vec::new(),
        }
    }

    /// Parses one option; every misuse is a spanned error.
    pub(crate) fn parse(&mut self, meta: ParseNestedMeta) -> syn::Result<()> {
        let what = self.kind.name();
        let Some(ident) = meta.path.get_ident().map(ToString::to_string) else {
            return Err(meta.error(format!(
                "unknown #[{what}] option; {}",
                self.kind.expected()
            )));
        };
        let duplicate =
            |meta: &ParseNestedMeta| meta.error(format!("duplicate #[{what}] option `{ident}`"));
        match (ident.as_str(), self.kind) {
            ("provides", _) => {
                if self.provides.is_some() {
                    return Err(duplicate(&meta));
                }
                self.provides = Some(parse_provides(&meta)?);
            }
            ("order", _) => {
                if self.order.is_some() {
                    return Err(duplicate(&meta));
                }
                self.order = Some(meta.value()?.parse()?);
            }
            ("profile", _) => {
                if self.profile.is_some() {
                    return Err(duplicate(&meta));
                }
                self.profile = Some(parse_profile(&meta, what)?);
            }
            ("primary", _) => flag(&mut self.primary, &meta, &ident, what)?,
            ("lazy", _) => flag(&mut self.lazy, &meta, &ident, what)?,
            ("lifecycle", Kind::Component) => flag(&mut self.lifecycle, &meta, &ident, what)?,
            ("fallible", Kind::Bean) => flag(&mut self.fallible, &meta, &ident, what)?,
            ("on_bean", Kind::Bean) => {
                let ty = parse_condition_type(&meta, "on_bean")?
                    .ok_or_else(|| meta.error("`on_bean` needs a type, e.g. `on_bean(Pool)`"))?;
                self.conditions.push(Condition::Bean(ty));
            }
            ("on_missing_bean", Kind::Bean) => {
                let ty = parse_condition_type(&meta, "on_missing_bean")?;
                self.conditions
                    .push(Condition::MissingBean(ty, meta.path.span()));
            }
            ("on_property", Kind::Bean) => self.conditions.push(parse_on_property(&meta)?),
            _ => {
                return Err(meta.error(format!(
                    "unknown #[{what}] option; {}",
                    self.kind.expected()
                )));
            }
        }
        Ok(())
    }

    /// The builder modifiers for the options, applied to the registration of `key`:
    /// `.provides(..)`, `.primary()`, `.order(n)`, `.lazy()`, `.lifecycle::<key>()` and one
    /// `.profile(name)` per name. Empty when no option is set.
    pub(crate) fn modifiers(&self, key: &TokenStream2) -> TokenStream2 {
        // Spanned at the trait so "trait bound not satisfied" points at `dyn Trait`.
        let provides = self.provides.iter().flatten().map(|ty| {
            // A prefixed name, so a unit struct `c` in scope cannot turn it into a pattern. It
            // keeps the trait's span (a mixed-site span would move the unsizing error from
            // `dyn Trait` to the whole attribute); the closure has no other bindings to clash.
            let c = proc_macro2::Ident::new("__oxbloom_c", ty.span());
            quote_spanned! {ty.span()=>
                .provides(|#c: ::std::sync::Arc<#key>| -> ::std::sync::Arc<#ty> { #c })
            }
        });
        let primary = self.primary.map(|_| quote! { .primary() });
        let order = self.order.as_ref().map(|n| quote! { .order(#n) });
        let lazy = self.lazy.map(|_| quote! { .lazy() });
        // Spanned at the option so "`Lifecycle` is not implemented" points at `lifecycle`.
        // Only `#[component]` has it, so the key is `Self`, spanned there too.
        let lifecycle = self
            .lifecycle
            .map(|span| quote_spanned! {span=> .lifecycle::<Self>() });
        let profiles = self
            .profile
            .iter()
            .flatten()
            .map(|name| quote! { .profile(#name) });
        let conditions = self.conditions.iter().map(|c| c.modifier(Some(key)));
        quote! { #(#provides)* #primary #order #lazy #lifecycle #(#profiles)* #(#conditions)* }
    }

    /// Whether any option that becomes a modifier is set.
    pub(crate) fn has_modifiers(&self) -> bool {
        self.provides.is_some()
            || self.primary.is_some()
            || self.order.is_some()
            || self.lazy.is_some()
            || self.lifecycle.is_some()
            || self.profile.is_some()
            || !self.conditions.is_empty()
    }
}

impl Condition {
    /// The modifier call, e.g. `.on_bean::<T>()`. `key` is the type a bare `on_missing_bean`
    /// stands for; callers without one reject that form while parsing.
    pub(crate) fn modifier(&self, key: Option<&TokenStream2>) -> TokenStream2 {
        // Spanned at the user's type or literal, so an unknown type points there.
        match self {
            Condition::Bean(ty) => quote_spanned! {ty.span()=> .on_bean::<#ty>() },
            Condition::MissingBean(Some(ty), _) => {
                quote_spanned! {ty.span()=> .on_missing_bean::<#ty>() }
            }
            Condition::MissingBean(None, span) => {
                quote_spanned! {*span=> .on_missing_bean::<#key>() }
            }
            Condition::Property(k) => quote! { .on_property(#k) },
            Condition::PropertyValue(k, v) => quote! { .on_property_value(#k, #v) },
        }
    }
}

/// A flag option such as `lazy`: no value, at most once.
fn flag(
    slot: &mut Option<Span>,
    meta: &ParseNestedMeta,
    ident: &str,
    what: &str,
) -> syn::Result<()> {
    if slot.is_some() {
        return Err(meta.error(format!("duplicate #[{what}] option `{ident}`")));
    }
    if !(meta.input.is_empty() || meta.input.peek(Token![,])) {
        return Err(meta.error(format!("the #[{what}] option `{ident}` takes no value")));
    }
    *slot = Some(meta.path.span());
    Ok(())
}

/// `provides(dyn A, dyn B)`: at least one type, each a `dyn Trait`.
fn parse_provides(meta: &ParseNestedMeta) -> syn::Result<Vec<syn::Type>> {
    let content;
    syn::parenthesized!(content in meta.input);
    let types = content.parse_terminated(<syn::Type as syn::parse::Parse>::parse, Token![,])?;
    if types.is_empty() {
        return Err(meta.error("`provides` needs at least one `dyn Trait`"));
    }
    for ty in &types {
        if !matches!(ty, syn::Type::TraitObject(t) if t.dyn_token.is_some()) {
            return Err(Error::new(
                ty.span(),
                "`provides` takes trait objects such as `dyn Trait`",
            ));
        }
    }
    Ok(types.into_iter().collect())
}

/// `profile = "dev"`, `profile = "!prod"` or `profile = ["dev", "test"]` (any-of).
fn parse_profile(meta: &ParseNestedMeta, what: &str) -> syn::Result<Vec<LitStr>> {
    let value = meta.value()?;
    let names: Vec<LitStr> = if value.peek(syn::token::Bracket) {
        let content;
        let brackets = syn::bracketed!(content in value);
        let names = content.parse_terminated(<LitStr as syn::parse::Parse>::parse, Token![,])?;
        if names.is_empty() {
            return Err(Error::new(
                brackets.span.join(),
                format!("#[{what}] `profile` needs at least one name"),
            ));
        }
        names.into_iter().collect()
    } else if value.peek(LitStr) {
        vec![value.parse()?]
    } else {
        return Err(value.error(format!(
            "#[{what}] `profile` takes a name such as `\"dev\"`, `\"!prod\"` or a list \
             `[\"dev\", \"test\"]`"
        )));
    };
    for name in &names {
        let text = name.value();
        if text.strip_prefix('!').unwrap_or(&text).is_empty() {
            return Err(Error::new(name.span(), "a profile name must not be empty"));
        }
    }
    Ok(names)
}

/// The type of `on_bean(T)` / `on_missing_bean(T)`, or `None` when the option has no
/// parentheses. Exactly one type, optionally followed by a comma; `dyn Trait` is accepted.
pub(crate) fn parse_condition_type(
    meta: &ParseNestedMeta,
    name: &str,
) -> syn::Result<Option<syn::Type>> {
    if meta.input.is_empty() || meta.input.peek(Token![,]) {
        return Ok(None);
    }
    let usage = format!("`{name}` takes one type, e.g. `{name}(Pool)` or `{name}(dyn Store)`");
    if !meta.input.peek(syn::token::Paren) {
        return Err(meta.error(usage));
    }
    let content;
    let parens = syn::parenthesized!(content in meta.input);
    if content.is_empty() {
        return Err(Error::new(parens.span.join(), usage));
    }
    let ty: syn::Type = content
        .parse()
        .map_err(|e| Error::new(e.span(), usage.clone()))?;
    // A trailing comma is accepted, as in `on_property("key",)`.
    if content.peek(Token![,]) {
        content.parse::<Token![,]>()?;
    }
    if !content.is_empty() {
        return Err(content.error(usage));
    }
    if let syn::Type::ImplTrait(_) = ty {
        return Err(Error::new_spanned(ty, usage));
    }
    Ok(Some(ty))
}

/// `on_property("key")` or `on_property("key", "value")`: string literals, a non-empty key.
pub(crate) fn parse_on_property(meta: &ParseNestedMeta) -> syn::Result<Condition> {
    let usage = "`on_property` takes a key, `on_property(\"key\")`, or a key and a value, \
                 `on_property(\"key\", \"value\")`";
    if !meta.input.peek(syn::token::Paren) {
        return Err(meta.error(usage));
    }
    let content;
    let parens = syn::parenthesized!(content in meta.input);
    let args = content
        .parse_terminated(<LitStr as syn::parse::Parse>::parse, Token![,])
        .map_err(|e| Error::new(e.span(), usage))?;
    let mut args = args.into_iter();
    let Some(key) = args.next() else {
        return Err(Error::new(parens.span.join(), usage));
    };
    if key.value().is_empty() {
        return Err(Error::new(
            key.span(),
            "an `on_property` key must not be empty",
        ));
    }
    let value = args.next();
    if let Some(extra) = args.next() {
        return Err(Error::new(extra.span(), usage));
    }
    Ok(match value {
        None => Condition::Property(key),
        Some(value) => Condition::PropertyValue(key, value),
    })
}
