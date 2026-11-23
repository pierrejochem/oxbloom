# Method security

Method security checks the caller inside a method, not at a path. You put an attribute on a method of a service or a controller, and the check runs for every caller of that method: a route, a scheduled task, another bean or a test. The crate `oxbloom-security` holds the runtime and re-exports the attributes from `oxbloom-macros`. This is security Slice 8. It builds on [Password login](19-password-login.md#password-login), [Form login](21-form-login.md#form-login) and [Remember-me](24-remember-me.md#remember-me), whose `Authentication` it reads.

```toml
# Cargo.toml
oxbloom-security = "0.1"
```

The decisions are in [ADR 0092](../adr/0092-method-security.md), the public items are in the [API reference](../API.md#method-security-slice-8), the attributes are documented on `method_security` in `crates/oxbloom-macros/src/lib.rs` (run `cargo doc -p oxbloom-macros --open`), and the example `examples/todos` has a secured service, `TodoAdmin::purge_done`, with `tests/method_security.rs` (see [Example](14-example.md#example)). The design is a local file under `docs/superpowers`, so this page names it without a link. There are no configuration keys.

## A first secured method

Put `#[method_security]` on an inherent `impl` block and one or more method attributes on its methods. A method that has `#[pre_authorize]`, `#[post_authorize]` or `#[post_filter]` returns `Result<T, E>`, where `E` implements `From<AccessDenied>`. `AccessDenied` itself and `HttpError` both do.

```rust
use oxbloom_security::{
    AccessDenied, Authentication, GrantedAuthority, SecurityContext, method_security,
};

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(role = "ADMIN")]
    fn close(&self, id: u64) -> Result<u64, AccessDenied> {
        Ok(id)
    }
}

# tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
let admin = Authentication::new("ada", [GrantedAuthority::new("ROLE_ADMIN")]);
let closed = SecurityContext::scope(SecurityContext::authenticated(admin), async {
    Accounts.close(7)
})
.await;
assert_eq!(closed, Ok(7));

// With no caller in scope the caller is anonymous.
assert_eq!(Accounts.close(7), Err(AccessDenied::Unauthenticated));
# });
# Ok::<(), std::io::Error>(())
```

Inside the `impl` block the macro handles `pre_authorize` and the other three attributes itself, so only `method_security` needs a `use`. The generated code refers to the crate by the name `oxbloom_security`, so do not rename that dependency in `Cargo.toml`. A trait impl is a compile error, and so are an unknown rule, a repeated attribute and a method whose return type is visibly not a `Result`.

## Who is the caller

A secured method has no parameter for the caller. It reads the ambient `SecurityContext` with `SecurityContext::current()`. The security filter chain runs the rest of every request inside `SecurityContext::scope(..)`, so a handler, and every service that the handler awaits, sees the caller of the request. Outside any scope the caller is anonymous.

The context is a Tokio task-local. Three things follow:

- **The check of an `async` method runs at its first poll, not at the call.** Await the future inside the scope. A future that is built in one scope and awaited in another is checked against the second.
- **A bare `tokio::spawn` loses the caller, and the call is denied.** A spawned task does not inherit a task-local, so it sees the anonymous context. That fails closed on purpose: losing the caller never grants access. Use `oxbloom_security::spawn` to start a task that keeps the caller of the code that spawns it.
- **A test sets the caller with `oxbloom_test::with_authentication`** (feature `security` of `oxbloom-test`), without HTTP:

```rust
use oxbloom_security::{AccessDenied, Authentication, GrantedAuthority, method_security, spawn};
use oxbloom_test::with_authentication;

struct Reports;

#[method_security]
impl Reports {
    #[pre_authorize(authenticated)]
    async fn monthly(&self) -> Result<&'static str, AccessDenied> {
        Ok("report")
    }
}

# tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
let user = Authentication::new("ada", [GrantedAuthority::new("ROLE_USER")]);
with_authentication(user, async {
    assert_eq!(Reports.monthly().await, Ok("report"));

    // `spawn` carries the caller into the task.
    let carried = spawn(async { Reports.monthly().await }).await.unwrap();
    assert_eq!(carried, Ok("report"));

    // A bare `tokio::spawn` does not: the task is anonymous and the call is denied.
    let lost = tokio::spawn(async { Reports.monthly().await }).await.unwrap();
    assert_eq!(lost, Err(AccessDenied::Unauthenticated));
})
.await;
# });
# Ok::<(), std::io::Error>(())
```

## Rules

A rule is checked at compile time. These forms exist:

| Rule | Grants |
|---|---|
| `role = "ADMIN"` | a caller with `ROLE_ADMIN` |
| `any_role("ADMIN", "AUDITOR")` | a caller with one of the roles |
| `authority = "accounts:read"` | a caller with exactly that authority |
| `any_authority("a", "b")` | a caller with one of the authorities |
| `authenticated` | any authenticated caller, also one restored from a remember-me cookie |
| `fully_authenticated` | an authenticated caller that is not remembered |
| `permit_all`, `deny_all` | everybody, nobody |
| `all(rule, ..)`, `any(rule, ..)`, `not(rule)` | the combination |
| `custom = check(arg, ..)` | what the function `check` returns |

The roles and authorities are decided by the same code as the path rules of the chain, so `role = "ADMIN"` means what `.has_role("ADMIN")` means there.

A `custom` check is a plain synchronous function `fn(Option<&Authentication>, &A, ..) -> bool`. Each name after the function is a parameter of the method, passed by reference. The name has to be a plain parameter name: a pattern parameter such as `(a, b): (u32, u32)` cannot be named.

```rust
use oxbloom_security::{
    AccessDenied, Authentication, GrantedAuthority, SecurityContext, method_security,
};

/// True when the caller owns the account.
fn owns(auth: Option<&Authentication>, owner: &String) -> bool {
    auth.is_some_and(|a| a.principal() == owner)
}

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(any(role = "ADMIN", custom = owns(owner)))]
    fn rename(&self, owner: String, name: &str) -> Result<String, AccessDenied> {
        Ok(format!("{owner}: {name}"))
    }
}

# tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
let bob = SecurityContext::authenticated(Authentication::new("bob", []));
let own = SecurityContext::scope(bob.clone(), async { Accounts.rename("bob".into(), "main") });
assert_eq!(own.await.as_deref(), Ok("bob: main"));
let other = SecurityContext::scope(bob, async { Accounts.rename("eve".into(), "main") });
assert_eq!(other.await, Err(AccessDenied::Forbidden));
# });
# Ok::<(), std::io::Error>(())
```

## Checks after the body and filters

`#[post_authorize(rule)]` runs after the body returned `Ok(value)`. In `custom = check(returned)` the reserved name `returned` stands for `&value`. A denial drops the value and returns `Err`, and an `Err` from the body passes through without a check. The name `returned` is reserved even when the method has a parameter of that name, and a post check can name a parameter only when the body did not move it.

`#[pre_filter(arg = ids, custom = keep)]` keeps the elements of the by-value `Vec<T>` parameter `ids` for which `keep(auth, &element)` is true, before the body runs. `#[post_filter(custom = keep)]` does the same for the collection in an `Ok` result: a `Vec<T>` or any type with a `retain` method that takes a `FnMut(&T) -> bool`, such as `HashSet<T>`. Filters never deny; they shrink the collection, possibly to an empty one.

The order is fixed, whatever the order of the attributes in the source: `pre_filter`, `pre_authorize`, the body, `post_authorize`, `post_filter`. A denied `pre_authorize` runs neither the body nor the post checks.

```rust
use oxbloom_security::{AccessDenied, Authentication, SecurityContext, method_security};

#[derive(Debug, PartialEq)]
struct Note {
    owner: String,
    text: String,
}

/// True when the caller wrote the note.
fn wrote(auth: Option<&Authentication>, note: &Note) -> bool {
    auth.is_some_and(|a| a.principal() == note.owner)
}

/// True for the ids an authenticated caller may see: the even ones.
fn visible(auth: Option<&Authentication>, id: &u64) -> bool {
    auth.is_some() && id.is_multiple_of(2)
}

struct Notes;

#[method_security]
impl Notes {
    #[post_authorize(custom = wrote(returned))]
    fn open(&self, owner: &str) -> Result<Note, AccessDenied> {
        Ok(Note { owner: owner.into(), text: "hello".into() })
    }

    #[post_filter(custom = visible)]
    fn list(&self) -> Result<Vec<u64>, AccessDenied> {
        Ok(vec![1, 2, 3, 4])
    }
}

# tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
let bob = SecurityContext::authenticated(Authentication::new("bob", []));
let mine = SecurityContext::scope(bob.clone(), async { Notes.open("bob") }).await;
assert_eq!(mine.map(|n| n.text).as_deref(), Ok("hello"));
let theirs = SecurityContext::scope(bob.clone(), async { Notes.open("alice") }).await;
assert_eq!(theirs, Err(AccessDenied::Forbidden));
let listed = SecurityContext::scope(bob, async { Notes.list() }).await;
assert_eq!(listed, Ok(vec![2, 4]));
# });
# Ok::<(), std::io::Error>(())
```

A method with only `#[pre_filter]` may return any type, including `impl Trait`. With a post attribute the declared return type must be written out, because the body is wrapped.

## Denial and the answer of a handler

A failed rule returns `Err(AccessDenied::Unauthenticated.into())` for an anonymous caller and `Err(AccessDenied::Forbidden.into())` for any other caller, and the body does not run. `HttpError` converts them to 401 and 403, the same status codes that the chain gives for a path rule. Only the status codes match: a method denial goes through the error filter and not through the entry point or the `AccessDeniedHandler` of the chain (see Limits). So a handler that returns `Result<T, HttpError>` needs no mapping, and `?` on a secured service call is enough.

`#[method_security]` combines with `#[controller]` or `#[configuration]` on the same block, in either order, and both orders are tested, so one handler can carry a route and a rule. Write `#[method_security]` as the outermost attribute: it then always sees its method attributes before any other macro handles the block.

The controller form:

```rust
use std::sync::Arc;
use oxbloom_security::{SecurityContext, method_security};
use oxbloom_web::{App, HttpError, HttpMethod, Request, controller};

struct Admin;

#[controller("/admin")]
#[method_security]
impl Admin {
    #[get("/stats")]
    #[pre_authorize(role = "ADMIN")]
    async fn stats(&self, _req: Request) -> Result<&'static str, HttpError> {
        Ok("42 users")
    }
}

# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let app = App::new().routes(Arc::new(Admin).routes())?;
let res = SecurityContext::scope(
    SecurityContext::anonymous(),
    app.handle(Request::new(HttpMethod::Get, "/admin/stats")),
)
.await;
assert_eq!(res.status(), 401);
# Ok(())
# }
```

A denial writes one debug event at the target `oxbloom::security` that names the method (`Type::method`) and the principal. It never logs an argument value or a returned value. To see the events, set `logging.level."oxbloom::security" = "debug"`.

## Limits

- **The caller is the task-local context.** A bare `tokio::spawn` loses it and denies; use `oxbloom_security::spawn`. Code that runs on another thread outside a future has no context and is anonymous.
- **The check of an `async` method runs at the first poll.**
- **A post check cannot name a parameter that the body moved**, and a pattern parameter cannot be named in `custom`. The name `returned` is reserved in `post_authorize`.
- **Early returns in an `async` method with a post attribute.** The macro rewrites each `return` of the body so that it keeps the coercions of the declared return type. A `return` written inside a macro call cannot be rewritten, and when it needs a coercion the compiler reports E0308 at the attribute line. Write `Ok(value.into())` there, or use a plain tail expression. A trailing `return Ok(x);` in such a method becomes a `break`, so `clippy::needless_return` no longer reports it.
- **Return types.** A method with `pre_authorize`, `post_authorize` or `post_filter` must return `Result<T, E>` with `E: From<AccessDenied>`, and with a post attribute the type must be written out (no `impl Trait`).
- **`custom` checks are synchronous.** A check that needs an asynchronous lookup is not possible yet.
- **There is no expression language and no caching** of decisions; every call is checked again.
- **Only inherent impls.** A trait impl is a compile error.
- **Method denials do not use the entry point or the access-denied handler of the chain.** A method denial goes through the error filter and only its status codes (401 and 403) match the chain. A 401 from a method carries no `WWW-Authenticate` challenge, a form-login application does not redirect an anonymous browser to the login page, and a custom 403 handler is skipped.
- **`custom = f(self)` cannot name the receiver.** The compiler reports that `self` is not a parameter of the method.
- **`AccessDenied` must reach `HttpError` to map to 401 and 403.** Only the `From<AccessDenied> for HttpError` conversion maps. Converting `AccessDenied` into an error type such as `anyhow::Error` or `Box<dyn Error>` loses the mapping, and the handler then answers 500.
- **There are no configuration keys.**
