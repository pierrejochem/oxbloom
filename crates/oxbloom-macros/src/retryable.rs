//! `#[retryable(..)]`: runs the body of an async method again when it returns `Err`.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::meta::ParseNestedMeta;
use syn::parse::Parser;
use syn::{Error, FnArg, Ident, ImplItemFn, Lit, LitStr, Pat, ReceiverKind, ReturnType, Type};

/// The keys `#[retryable(..)]` accepts.
const KEYS: [&str; 7] = [
    "max_attempts",
    "backoff",
    "initial",
    "multiplier",
    "max_delay",
    "jitter",
    "retry_if",
];

/// The arguments of one `#[retryable(..)]` attribute, checked and converted.
#[derive(Default)]
struct Args {
    /// The keys seen so far, to reject a repeated key.
    seen: Vec<String>,
    max_attempts: Option<u32>,
    /// The variant name of `Backoff`.
    backoff: Option<&'static str>,
    /// The literal and its value in milliseconds.
    initial: Option<(LitStr, u64)>,
    multiplier: Option<f64>,
    max_delay: Option<(LitStr, u64)>,
    /// The variant name of `Jitter`.
    jitter: Option<&'static str>,
    retry_if: Option<syn::Path>,
}

impl Args {
    fn parse(&mut self, meta: &ParseNestedMeta<'_>, method: &Ident) -> syn::Result<()> {
        let Some(key) = meta.path.get_ident().cloned() else {
            return Err(meta.error(unknown_key(method)));
        };
        let name = key.to_string();
        if !KEYS.contains(&name.as_str()) {
            return Err(Error::new(key.span(), unknown_key(method)));
        }
        if self.seen.contains(&name) {
            return Err(Error::new(
                key.span(),
                format!("`{name}` is given twice on the retryable method `{method}`"),
            ));
        }
        self.seen.push(name.clone());
        let example = match name.as_str() {
            "max_attempts" => "max_attempts = 3",
            "backoff" => "backoff = \"exponential\"",
            "initial" => "initial = \"200ms\"",
            "multiplier" => "multiplier = 2.0",
            "max_delay" => "max_delay = \"10s\"",
            "jitter" => "jitter = \"full\"",
            _ => "retry_if = \"is_transient\"",
        };
        let kind = match name.as_str() {
            "max_attempts" => "a whole number",
            "multiplier" => "a number",
            "retry_if" => "the path of a `fn(&E) -> bool` as a string",
            _ => "a string",
        };
        let takes = |span: Span| {
            Error::new(
                span,
                format!(
                    "`{name}` on the retryable method `{method}` takes {kind}, e.g. `{example}`"
                ),
            )
        };
        let value = meta.value().map_err(|_| takes(key.span()))?;
        let lit: Lit = value.parse().map_err(|_| takes(value.span()))?;
        match (name.as_str(), &lit) {
            ("max_attempts", Lit::Int(int)) => {
                let value = int.base10_parse::<u32>().map_err(|_| takes(int.span()))?;
                if value == 0 {
                    return Err(Error::new(
                        int.span(),
                        format!(
                            "`max_attempts` on the retryable method `{method}` must be at least \
                             1; it counts the first call"
                        ),
                    ));
                }
                self.max_attempts = Some(value);
            }
            ("backoff", Lit::Str(text)) => {
                self.backoff = Some(match text.value().trim() {
                    "fixed" => "Fixed",
                    "exponential" => "Exponential",
                    _ => {
                        return Err(Error::new(
                            text.span(),
                            format!(
                                "unknown `backoff` on the retryable method `{method}`; expected \
                                 \"fixed\" or \"exponential\""
                            ),
                        ));
                    }
                });
            }
            ("jitter", Lit::Str(text)) => {
                self.jitter = Some(match text.value().trim() {
                    "none" => "None",
                    "full" => "Full",
                    "equal" => "Equal",
                    _ => {
                        return Err(Error::new(
                            text.span(),
                            format!(
                                "unknown `jitter` on the retryable method `{method}`; expected \
                                 \"none\", \"full\" or \"equal\""
                            ),
                        ));
                    }
                });
            }
            ("initial" | "max_delay", Lit::Str(text)) => {
                let millis = parse_duration_millis(&text.value()).map_err(|problem| {
                    Error::new(
                        text.span(),
                        format!("`{name}` on the retryable method `{method}` {problem}"),
                    )
                })?;
                if name == "initial" {
                    self.initial = Some((text.clone(), millis));
                } else {
                    self.max_delay = Some((text.clone(), millis));
                }
            }
            ("multiplier", Lit::Float(_) | Lit::Int(_)) => {
                let value = match &lit {
                    Lit::Float(float) => float.base10_parse::<f64>(),
                    Lit::Int(int) => int.base10_parse::<f64>(),
                    _ => unreachable!("matched above"),
                }
                .map_err(|_| takes(lit.span()))?;
                if !value.is_finite() || value < 1.0 {
                    return Err(Error::new(
                        lit.span(),
                        format!(
                            "`multiplier` on the retryable method `{method}` must be a finite \
                             number of at least 1"
                        ),
                    ));
                }
                self.multiplier = Some(value);
            }
            ("retry_if", Lit::Str(text)) => {
                self.retry_if = Some(text.parse::<syn::Path>().map_err(|_| takes(text.span()))?);
            }
            _ => return Err(takes(lit.span())),
        }
        Ok(())
    }
}

