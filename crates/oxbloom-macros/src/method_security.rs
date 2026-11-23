//! `#[method_security]` and its method attribute `#[pre_authorize]`: a rule checked against the
//! caller's `Authentication` before the method body runs.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::meta::ParseNestedMeta;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Error, Expr, FnArg, Ident, ImplItem, ImplItemFn, ItemImpl, LitStr, Pat, ReturnType,
    Token, Type, token,
};

/// The method attributes `#[method_security]` handles.
const ATTRIBUTES: [&str; 4] = [
    "pre_authorize",
    "post_authorize",
    "pre_filter",
    "post_filter",
];

/// The rule names, for the error on an unknown one.
const RULES: &str = "`role`, `any_role`, `authority`, `any_authority`, `authenticated`, \
                     `fully_authenticated`, `permit_all`, `deny_all`, `all`, `any`, `not` and \
                     `custom`";

/// The name of a method security attribute, when `attr` is one (also with the prefix
/// `oxbloom_security::` or `oxbloom_macros::`).
fn attribute_name(attr: &Attribute) -> Option<&'static str> {
    let segments = &attr.path().segments;
    let prefix_ok = match segments.len() {
        1 => true,
        2 => segments[0].ident == "oxbloom_security" || segments[0].ident == "oxbloom_macros",
        _ => false,
    };
    let last = segments.last()?;
    ATTRIBUTES
        .into_iter()
        .find(|name| prefix_ok && last.ident == name)
}

/// One authorization rule of a method attribute, parsed from its typed arguments.
pub(crate) enum Rule {
    /// `role = "ADMIN"`.
    Role(LitStr),
    /// `any_role("A", "B")`.
    AnyRole(Vec<LitStr>),
    /// `authority = "x"`.
    Authority(LitStr),
    /// `any_authority("x", "y")`.
    AnyAuthority(Vec<LitStr>),
    /// `authenticated`.
    Authenticated,
    /// `fully_authenticated`.
    FullyAuthenticated,
    /// `permit_all`.
    PermitAll,
    /// `deny_all`.
    DenyAll,
    /// `all(..)`.
    All(Vec<Rule>),
    /// `any(..)`.
    Any(Vec<Rule>),
    /// `not(..)`.
    Not(Box<Rule>),
    /// `custom = check(arg1, arg2)`: the function path and the argument names.
    Custom(syn::ExprPath, Vec<Ident>),
}

impl Rule {
    /// Parses the single rule of an attribute such as `#[pre_authorize(role = "ADMIN")]`.
    pub(crate) fn from_attribute(attr: &Attribute, name: &str) -> syn::Result<Rule> {
        let example = format!("e.g. `#[{name}(role = \"ADMIN\")]`");
        if !matches!(attr.meta, syn::Meta::List(_)) {
            return Err(Error::new_spanned(
                attr,
                format!("#[{name}] takes one rule, {example}"),
            ));
        }
        let mut rules = Vec::new();
        attr.parse_nested_meta(|meta| {
            if !rules.is_empty() {
                return Err(meta.error(format!(
                    "#[{name}] takes one rule; combine several with `all(..)` or `any(..)`"
                )));
            }
            rules.push(Rule::parse(&meta)?);
            Ok(())
        })?;
        rules
            .pop()
            .ok_or_else(|| Error::new_spanned(attr, format!("#[{name}] takes one rule, {example}")))
    }

