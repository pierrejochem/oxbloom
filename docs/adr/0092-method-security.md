# ADR 0092: Method security is woven at compile time from typed rules and reads a task-local security context that fails closed

- **Status:** Accepted
- **Date:** 2026-11-23
- **Source:** the security Slice 8 design spec `2026-11-23-method-security-design.md` (a local file, not linked) and its plan

## Context

After Slice 7b the security chain decides by request path. That is too coarse when the same service method is reached from several places, such as a route, a scheduled task or another service, or when the decision needs the value that the method returns or one of its arguments. Spring Security answers this with `@PreAuthorize`, `@PostAuthorize`, `@PreFilter` and `@PostFilter`, which run through proxies and an expression language. How does Oxbloom offer the same protection without a proxy layer, without a second language to learn and parse at run time, and without a way for a check to silently run for the wrong caller?

The chain is [ADR 0011](0011-one-deny-by-default-security-chain.md), and its sessions, form login and remember-me are [ADR 0087](0087-sessions.md), [ADR 0088](0088-form-login.md) and [ADR 0091](0091-remember-me.md). [ADR 0012](0012-security-context-in-request-extensions.md) kept the security context in the request extensions and not in a thread-local; that stays true for handlers and filters, which read the extension, and the task-local of this ADR is an addition that the chain fills from the same context for code that has no request in hand.

## Decision

**The caller is an ambient, task-local context.** A secured method has no parameter for the caller. The chain runs the rest of the request inside `SecurityContext::scope(context, ..)`, which stores the `SecurityContext` in a Tokio task-local, and the generated check reads it with `SecurityContext::current()`. Outside any scope `current()` returns the anonymous context. A task-local follows the future across `.await` points and across worker threads, it needs no thread-local cleanup, and it is dropped with the future. The check runs when an `async` method is first polled and not when the method is called, so the caller must await the future inside the scope.

**A plain `tokio::spawn` fails closed.** A task-local is not inherited by a spawned task, so code that calls a secured method from a bare `tokio::spawn` sees the anonymous context and is denied. That is deliberate: losing the caller must never grant access. `oxbloom_security::spawn` copies the current context into the new task for code that wants the caller to follow, and `oxbloom_test::with_authentication` (feature `security`) sets a caller for a test without HTTP. The alternative, inheriting the context into every spawned task, would let a background job that outlives the request act with the rights of a caller who is long gone.

**Rules are typed and checked at compile time.** `#[method_security]` goes on an inherent `impl` block and the attributes `#[pre_authorize]`, `#[post_authorize]`, `#[pre_filter]` and `#[post_filter]` go on its methods. A rule is one of `role`, `any_role`, `authority`, `any_authority`, `authenticated`, `fully_authenticated`, `permit_all`, `deny_all`, the combinators `all`, `any` and `not`, or `custom = check(arg, ..)` with a plain synchronous function `fn(Option<&Authentication>, &A, ..) -> bool`. There is no expression language: an unknown rule, a name that is not a parameter and a wrong return type are compile errors with a span on the attribute. The role and authority rules call the same functions as the path rules of the chain, so a role means the same thing in both places.

**Weaving happens at compile time, not through proxies.** The macro rewrites the method in place. The check is code inside the method, so it holds for every caller, a direct call included, and no container proxy can be bypassed by `self.other()` or by building the value by hand. The cost is that the attribute works only on inherent impls and that the method signature is constrained: a method with `pre_authorize`, `post_authorize` or `post_filter` must return `Result<T, E>` with `E: From<AccessDenied>`. `#[pre_filter]` alone changes only an argument and allows any return type, including `impl Trait`.

**The order is fixed.** The steps run as `pre_filter`, `pre_authorize`, the body, `post_authorize` and `post_filter`, whatever the order of the attributes in the source. A reader can therefore not be surprised by a refactoring that moves an attribute, and a denied `pre_authorize` never runs the body or the post checks. Filters never deny. `post_authorize` sees `returned`, a reference to the `Ok` value, and an `Err` from the body passes through without a check.