fn unknown_key(method: &Ident) -> String {
    format!(
        "unknown key on the retryable method `{method}`; the keys are `max_attempts`, \
         `backoff`, `initial`, `multiplier`, `max_delay`, `jitter` and `retry_if`"
    )
}

/// The duration grammar of `oxbloom_tasks::parse_duration`: a whole number followed by `ms`,
/// `s`, `m`, `h` or `d`, with surrounding whitespace ignored. The macro cannot call that
/// function, so it keeps this copy; a unit test checks that both agree. The error completes a
/// sentence that starts with the key and the method.
fn parse_duration_millis(text: &str) -> Result<u64, &'static str> {
    const INVALID: &str =
        "is not a duration; use a whole number with `ms`, `s`, `m`, `h` or `d`, e.g. \"200ms\"";
    const TOO_LARGE: &str = "is out of range";
    let text = text.trim();
    let digits_end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(digits_end);
    if digits.is_empty() {
        return Err(INVALID);
    }
    let millis_per_unit: u64 = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(INVALID),
    };
    let amount: u64 = digits.parse().map_err(|_| TOO_LARGE)?;
    amount.checked_mul(millis_per_unit).ok_or(TOO_LARGE)
}

pub(crate) fn expand(args: TokenStream2, input: TokenStream2) -> TokenStream2 {
    let mut item: ImplItemFn = match syn::parse2(input.clone()) {
        Ok(item) => item,
        Err(_) => {
            // The call site of an attribute macro is the attribute itself.
            let error = Error::new(
                Span::call_site(),
                "#[retryable] can only be used on an `async fn` method that returns a `Result`",
            )
            .into_compile_error();
            return quote! { #error #input };
        }
    };
    let method = item.sig.ident.clone();

    let mut errors: Option<Error> = None;
    let mut push = |error: Error| match &mut errors {
        Some(all) => all.combine(error),
        None => errors = Some(error),
    };
    let mut parsed = Args::default();
    let parser = syn::meta::parser(|meta| parsed.parse(&meta, &method));
    if let Err(error) = parser.parse2(args) {
        push(error);
    }
    if let (Some((initial, initial_ms)), Some((_, max_ms))) = (&parsed.initial, &parsed.max_delay)
        && initial_ms > max_ms
    {
        push(Error::new(
            initial.span(),
            format!("`initial` on the retryable method `{method}` is greater than its `max_delay`"),
        ));
    }
    for error in check_signature(&item.sig) {
        push(error);
    }
    if let Some(errors) = errors {
        // The method stays, so its callers do not report further errors.
        let errors = errors.into_compile_error();
        return quote! { #errors #item };
    }

    let label = method.to_string();
    let some = |value: TokenStream2| quote! { ::std::option::Option::Some(#value) };
    let mut fields = Vec::new();
    if let Some(value) = parsed.max_attempts {
        let value = some(quote! { #value });
        fields.push(quote! { max_attempts: #value });
    }
    if let Some(variant) = parsed.backoff {
        let variant = Ident::new(variant, Span::call_site());
        let value = some(quote! { ::oxbloom_tasks::Backoff::#variant });
        fields.push(quote! { backoff: #value });
    }
    if let Some((_, millis)) = parsed.initial {
        let value = some(quote! { #millis });
        fields.push(quote! { initial_millis: #value });
    }
    if let Some(multiplier) = parsed.multiplier {
        let value = some(quote! { #multiplier });
        fields.push(quote! { multiplier: #value });
    }
    if let Some((_, millis)) = parsed.max_delay {
        let value = some(quote! { #millis });
        fields.push(quote! { max_delay_millis: #value });
    }
    if let Some(variant) = parsed.jitter {
        let variant = Ident::new(variant, Span::call_site());
        let value = some(quote! { ::oxbloom_tasks::Jitter::#variant });
        fields.push(quote! { jitter: #value });
    }
    let retry_if = match &parsed.retry_if {
        Some(path) => quote! { #path },
        None => quote! { |_| true },
    };
    // With all six fields given, a struct update would trip Clippy's `needless_update`.
    let rest = (fields.len() < 6).then(|| quote! { ..::oxbloom_tasks::__private::Overrides::NONE });
    let block = &item.block;
    let pin = output_pin(&item.sig.output);
    let span = method.span();
    let body = quote_spanned! {span=>
        ::oxbloom_tasks::__private::retry_call(
            #label,
            ::oxbloom_tasks::__private::Overrides {
                #(#fields,)*
                #rest
            },
            #retry_if,
            || async {
                #pin
                #block
            },
        )
        .await
    };
    item.block = syn::parse_quote_spanned! {span=> { #body } };
    quote! { #item }
}

/// A statement that fixes the output type of the generated `async` block to the method's
/// declared return type before the body is checked.
///
/// An `async` block gets no expected type from its surroundings, so without it the tail
/// expression and every `return` of the body would lose the coercions of a normal method
/// (`Ok(&self.name)` as `Result<&str, E>`, `Err("..".into())` as `Box<dyn Error>`). The first
/// `return` of a block fixes its output type, and this one is never taken. It calls a generic
/// function instead of diverging, so neither `unreachable_code` nor Clippy's
/// `diverging_sub_expression` fires in the user's crate. A return type that
/// contains `impl Trait` cannot be named in a `let`, so it gets no statement.
fn output_pin(output: &ReturnType) -> TokenStream2 {
    let ReturnType::Type(_, ty) = output else {
        return TokenStream2::new();
    };
    if contains_impl(quote! { #ty }) {
        return TokenStream2::new();
    }
    quote! {
        if false {
            return ::oxbloom_tasks::__private::never::<#ty>();
        }
    }
}

/// Whether the tokens contain the keyword `impl`, also inside groups.
fn contains_impl(tokens: TokenStream2) -> bool {
    tokens.into_iter().any(|token| match token {
        proc_macro2::TokenTree::Ident(ident) => ident == "impl",
        proc_macro2::TokenTree::Group(group) => contains_impl(group.stream()),
        _ => false,
    })
}

/// A retryable method is `async`, takes `&self` or no receiver, has no `mut` argument and no
/// generic parameters, and returns a `Result`. Returns every violation.
fn check_signature(sig: &syn::Signature) -> Vec<Error> {
    let method = &sig.ident;
    let mut errors = Vec::new();
    if sig.asyncness.is_none() {
        let span = sig.fn_token.span;
        errors.push(Error::new(
            span,
            format!(
                "the retryable method `{method}` must be `async`; #[retryable] sleeps between \
                 attempts"
            ),
        ));
    }
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        errors.push(Error::new_spanned(
            &sig.generics,
            format!("the retryable method `{method}` cannot have generic parameters"),
        ));
    }
    for input in &sig.inputs {
        match input {
            FnArg::Receiver(rec) => {
                let by_ref = rec.mutability.is_none()
                    && match &rec.kind {
                        ReceiverKind::Reference(_, _, None) => true,
                        ReceiverKind::Typed(_, ty) => is_ref_self(ty),
                        _ => false,
                    };
                if !by_ref {
                    errors.push(Error::new_spanned(
                        rec,
                        format!(
                            "the retryable method `{method}` must take `&self` or no receiver: \
                             every attempt runs the body again on the same instance"
                        ),
                    ));
                }
            }
            FnArg::Typed(arg) => {
                if let Pat::Ident(ident) = &*arg.pat
                    && let Some(mutability) = &ident.mutability
                {
                    errors.push(Error::new_spanned(
                        mutability,
                        format!(
                            "the argument `{}` of the retryable method `{method}` cannot be \
                             `mut`: every attempt runs the body again with the same arguments; \
                             clone it into a local variable instead",
                            ident.ident
                        ),
                    ));
                }
            }
        }
    }
    let message =
        format!("the retryable method `{method}` must return `Result<T, E>` where `E: Display`");
    match &sig.output {
        ReturnType::Default => errors.push(Error::new(method.span(), message)),
        ReturnType::Type(_, ty) if !is_result(ty) => {
            errors.push(Error::new_spanned(ty, message));
        }
        ReturnType::Type(..) => {}
    }
    errors
}

/// Whether `ty` is `&Self` or `&'a Self` (not `mut`).
fn is_ref_self(ty: &Type) -> bool {
    let Type::Reference(r) = ty else {
        return false;
    };
    r.mutability.is_none()
        && matches!(&*r.elem, Type::Path(p) if p.qself.is_none() && p.path.is_ident("Self"))
}

/// Whether `ty` is a path ending in `Result<..>`: `Result<T, E>`, or an alias such as
/// `io::Result<T>`. That the error is `Display` is checked by the compiler.
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::parse_duration_millis;

    #[test]
    fn duration_grammar_agrees_with_parse_duration() {
        let table = [
            "0ms",
            "250ms",
            "5s",
            "2m",
            "1h",
            "1d",
            " 5s ",
            "5",
            "s",
            "1.5s",
            "-1s",
            "99999999999999999999d",
            "",
        ];
        for text in table {
            let ours = parse_duration_millis(text).map(Duration::from_millis);
            let theirs = oxbloom_tasks::parse_duration(text);
            assert_eq!(
                ours.is_ok(),
                theirs.is_ok(),
                "{text:?}: {ours:?} vs {theirs:?}"
            );
            if let (Ok(ours), Ok(theirs)) = (ours, theirs) {
                assert_eq!(ours, theirs, "{text:?}");
            }
        }
        assert_eq!(parse_duration_millis("250ms"), Ok(250));
        assert_eq!(parse_duration_millis(" 5s "), Ok(5_000));
        assert_eq!(parse_duration_millis("1d"), Ok(86_400_000));
    }

    #[test]
    fn duration_errors_tell_invalid_from_out_of_range() {
        assert!(
            parse_duration_millis("soon")
                .unwrap_err()
                .starts_with("is not a duration")
        );
        assert_eq!(
            parse_duration_millis("99999999999999999999d"),
            Err("is out of range")
        );
        assert_eq!(
            parse_duration_millis("213503982334602d"),
            Err("is out of range")
        );
    }
}