    /// Parses one rule at the position of `meta`, recursively for the combinators.
    pub(crate) fn parse(meta: &ParseNestedMeta<'_>) -> syn::Result<Rule> {
        let Some(key) = meta.path.get_ident() else {
            return Err(meta.error(format!("unknown rule; the rules are {RULES}")));
        };
        let name = key.to_string();
        let string = || -> syn::Result<LitStr> {
            let value = meta.value()?;
            let lit: LitStr = value.parse().map_err(|_| {
                Error::new(
                    key.span(),
                    format!("`{name}` takes a string, e.g. `{name} = \"ADMIN\"`"),
                )
            })?;
            if lit.value().is_empty() {
                return Err(Error::new(lit.span(), format!("`{name}` cannot be empty")));
            }
            Ok(lit)
        };
        let strings = || -> syn::Result<Vec<LitStr>> {
            if !meta.input.peek(token::Paren) {
                return Err(Error::new(
                    key.span(),
                    format!("`{name}` takes a list of strings, e.g. `{name}(\"A\", \"B\")`"),
                ));
            }
            let content;
            syn::parenthesized!(content in meta.input);
            let list = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
            if list.is_empty() {
                return Err(Error::new(
                    key.span(),
                    format!("`{name}` needs at least one name"),
                ));
            }
            if let Some(empty) = list.iter().find(|lit| lit.value().is_empty()) {
                return Err(Error::new(
                    empty.span(),
                    format!("a name in `{name}` cannot be empty"),
                ));
            }
            Ok(list.into_iter().collect())
        };
        let rules = || -> syn::Result<Vec<Rule>> {
            if !meta.input.peek(token::Paren) {
                return Err(Error::new(
                    key.span(),
                    format!(
                        "`{name}` takes a list of rules, e.g. `{name}(role = \"A\", authenticated)`"
                    ),
                ));
            }
            let mut list = Vec::new();
            meta.parse_nested_meta(|inner| {
                list.push(Rule::parse(&inner)?);
                Ok(())
            })?;
            if list.is_empty() {
                return Err(Error::new(
                    key.span(),
                    format!("`{name}` needs at least one rule"),
                ));
            }
            Ok(list)
        };
        let word = |rule: Rule| -> syn::Result<Rule> {
            if meta.input.peek(Token![=]) || meta.input.peek(token::Paren) {
                return Err(Error::new(key.span(), format!("`{name}` takes no value")));
            }
            Ok(rule)
        };
        match name.as_str() {
            "role" => Ok(Rule::Role(string()?)),
            "any_role" => Ok(Rule::AnyRole(strings()?)),
            "authority" => Ok(Rule::Authority(string()?)),
            "any_authority" => Ok(Rule::AnyAuthority(strings()?)),
            "authenticated" => word(Rule::Authenticated),
            "fully_authenticated" => word(Rule::FullyAuthenticated),
            "permit_all" => word(Rule::PermitAll),
            "deny_all" => word(Rule::DenyAll),
            "all" => Ok(Rule::All(rules()?)),
            "any" => Ok(Rule::Any(rules()?)),
            "not" => {
                let mut list = rules()?;
                if list.len() != 1 {
                    return Err(Error::new(key.span(), "`not` takes exactly one rule"));
                }
                Ok(Rule::Not(Box::new(list.remove(0))))
            }
            "custom" => Self::parse_custom(meta, key),
            _ => Err(Error::new(
                key.span(),
                format!("unknown rule `{name}`; the rules are {RULES}"),
            )),
        }
    }

    /// Parses `custom = check(arg1, arg2)`.
    fn parse_custom(meta: &ParseNestedMeta<'_>, key: &Ident) -> syn::Result<Rule> {
        let usage = "`custom` takes a call of a function with parameter names of the method, \
                     e.g. `custom = owns(account)`";
        let value = meta.value().map_err(|_| Error::new(key.span(), usage))?;
        let call: syn::ExprCall = value.parse().map_err(|e| Error::new(e.span(), usage))?;
        let Expr::Path(func) = *call.func else {
            return Err(Error::new_spanned(&call.func, usage));
        };
        let mut args = Vec::new();
        for arg in &call.args {
            match arg {
                Expr::Path(p) if p.qself.is_none() && p.path.get_ident().is_some() => {
                    args.push(
                        p.path
                            .get_ident()
                            .cloned()
                            .unwrap_or_else(|| unreachable!()),
                    );
                }
                other => return Err(Error::new_spanned(other, usage)),
            }
        }
        Ok(Rule::Custom(func, args))
    }

