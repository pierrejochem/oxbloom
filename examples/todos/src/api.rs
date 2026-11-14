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
use oxbloom_security::HttpSecurity;
use oxbloom_web::{HttpError, IntoResponse, Json, Request, Response, controller};
use serde::Deserialize;

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

/// The security rules. `/api/todos/**` is open so `curl` works out of the box;
/// every other path, including unknown ones, needs an authenticated request
/// (without a configured `security.jwt` that means 401).
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
                .any_request()
                .authenticated()
        })
    }
}
