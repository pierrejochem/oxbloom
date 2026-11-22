//! The HTTP side: the CRUD controller and the security rules.
//!
//! | Request | Result |
//! |---|---|
//! | `POST /api/todos` | 201, `Location: /api/todos/{id}`, the new todo |
//! | `GET /api/todos[?done=true\|false]` | 200, a JSON array |
//! | `GET /api/todos/{id}` | 200, the todo |
//! | `PUT /api/todos/{id}` | 200, full replace (`title` and `done`) |
//! | `PATCH /api/todos/{id}` | 200, any of `title`, `done` |
//! | `DELETE /api/todos/{id}` | 204, empty body |
//!
//! Bodies are read with `Request::json` and answers are built with `Json<T>`.
//! A request body needs `Content-Type: application/json`, otherwise the answer
//! is 415.
//!
//! Errors are `HttpError` values, rendered as `application/problem+json`: 400
//! for an unparsable body, a bad `done` filter or a non-numeric id, 404 for an
//! unknown id (also for `PUT` and `PATCH`, which never create), 415 for a wrong
//! `Content-Type` and 422 for an empty or blank title. For example
//! `{"type":"about:blank","title":"Not Found","status":404,"detail":"no todo with this id","instance":"/api/todos/99"}`.

use std::sync::Arc;

use http::{HeaderValue, StatusCode, header};
use oxbloom_boot::SecurityCustomizer;
use oxbloom_di::component;
use oxbloom_security::{HttpSecurity, RequestSecurityExt, RequestSessionExt};
use oxbloom_web::{HttpError, IntoResponse, Json, Request, Response, controller};
use serde::{Deserialize, Serialize};

use crate::todo::TodoStore;

/// The body of `POST`.
#[derive(Debug, Deserialize)]
struct NewTodo {
    title: String,
}

/// The body of `PUT`.
#[derive(Debug, Deserialize)]
struct FullTodo {
    title: String,
    done: bool,
}

/// The body of `PATCH`.
#[derive(Debug, Deserialize)]
struct TodoChanges {
    title: Option<String>,
    done: Option<bool>,
}

/// The body of `GET /api/admin/stats`.
#[derive(Debug, Serialize)]
struct Stats {
    total: usize,
    done: usize,
}

/// The body of `GET /api/visits`.
#[derive(Debug, Serialize)]
struct Visits {
    visits: u64,
}

/// A 404 as an `HttpError`; the error filter renders it as problem+json.
fn not_found() -> Response {
    HttpError::new(StatusCode::NOT_FOUND)
        .detail("no todo with this id")
        .into_response()
}

/// A 422 as an `HttpError`; the error filter renders it as problem+json.
fn blank_title() -> Response {
    HttpError::new(StatusCode::UNPROCESSABLE_ENTITY)
        .detail("the title must not be empty")
        .into_response()
}

/// The `{id}` path parameter, or a 400 `HttpError`.
#[allow(clippy::result_large_err)]
fn id_param(req: &Request) -> Result<u64, HttpError> {
    req.param("id")
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| {
            HttpError::new(StatusCode::BAD_REQUEST).detail("the id must be a whole number")
        })
}

/// The admin controller for `/api/admin`: one route. The rule that guards it (HTTP Basic,
/// role `ADMIN`) is in [`PublicPaths`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_web::{App, HttpMethod, Request};
/// use todos::api::{AdminController, TodoController};
/// use todos::todo::TodoStore;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let store = Arc::new(TodoStore::default());
/// store.create("one".into());
/// let controller = Arc::new(AdminController { store });
/// let app = App::new().routes(controller.routes())?;
/// let res = app.handle(Request::new(HttpMethod::Get, "/api/admin/stats")).await;
/// assert_eq!(res.status(), 200);
/// # Ok(())
/// # }
/// ```
#[component]
pub struct AdminController {
    /// The store the statistics are counted on.
    pub store: Arc<TodoStore>,
}

#[controller("/api/admin")]
impl AdminController {
    /// `GET /api/admin/stats`: the numbers of todos, as `{"total":..,"done":..}`. The
    /// security rules let only the role `ADMIN` in.
    #[get("/stats")]
    async fn stats(&self, _req: Request) -> Response {
        let all = self.store.list(None);
        let done = all.iter().filter(|t| t.done).count();
        Json(&Stats {
            total: all.len(),
            done,
        })
        .into_response()
    }
}

/// The visit counter for `/api/visits`: one route that keeps its count in the session.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_web::{App, HttpMethod, Request};
/// use todos::api::VisitController;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // Without a session filter the route answers 503; the application's
/// // `security.session` section adds the filter.
/// let app = App::new().routes(Arc::new(VisitController).routes())?;
/// let res = app.handle(Request::new(HttpMethod::Get, "/api/visits")).await;
/// assert_eq!(res.status(), 503);
/// # Ok(())
/// # }
/// ```
#[component]
pub struct VisitController;