**The 401 and 403 status codes match the chain.** A denial is `AccessDenied::Unauthenticated` for an anonymous caller and `AccessDenied::Forbidden` for an authenticated one. `HttpError` converts them to 401 and 403, the same status codes as the filter chain gives for the path rules. Only the status codes match: a method denial goes through the error filter, not through the entry point or the `AccessDeniedHandler`, so a 401 from a method carries no `WWW-Authenticate` challenge, a form-login application does not redirect an anonymous browser to the login page, and a custom 403 handler is skipped. A denial logs the method (`Type::method`) and the principal at debug level on the target `oxbloom::security`, and never an argument or a returned value.

**Early returns in async methods with post checks are rewritten with a labeled break.** A post check must run after every exit of the body, so the body is wrapped: in a closure for a synchronous method and in an `async` block for an `async` one. A closure turns `return` into leaving the closure, which is what is wanted. In an `async` block a `return` written by the user would leave the block with the type of the block, and a coercion in `return Ok(&self.name)` for `Result<&str, E>` would fail with E0308. So for an `async` method the macro rewrites each `return x` of the body into `break 'label x` out of a block that has the declared return type, which keeps the coercions of the declared type. A `return` inside a macro call cannot be rewritten because the macro sees only tokens; it still leaves only the body, but a coercion there fails with E0308 at the attribute. That is a documented limitation. Methods that have no post attribute are not wrapped and have none of this.

**The split.** Slice 8 is the context, the four attributes, the chain scoping the context, `spawn` and `with_authentication`. There are no configuration keys. Asynchronous `custom` checks, an expression language and caching of decisions stay out.

## Consequences

Good:

- A rule on a service method holds for every caller, whether the call comes from a route, a scheduled task, another bean or a test.
- The same role and authority code decides paths and methods, and a denial gives the same 401 or 403 status code.
- Everything is checked by the compiler: a typo in a rule or a name that is not a parameter does not reach production.
- There is no run-time parsing, no proxy and no reflection, and an unsecured method costs nothing.
- Losing the context by a plain `tokio::spawn` denies instead of granting.
- The check never logs an argument or a returned value.
- The boot stack probes stay as they were.

Bad:

- A background task must use `oxbloom_security::spawn` to keep the caller, and code that forgets is denied, which shows up as a 401 in a place that looked fine in a test with a scope around it.
- An `async` method is checked at its first poll, so a future that is created in one scope and awaited in another is checked against the second.
- A secured method must return `Result<T, E>` with `E: From<AccessDenied>`, so a plain `fn() -> u32` cannot carry `pre_authorize`.
- A post check cannot name a parameter that the body moved, a pattern parameter such as `(a, b): (u32, u32)` cannot be named in `custom`, and the name `returned` is reserved in `post_authorize`.
- In an `async` method with a post attribute, a `return` inside a macro call that needs a coercion fails with E0308 (documented limitation). A trailing `return Ok(x);` in such a method becomes a `break`, so `clippy::needless_return` no longer reports it.
- `custom` checks are synchronous: a check that needs a database lookup cannot be written yet.
- The roles of an `HasAnyRole` or `HasAnyAuthority` rule of the chain allocate a small `Vec<&str>` per call, because the shared function takes a slice.
- Each check clones the `SecurityContext` of the caller.

## Alternatives considered

- Proxies around beans in the container, as in Spring: rejected. They are bypassed by self calls and by values built by hand, they need dynamic dispatch for every secured bean, and Rust has no runtime proxy generation.
- An expression language such as SpEL: rejected for this slice. It needs a parser and an evaluator at run time, errors show up at run time, and `custom = function` covers the cases that matter with the compiler's help.
- A thread-local context: rejected, because an `async` task moves between threads at every `.await`.
- Inheriting the context into every `tokio::spawn`: not possible with a plain `tokio::spawn`, and a wrapper that did it by default would let a long running job act as a caller that has left.
- Passing the `Authentication` as an explicit parameter: rejected, because every secured method and each of its callers would change, and a caller could pass the wrong one.
- A pre check that runs at the call of an `async` method instead of at the first poll: rejected, because the method would need to return a hand-written future instead of staying an `async fn`.
- Caching the decision of a rule per request: parked, since a decision may depend on arguments and a stale grant is worse than a repeated check.