    /// Checks that each `custom` argument names a parameter of the method. `returned` is
    /// reserved for `#[post_authorize]`; `returned_allowed` says whether it is available.
    pub(crate) fn check_names(
        &self,
        params: &[Ident],
        returned_allowed: bool,
        attribute: &str,
    ) -> syn::Result<()> {
        match self {
            Rule::All(rules) | Rule::Any(rules) => rules
                .iter()
                .try_for_each(|r| r.check_names(params, returned_allowed, attribute)),
            Rule::Not(rule) => rule.check_names(params, returned_allowed, attribute),
            Rule::Custom(_, args) => {
                for arg in args {
                    if arg == "returned" {
                        if returned_allowed {
                            continue;
                        }
                        return Err(Error::new(
                            arg.span(),
                            format!(
                                "`returned` stands for the returned value and is only \
                                 available in #[post_authorize], not in #[{attribute}]"
                            ),
                        ));
                    }
                    if !params.contains(arg) {
                        return Err(Error::new(
                            arg.span(),
                            format!("`{arg}` is not a parameter of the method"),
                        ));
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// The `bool` expression of the rule over `auth` (an `Option<&Authentication>`).
    /// `returned` is the expression that stands for `&value` in `#[post_authorize]`. The
    /// built-in rules call the shared functions of `oxbloom_security::__private` with string
    /// literals and constant slices, so they allocate nothing.
    pub(crate) fn tokens(&self, auth: &Ident, returned: Option<&TokenStream2>) -> TokenStream2 {
        let private = quote!(::oxbloom_security::__private);
        match self {
            Rule::Role(role) => quote!(#private::has_role(#auth, #role)),
            Rule::AnyRole(roles) => quote!(#private::has_any_role(#auth, &[#(#roles),*])),
            Rule::Authority(name) => quote!(#private::has_authority(#auth, #name)),
            Rule::AnyAuthority(names) => {
                quote!(#private::has_any_authority(#auth, &[#(#names),*]))
            }
            Rule::Authenticated => quote!(#private::authenticated(#auth)),
            Rule::FullyAuthenticated => quote!(#private::fully_authenticated(#auth)),
            Rule::PermitAll => quote!(true),
            Rule::DenyAll => quote!(false),
            Rule::All(rules) => {
                let parts = rules.iter().map(|r| r.tokens(auth, returned));
                quote!((#(#parts)&&*))
            }
            Rule::Any(rules) => {
                let parts = rules.iter().map(|r| r.tokens(auth, returned));
                quote!((#(#parts)||*))
            }
            Rule::Not(rule) => {
                let inner = rule.tokens(auth, returned);
                quote!((!#inner))
            }
            Rule::Custom(func, args) => {
                let args = args.iter().map(|arg| match returned {
                    Some(value) if arg == "returned" => value.clone(),
                    _ => quote!(&#arg),
                });
                quote!(#func(#auth #(, #args)*))
            }
        }
    }
}

/// The last segment of the impl's type, for the method name `Type::method` in the denial log.
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
        "#[method_security] requires an impl of a named type such as `impl Accounts { .. }`",
    ))
}

/// The names of the method's parameters that are plain identifiers, for `custom`.
fn parameter_names(f: &ImplItemFn) -> Vec<Ident> {
    f.sig
        .inputs
        .iter()
        .filter_map(|input| match input {
            FnArg::Typed(typed) => match &*typed.pat {
                Pat::Ident(p) => Some(p.ident.clone()),
                _ => None,
            },
            FnArg::Receiver(_) => None,
        })
        .collect()
}

/// Rejects a return type that is visibly not a `Result`. A path type passes, so aliases such
/// as `io::Result<T>` work; the generated `Err(..)` checks the rest.
fn check_returns_result(f: &ImplItemFn, attribute: &str) -> syn::Result<()> {
    fn is_path(ty: &Type) -> bool {
        match ty {
            Type::Path(_) => true,
            Type::Group(g) => is_path(&g.elem),
            Type::Paren(p) => is_path(&p.elem),
            _ => false,
        }
    }
    let ok = match &f.sig.output {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => is_path(ty),
    };
    if ok {
        return Ok(());
    }
    let method = &f.sig.ident;
    let message = format!(
        "the method `{method}` has #[{attribute}], so it must return `Result<T, E>` with \
         `E: From<oxbloom_security::AccessDenied>`"
    );
    Err(match &f.sig.output {
        ReturnType::Default => Error::new_spanned(&f.sig, message),
        ReturnType::Type(_, ty) => Error::new_spanned(ty, message),
    })
}

/// The method security attributes of one method, taken off it.
#[derive(Default)]
struct MethodAttributes {
    pre_authorize: Option<Attribute>,
}

impl MethodAttributes {
    /// Removes every method security attribute from `f`, so the method stays valid even when
    /// one of them is rejected, then rejects repeated ones and the ones not supported yet.
    fn take(f: &mut ImplItemFn) -> syn::Result<Self> {
        let (ours, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut f.attrs)
            .into_iter()
            .partition(|attr| attribute_name(attr).is_some());
        f.attrs = kept;
        let mut found = Self::default();
        for attr in ours {
            match attribute_name(&attr) {
                Some("pre_authorize") if found.pre_authorize.is_some() => {
                    return Err(Error::new_spanned(
                        &attr,
                        format!(
                            "the method `{}` has two #[pre_authorize] attributes; combine the \
                             rules with `all(..)` or `any(..)`",
                            f.sig.ident
                        ),
                    ));
                }
                Some("pre_authorize") => found.pre_authorize = Some(attr),
                other => {
                    return Err(Error::new_spanned(
                        &attr,
                        format!("#[{}] is not supported yet", other.unwrap_or_default()),
                    ));
                }
            }
        }
        Ok(found)
    }
}

/// Rewrites one method: reads the context once, checks the `pre_authorize` rule and returns
/// the denial before the original body runs.
fn secure_method(type_name: &str, f: &mut ImplItemFn) -> syn::Result<()> {
    let attrs = MethodAttributes::take(f)?;
    let Some(pre) = attrs.pre_authorize else {
        return Ok(());
    };
    check_returns_result(f, "pre_authorize")?;
    let rule = Rule::from_attribute(&pre, "pre_authorize")?;
    rule.check_names(&parameter_names(f), false, "pre_authorize")?;

    let context = Ident::new("__oxbloom_context", Span::mixed_site());
    let auth = Ident::new("__auth", Span::mixed_site());
    let method = format!("{type_name}::{}", f.sig.ident);
    let check = rule.tokens(&auth, None);
    let body = &f.block;
    f.block = syn::parse_quote!({
        let #context = ::oxbloom_security::SecurityContext::current();
        let #auth: ::core::option::Option<&::oxbloom_security::Authentication> =
            #context.authentication();
        if !(#check) {
            return ::core::result::Result::Err(::core::convert::Into::into(
                ::oxbloom_security::__private::deny(#method, #auth),
            ));
        }
        #body
    });
    Ok(())
}

pub(crate) fn expand(args: TokenStream2, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    if !args.is_empty() {
        return Err(Error::new_spanned(
            &args,
            "#[method_security] takes no arguments",
        ));
    }
    if let Some((path, _)) = &item.trait_ {
        return Err(Error::new_spanned(
            path,
            "#[method_security] requires an inherent impl such as `impl Accounts { .. }`, not a \
             trait impl",
        ));
    }
    let type_name = type_name(&item.self_ty)?;
    let mut errors: Option<Error> = None;
    for it in &mut item.items {
        let ImplItem::Fn(f) = it else { continue };
        if let Err(error) = secure_method(&type_name, f) {
            match &mut errors {
                Some(all) => all.combine(error),
                None => errors = Some(error),
            }
        }
    }
    // On an error the impl stays, without the method security attributes and with the bodies
    // of the rejected methods unchanged, so the error is the only one.
    let errors = errors.map(Error::into_compile_error);
    Ok(quote!(#errors #item))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(attr: syn::Attribute) -> syn::Result<Rule> {
        Rule::from_attribute(&attr, "pre_authorize")
    }

    fn code(attr: syn::Attribute) -> String {
        let auth = Ident::new("auth", Span::call_site());
        rule(attr).unwrap().tokens(&auth, None).to_string()
    }

    #[test]
    fn built_in_rules_use_literals_and_constant_slices() {
        let any = code(syn::parse_quote!(#[pre_authorize(any_role("A", "B"))]));
        assert_eq!(
            any,
            ":: oxbloom_security :: __private :: has_any_role (auth , & [\"A\" , \"B\"])"
        );
        assert!(!any.contains("vec") && !any.contains("String"));
    }

    #[test]
    fn combinators_nest() {
        let all = code(syn::parse_quote!(
            #[pre_authorize(all(authenticated, not(any(permit_all, deny_all))))]
        ));
        assert!(all.contains("&&"), "{all}");
        assert!(all.contains("(! (true || false))"), "{all}");
    }

    #[test]
    fn custom_passes_the_caller_and_references_to_the_arguments() {
        let custom = code(syn::parse_quote!(#[pre_authorize(custom = checks::owns(a, b))]));
        assert_eq!(custom, "checks :: owns (auth , & a , & b)");
    }

    #[test]
    fn returned_maps_to_the_given_expression_only_when_allowed() {
        let r = rule(syn::parse_quote!(#[pre_authorize(custom = mine(returned))])).unwrap();
        assert!(r.check_names(&[], false, "pre_authorize").is_err());
        assert!(r.check_names(&[], true, "post_authorize").is_ok());
        let auth = Ident::new("auth", Span::call_site());
        let value = quote!(&__value);
        assert_eq!(
            r.tokens(&auth, Some(&value)).to_string(),
            "mine (auth , & __value)"
        );
    }

    #[test]
    fn malformed_rules_are_rejected() {
        for attr in [
            syn::parse_quote!(#[pre_authorize]),
            syn::parse_quote!(#[pre_authorize()]),
            syn::parse_quote!(#[pre_authorize(role = "A", authenticated)]),
            syn::parse_quote!(#[pre_authorize(role = "")]),
            syn::parse_quote!(#[pre_authorize(any_role())]),
            syn::parse_quote!(#[pre_authorize(all())]),
            syn::parse_quote!(#[pre_authorize(not(permit_all, deny_all))]),
            syn::parse_quote!(#[pre_authorize(authenticated = "yes")]),
            syn::parse_quote!(#[pre_authorize(custom = owns(a.b))]),
            syn::parse_quote!(#[pre_authorize(superuser)]),
        ] {
            assert!(rule(attr).is_err());
        }
    }
}