#[controller("/api/visits")]
impl VisitController {
    /// `GET /api/visits`: adds one to the counter in the session and answers
    /// `{"visits":n}`. The first request of a session answers 1. Without the session
    /// filter (no `security.session` section) the answer is 503.
    #[get("")]
    async fn count(&self, req: Request) -> Response {
        let Some(session) = req.session() else {
            return HttpError::new(StatusCode::SERVICE_UNAVAILABLE)
                .detail("sessions are not configured")
                .into_response();
        };
        let visits = match session.get::<u64>("visits") {
            Ok(count) => count.unwrap_or(0).saturating_add(1),
            Err(_) => 1,
        };
        if session.set("visits", &visits).is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Json(&Visits { visits }).into_response()
    }
}

/// The protected page for `/me`: an HTML page that shows who is logged in, with a logout button
/// and a button that resets the visit counter. Both are forms that post the CSRF token in the
/// hidden field `_csrf`. It is not in [`PublicPaths`], so the last rule applies: a browser
/// without a login is redirected to `/login` (served by Oxbloom's default page) and returns here
/// after it.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_security::{Authentication, SecurityContext};
/// use oxbloom_web::{App, HttpMethod, Request};
/// use todos::api::MeController;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = App::new().routes(Arc::new(MeController).routes())?;
/// let mut req = Request::new(HttpMethod::Get, "/me");
/// req.extensions_mut()
///     .insert(SecurityContext::authenticated(Authentication::new("<b>x</b>", [])));
/// let res = app.handle(req).await;
/// assert_eq!(res.status(), 200);
/// let body = String::from_utf8(res.body().to_vec())?;
/// assert!(body.contains("&lt;b&gt;x&lt;/b&gt;"));
/// // Without a session filter there is no token, so the forms have no hidden field.
/// assert!(!body.contains("_csrf"));
/// # Ok(())
/// # }
/// ```
#[component]
pub struct MeController;

/// Escapes a value for HTML text and for a double-quoted attribute.
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[controller("/me")]
impl MeController {
    /// `GET /me`: the name of the logged-in user, escaped, and two forms, one that posts to
    /// `/logout` and one that posts to `/me/reset-visits`. Each form holds the CSRF token of the
    /// session in a hidden field named `_csrf`, escaped like the name. Without an authentication
    /// (the rules keep that from happening) the answer is 401.
    #[get("")]
    async fn me(&self, req: Request) -> Response {
        let Some(auth) = req.authentication() else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        let name = escape_html(auth.principal());
        // The token exists when a session filter ran. The field name is the default; an
        // application that sets `security.csrf.field-name` must use that name here.
        let hidden = req.csrf_token().map_or_else(String::new, |token| {
            format!(
                "<input type=\"hidden\" name=\"_csrf\" value=\"{}\">",
                escape_html(token.as_str())
            )
        });
        let body = format!(
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>Me</title></head>\n\
             <body>\n<h1>Hello, {name}</h1>\n\
             <form method=\"post\" action=\"/me/reset-visits\">{hidden}<button type=\"submit\">Reset the visit counter</button></form>\n\
             <form method=\"post\" action=\"/logout\">{hidden}<button type=\"submit\">Log out</button></form>\n\
             </body>\n</html>\n"
        );
        let mut res = Response::new(body.into());
        let h = res.headers_mut();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        res
    }

    /// `POST /me/reset-visits`: removes the visit counter of `GET /api/visits` from the session
    /// and redirects to `/me` with 303. The request is a state change of a logged-in session, so
    /// the chain refuses it with 403 unless it carries the CSRF token.
    #[post("/reset-visits")]
    async fn reset_visits(&self, req: Request) -> Response {
        let Some(session) = req.session() else {
            return HttpError::new(StatusCode::SERVICE_UNAVAILABLE)
                .detail("sessions are not configured")
                .into_response();
        };
        session.remove("visits");
        let mut res = Response::new(Default::default());
        *res.status_mut() = StatusCode::SEE_OTHER;
        res.headers_mut()
            .insert(header::LOCATION, HeaderValue::from_static("/me"));
        res
    }
}

/// The CRUD controller for `/api/todos`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_web::{App, HttpMethod, Request};
/// use todos::api::TodoController;
/// use todos::todo::TodoStore;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let controller = Arc::new(TodoController { store: Arc::new(TodoStore::default()) });
/// let app = App::new().routes(controller.routes())?;
///
/// let mut req = Request::new(HttpMethod::Post, "/api/todos");
/// req.headers.insert("content-type", "application/json".parse()?);
/// req.body = r#"{"title":"try it"}"#.into();
/// let res = app.handle(req).await;
/// assert_eq!(res.status(), 201);
/// assert_eq!(res.headers()["location"], "/api/todos/1");
///
/// let res = app.handle(Request::new(HttpMethod::Get, "/api/todos/1")).await;
/// assert_eq!(res.status(), 200);
/// # Ok(())
/// # }
/// ```
#[component]
pub struct TodoController {
    /// The store the handlers work on.
    pub store: Arc<TodoStore>,
}

