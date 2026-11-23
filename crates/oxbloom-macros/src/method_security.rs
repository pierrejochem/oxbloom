//! `#[method_security]` and its method attributes: `#[pre_authorize]` and `#[post_authorize]`
//! check a rule against the caller's `Authentication` before and after the method body, and
//! `#[pre_filter]` and `#[post_filter]` shrink a `Vec` argument or a returned `Vec`.

use proc_macro2::{Span, TokenStream as TokenStream2, TokenTree};
use quote::{quote, quote_spanned};
use syn::meta::ParseNestedMeta;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
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
        params: &Parameters,
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
                    if params.named(arg).is_none() {
                        return Err(params.unknown(arg));
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

/// A parameter of the method bound to a plain name, such as `ids: Vec<u64>`.
pub(crate) struct Named {
    name: Ident,
    ty: Type,
    /// The binding is already `mut`, so a filter needs no rebinding.
    mutable: bool,
    /// The binding is `ref`, so it is not taken by value.
    by_ref: bool,
}

/// The parameters of a method, as far as `custom` and `arg` can name them.
#[derive(Default)]
pub(crate) struct Parameters {
    named: Vec<Named>,
    /// Names bound inside a pattern parameter such as `(id, name): (u64, String)`.
    in_patterns: Vec<Ident>,
}

impl Parameters {
    fn of(f: &ImplItemFn) -> Self {
        let mut params = Self::default();
        for input in &f.sig.inputs {
            let FnArg::Typed(typed) = input else { continue };
            match &*typed.pat {
                Pat::Ident(p) if p.subpat.is_none() => params.named.push(Named {
                    name: p.ident.clone(),
                    ty: (*typed.ty).clone(),
                    mutable: p.mutability.is_some(),
                    by_ref: p.by_ref.is_some(),
                }),
                pat => bindings(pat, &mut params.in_patterns),
            }
        }
        params
    }

    fn named(&self, name: &Ident) -> Option<&Named> {
        self.named.iter().find(|p| p.name == *name)
    }

    /// The error for a name that is not a plain parameter of the method.
    fn unknown(&self, name: &Ident) -> Error {
        let message = if self.in_patterns.contains(name) {
            format!(
                "`{name}` is bound inside a pattern parameter, and pattern parameters cannot be \
                 named here; bind the whole parameter to a name, e.g. `pair: (u64, String)`, \
                 and name that"
            )
        } else {
            format!("`{name}` is not a parameter of the method")
        };
        Error::new(name.span(), message)
    }

    /// Resolves the `arg` of `#[pre_filter]`: a by-value `Vec<T>` parameter. Returns whether
    /// it is already bound as `mut`.
    fn filtered(&self, arg: &Ident) -> syn::Result<bool> {
        let Some(param) = self.named(arg) else {
            return Err(self.unknown(arg));
        };
        if param.by_ref || !is_vec(&param.ty) {
            return Err(Error::new_spanned(
                &param.ty,
                format!(
                    "#[pre_filter] can only filter a `Vec<T>` parameter taken by value, and \
                     `{arg}` is not one"
                ),
            ));
        }
        Ok(param.mutable)
    }
}

/// Collects the names a pattern binds.
fn bindings(pat: &Pat, out: &mut Vec<Ident>) {
    match pat {
        Pat::Ident(p) => {
            out.push(p.ident.clone());
            if let Some((_, sub)) = &p.subpat {
                bindings(sub, out);
            }
        }
        Pat::Tuple(t) => t.elems.iter().for_each(|p| bindings(p, out)),
        Pat::TupleStruct(t) => t.elems.iter().for_each(|p| bindings(p, out)),
        Pat::Slice(s) => s.elems.iter().for_each(|p| bindings(p, out)),
        Pat::Or(o) => o.cases.iter().for_each(|p| bindings(p, out)),
        Pat::Struct(s) => s.fields.iter().for_each(|field| bindings(&field.pat, out)),
        Pat::Reference(r) => bindings(&r.pat, out),
        Pat::Paren(p) => bindings(&p.pat, out),
        Pat::Type(t) => bindings(&t.pat, out),
        _ => {}
    }
}

/// True for a type written as `Vec<T>` (also with a path such as `std::vec::Vec<T>`).
fn is_vec(ty: &Type) -> bool {
    match ty {
        Type::Path(p) if p.qself.is_none() => p.path.segments.last().is_some_and(|last| {
            last.ident == "Vec" && matches!(last.arguments, syn::PathArguments::AngleBracketed(_))
        }),
        Type::Group(g) => is_vec(&g.elem),
        Type::Paren(p) => is_vec(&p.elem),
        _ => false,
    }
}

/// True when the tokens contain the keyword `impl`, which in a type means `impl Trait`.
fn mentions_impl(tokens: TokenStream2) -> bool {
    tokens.into_iter().any(|tree| match tree {
        TokenTree::Ident(ident) => ident == "impl",
        TokenTree::Group(group) => mentions_impl(group.stream()),
        _ => false,
    })
}

/// Rejects an `impl Trait` return type on a method with a post attribute: the body is wrapped
/// in a closure or an `async` block annotated with the declared return type, which cannot name
/// `impl Trait`.
fn check_written_out(f: &ImplItemFn, attribute: &str) -> syn::Result<()> {
    let ReturnType::Type(_, ty) = &f.sig.output else {
        return Ok(());
    };
    if !mentions_impl(quote!(#ty)) {
        return Ok(());
    }
    Err(Error::new_spanned(
        ty,
        format!(
            "the method `{}` has #[{attribute}], so its return type must be written out: the body \
             is wrapped in a closure or an `async` block of the declared return type, and that \
             cannot be `impl Trait`",
            f.sig.ident
        ),
    ))
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

/// The function of a filter attribute and, for `#[pre_filter]`, the filtered parameter.
struct Filter {
    arg: Option<Ident>,
    keep: syn::ExprPath,
}

impl Filter {
    /// Parses `#[pre_filter(arg = ids, custom = keep)]` (`takes_arg`) or
    /// `#[post_filter(custom = keep)]`.
    fn from_attribute(attr: &Attribute, name: &str, takes_arg: bool) -> syn::Result<Filter> {
        let example = if takes_arg {
            "e.g. `#[pre_filter(arg = ids, custom = keep)]`"
        } else {
            "e.g. `#[post_filter(custom = keep)]`"
        };
        let options = if takes_arg {
            "`arg` and `custom`"
        } else {
            "`custom`"
        };
        if !matches!(attr.meta, syn::Meta::List(_)) {
            return Err(Error::new_spanned(
                attr,
                format!("#[{name}] takes {options}, {example}"),
            ));
        }
        let mut arg: Option<Ident> = None;
        let mut keep: Option<syn::ExprPath> = None;
        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(Ident::to_string)
                .unwrap_or_default();
            let repeated = || meta.error(format!("`{key}` is given twice in #[{name}]"));
            match key.as_str() {
                "arg" if !takes_arg => Err(meta.error(format!(
                    "#[{name}] filters the returned `Vec` and takes no `arg`; {example}"
                ))),
                "arg" if arg.is_some() => Err(repeated()),
                "arg" => {
                    let usage = "`arg` takes the name of a `Vec<T>` parameter, e.g. `arg = ids`";
                    let value = meta.value().map_err(|e| Error::new(e.span(), usage))?;
                    arg = Some(value.parse().map_err(|e| Error::new(e.span(), usage))?);
                    Ok(())
                }
                "custom" if keep.is_some() => Err(repeated()),
                "custom" => {
                    let usage = "`custom` takes the path of a function \
                                 `fn(Option<&Authentication>, &T) -> bool`, e.g. `custom = keep`; \
                                 the filter passes each element to it";
                    let value = meta.value().map_err(|e| Error::new(e.span(), usage))?;
                    match value.parse::<Expr>() {
                        Ok(Expr::Path(path)) => {
                            keep = Some(path);
                            Ok(())
                        }
                        Ok(other) => Err(Error::new_spanned(other, usage)),
                        Err(e) => Err(Error::new(e.span(), usage)),
                    }
                }
                _ => Err(meta.error(format!(
                    "unknown option of #[{name}]; it takes {options}, {example}"
                ))),
            }
        })?;
        let missing = |option: &str| {
            Error::new_spanned(attr, format!("#[{name}] needs `{option}`, {example}"))
        };
        if takes_arg && arg.is_none() {
            return Err(missing("arg"));
        }
        let keep = keep.ok_or_else(|| missing("custom"))?;
        Ok(Filter { arg, keep })
    }
}

/// The method security attributes of one method, taken off it.
#[derive(Default)]
struct MethodAttributes {
    pre_authorize: Option<Attribute>,
    post_authorize: Option<Attribute>,
    pre_filter: Option<Attribute>,
    post_filter: Option<Attribute>,
}

impl MethodAttributes {
    /// Removes every method security attribute from `f`, so the method stays valid even when
    /// one of them is rejected, then rejects repeated ones.
    fn take(f: &mut ImplItemFn) -> syn::Result<Self> {
        let (ours, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut f.attrs)
            .into_iter()
            .partition(|attr| attribute_name(attr).is_some());
        f.attrs = kept;
        let mut found = Self::default();
        for attr in ours {
            let name = attribute_name(&attr).unwrap_or_default();
            let (slot, hint) = match name {
                "pre_authorize" => (
                    &mut found.pre_authorize,
                    "combine the rules with `all(..)` or `any(..)`",
                ),
                "post_authorize" => (
                    &mut found.post_authorize,
                    "combine the rules with `all(..)` or `any(..)`",
                ),
                "pre_filter" => (&mut found.pre_filter, "combine the checks in one function"),
                _ => (&mut found.post_filter, "combine the checks in one function"),
            };
            if slot.is_some() {
                return Err(Error::new_spanned(
                    &attr,
                    format!(
                        "the method `{}` has two #[{name}] attributes; {hint}",
                        f.sig.ident
                    ),
                ));
            }
            *slot = Some(attr);
        }
        Ok(found)
    }

    fn is_empty(&self) -> bool {
        self.pre_authorize.is_none()
            && self.post_authorize.is_none()
            && self.pre_filter.is_none()
            && self.post_filter.is_none()
    }
}

/// Rewrites one method. The order of execution is `pre_filter`, `pre_authorize`, the body,
/// `post_authorize` and `post_filter`. Without a post attribute the body stays as written and
/// the context is read and dropped in its own block before it. With a post attribute the body
/// is wrapped (a closure for a synchronous method, an `async` block for an `async` one, both of
/// the declared return type), so `return` and `?` in the body leave only the body and the post
/// checks still run.
fn secure_method(type_name: &str, f: &mut ImplItemFn) -> syn::Result<()> {
    let attrs = MethodAttributes::take(f)?;
    if attrs.is_empty() {
        return Ok(());
    }
    let params = Parameters::of(f);
    let pre_authorize = match &attrs.pre_authorize {
        Some(attr) => {
            check_returns_result(f, "pre_authorize")?;
            let rule = Rule::from_attribute(attr, "pre_authorize")?;
            rule.check_names(&params, false, "pre_authorize")?;
            Some(rule)
        }
        None => None,
    };
    let post_authorize = match &attrs.post_authorize {
        Some(attr) => {
            check_written_out(f, "post_authorize")?;
            check_returns_result(f, "post_authorize")?;
            let rule = Rule::from_attribute(attr, "post_authorize")?;
            rule.check_names(&params, true, "post_authorize")?;
            Some(rule)
        }
        None => None,
    };
    let pre_filter = match &attrs.pre_filter {
        Some(attr) => {
            let filter = Filter::from_attribute(attr, "pre_filter", true)?;
            let arg = filter.arg.clone().unwrap_or_else(|| unreachable!());
            let mutable = params.filtered(&arg)?;
            Some((filter, arg, mutable))
        }
        None => None,
    };
    let post_filter = match &attrs.post_filter {
        Some(attr) => {
            check_written_out(f, "post_filter")?;
            check_returns_result(f, "post_filter")?;
            Some(Filter::from_attribute(attr, "post_filter", false)?)
        }
        None => None,
    };

    let local = |name: &str| Ident::new(name, Span::mixed_site());
    let context = local("__oxbloom_context");
    let auth = local("__auth");
    let element = local("__element");
    let method = format!("{type_name}::{}", f.sig.ident);
    // A type error from the generated `Err(..)` (a return type that is not a `Result`, or an
    // error type without `From<AccessDenied>`) points at the declared return type.
    let return_span = match &f.sig.output {
        ReturnType::Type(_, ty) => ty.span(),
        ReturnType::Default => Span::call_site(),
    };
    let deny = quote_spanned!(return_span=>
        return ::core::result::Result::Err(::core::convert::Into::into(
            ::oxbloom_security::__private::deny(#method, #auth),
        ));
    );
    let read_context = quote! {
        let #context = ::oxbloom_security::SecurityContext::current();
        let #auth: ::core::option::Option<&::oxbloom_security::Authentication> =
            #context.authentication();
    };
    let (rebind, pre_filter) = match pre_filter {
        Some((filter, arg, mutable)) => {
            let keep = &filter.keep;
            let rebind = (!mutable).then(|| quote!(let mut #arg = #arg;));
            let retain = quote!(#arg.retain(|#element| #keep(#auth, #element)););
            (rebind, Some(retain))
        }
        None => (None, None),
    };
    let pre_authorize = pre_authorize.map(|rule| {
        let check = rule.tokens(&auth, None);
        quote!(if !(#check) { #deny })
    });
    // The body's statements are spliced into a block of the macro, so a one-line body does not
    // keep its own braces as a nested block (which rustc reports as `unused_braces`).
    let stmts = &f.block.stmts;

    if post_authorize.is_none() && post_filter.is_none() {
        f.block = syn::parse_quote!({
            #rebind
            {
                #read_context
                #pre_filter
                #pre_authorize
            }
            #(#stmts)*
        });
        return Ok(());
    }

    let ReturnType::Type(_, ret) = &f.sig.output else {
        unreachable!("check_returns_result rejected a method without a return type")
    };
    let result = local("__result");
    let value = local("__value");
    let wrapped = if f.sig.asyncness.is_some() {
        quote!(async {
            let #value: #ret = { #(#stmts)* };
            #value
        }
        .await)
    } else {
        // The closure is passed as `FnOnce`, so a body that returns a borrow of a `&mut`
        // parameter (`Ok(&mut self.items)`) compiles as it does without the wrapper.
        let call = local("__call_once");
        quote!({
            fn #call<R, F: ::core::ops::FnOnce() -> R>(f: F) -> R {
                f()
            }
            #call(|| -> #ret { #(#stmts)* })
        })
    };
    let post_authorize = post_authorize.map(|rule| {
        let returned = local("__returned");
        let check = rule.tokens(&auth, Some(&quote!(#returned)));
        quote! {
            if let ::core::result::Result::Ok(#returned) = &#result {
                if !(#check) { #deny }
            }
        }
    });
    let (mutable, post_filter) = match post_filter {
        Some(filter) => {
            let keep = &filter.keep;
            let retain = quote! {
                if let ::core::result::Result::Ok(#value) = &mut #result {
                    #value.retain(|#element| #keep(#auth, #element));
                }
            };
            (Some(quote!(mut)), Some(retain))
        }
        None => (None, None),
    };
    f.block = syn::parse_quote!({
        #rebind
        #read_context
        #pre_filter
        #pre_authorize
        let #mutable #result: #ret = #wrapped;
        #post_authorize
        #post_filter
        #result
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
        let params = Parameters::default();
        assert!(r.check_names(&params, false, "pre_authorize").is_err());
        assert!(r.check_names(&params, true, "post_authorize").is_ok());
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