#[controller("/api/todos")]
impl TodoController {
    /// `POST /api/todos`: creates a todo from `{"title": ".."}`. The empty
    /// route path means the controller's base path.
    #[post("")]
    async fn create(&self, req: Request) -> Response {
        let body: NewTodo = match req.json() {
            Ok(b) => b,
            Err(err) => return err.into_response(),
        };
        if body.title.trim().is_empty() {
            return blank_title();
        }
        let todo = self.store.create(body.title);
        tracing::info!(target: "todos", id = todo.id, "created todo");
        let mut res = (StatusCode::CREATED, Json(&todo)).into_response();
        if let Ok(location) = HeaderValue::from_str(&format!("/api/todos/{}", todo.id)) {
            res.headers_mut().insert(header::LOCATION, location);
        }
        res
    }

    /// `GET /api/todos`: all todos, optionally `?done=true` or `?done=false`.
    #[get("")]
    async fn list(&self, req: Request) -> Response {
        let done = match req
            .query
            .as_deref()
            .into_iter()
            .flat_map(|q| q.split('&'))
            .find_map(|pair| pair.strip_prefix("done="))
        {
            None => None,
            Some("true") => Some(true),
            Some("false") => Some(false),
            Some(_) => {
                return HttpError::new(StatusCode::BAD_REQUEST)
                    .detail("`done` must be true or false")
                    .into_response();
            }
        };
        Json(&self.store.list(done)).into_response()
    }

    /// `GET /api/todos/{id}`: one todo.
    #[get("/{id}")]
    async fn get(&self, req: Request) -> Response {
        let id = match id_param(&req) {
            Ok(id) => id,
            Err(err) => return err.into_response(),
        };
        match self.store.get(id) {
            Some(todo) => Json(&todo).into_response(),
            None => not_found(),
        }
    }

    /// `PUT /api/todos/{id}`: replaces `title` and `done`. Unknown id: 404.
    #[put("/{id}")]
    async fn replace(&self, req: Request) -> Response {
        let id = match id_param(&req) {
            Ok(id) => id,
            Err(err) => return err.into_response(),
        };
        let body: FullTodo = match req.json() {
            Ok(b) => b,
            Err(err) => return err.into_response(),
        };
        if body.title.trim().is_empty() {
            return blank_title();
        }
        match self.store.replace(id, body.title, body.done) {
            Some(todo) => {
                tracing::info!(target: "todos", id, "updated todo");
                Json(&todo).into_response()
            }
            None => not_found(),
        }
    }

    /// `PATCH /api/todos/{id}`: changes the given fields. Unknown id: 404.
    #[patch("/{id}")]
    async fn change(&self, req: Request) -> Response {
        let id = match id_param(&req) {
            Ok(id) => id,
            Err(err) => return err.into_response(),
        };
        let body: TodoChanges = match req.json() {
            Ok(b) => b,
            Err(err) => return err.into_response(),
        };
        if body.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
            return blank_title();
        }
        match self.store.patch(id, body.title, body.done) {
            Some(todo) => {
                tracing::info!(target: "todos", id, "updated todo");
                Json(&todo).into_response()
            }
            None => not_found(),
        }
    }

    /// `DELETE /api/todos/{id}`: removes the todo, 204 with an empty body.
    #[delete("/{id}")]
    async fn remove(&self, req: Request) -> Response {
        let id = match id_param(&req) {
            Ok(id) => id,
            Err(err) => return err.into_response(),
        };
        if self.store.delete(id) {
            tracing::info!(target: "todos", id, "deleted todo");
            StatusCode::NO_CONTENT.into_response()
        } else {
            not_found()
        }
    }
}

/// The security rules. `/api/todos/**` is open so `curl` works out of the box,
/// and so are the actuator's health probes (`GET /actuator/health/**`).
/// `GET /api/visits` is open as well: it counts in the session, which needs no login.
/// `GET /api/admin/stats` needs the role `ADMIN`, through HTTP Basic (see the
/// README). Every other path, including unknown ones and the other actuator
/// endpoints, needs an authenticated request: any user of HTTP Basic, including
/// `viewer`, passes, and an anonymous request gets 401.
///
/// The rules live here, and not behind `management.endpoints.health.public`,
/// because the chain takes only one `authorize` section.
///
/// To require a token for writes, put a rule ahead of the permit rule; the
/// first matching rule wins:
///
/// ```text
/// a.post("/api/todos/**").authenticated()
///     .put("/api/todos/**").authenticated()
///     .patch("/api/todos/**").authenticated()
///     .delete("/api/todos/**").authenticated()
///     .any_request().permit_all() // reads stay open
/// ```
///
/// # Examples
///
/// ```
/// use oxbloom_boot::SecurityCustomizer;
/// use oxbloom_security::HttpSecurity;
/// use todos::api::PublicPaths;
///
/// let http = PublicPaths.customize(HttpSecurity::new());
/// assert!(http.build().is_ok());
/// ```
pub struct PublicPaths;

impl SecurityCustomizer for PublicPaths {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| {
            a.path("/api/todos/**")
                .permit_all()
                .get("/actuator/health/**")
                .permit_all()
                .get("/api/visits")
                .permit_all()
                .get("/api/admin/stats")
                .has_role("ADMIN")
                .any_request()
                .authenticated()
        })
    }
}
