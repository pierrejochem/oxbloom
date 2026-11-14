//! The HTTP side: a controller and the security rules.
//!
//! Shows `#[controller]` with injected trait beans and a `SecurityCustomizer`;
//! the filter chain and the `WebServer` service come from the web and security
//! auto-configurations of `oxbloom-boot`.

use std::sync::Arc;

use oxbloom_boot::SecurityCustomizer;
use oxbloom_di::component;
use oxbloom_security::{HttpSecurity, RequestSecurityExt};
use oxbloom_web::{IntoResponse, Request, controller};

use crate::greeting::Greeter;
use crate::lifecycle::VisitCounter;

/// Feature: `#[component]` with injected fields. `primary` is the single
/// `dyn Greeter` marked `primary`; `all` holds every provider, by `order`.
#[component]
pub struct HelloController {
    primary: Arc<dyn Greeter>,
    all: Vec<Arc<dyn Greeter>>,
    visits: Arc<VisitCounter>,
}

#[controller("/api")]
impl HelloController {
    /// `GET /api/hello`: the primary greeter, public.
    #[get("/hello")]
    async fn index(&self, _req: Request) -> impl IntoResponse {
        self.visits.record();
        self.primary.greet("world")
    }

    /// `GET /api/hello/{name}`: the primary greeter for `name`, public.
    #[get("/hello/{name}")]
    async fn greet(&self, req: Request) -> impl IntoResponse {
        self.visits.record();
        self.primary.greet(req.param("name").unwrap_or("stranger"))
    }

    /// `GET /api/greetings/{name}`: every greeter, one line each, public.
    #[get("/greetings/{name}")]
    async fn all(&self, req: Request) -> impl IntoResponse {
        self.visits.record();
        let name = req.param("name").unwrap_or("stranger");
        let lines: Vec<String> = self
            .all
            .iter()
            .map(|g| format!("{}: {}", g.language(), g.greet(name)))
            .collect();
        lines.join("\n")
    }

    /// `GET /api/admin/visits`: needs a valid bearer token.
    #[get("/admin/visits")]
    async fn visits(&self, req: Request) -> impl IntoResponse {
        let user = req.authentication().map(|a| a.principal().to_owned());
        format!(
            "{} visits so far, asked by {}",
            self.visits.count(),
            user.unwrap_or_default()
        )
    }
}

/// Feature: a `SecurityCustomizer` bean. The auto-configured filter chain
/// applies it: the greeting routes are public; everything else, including
/// unknown paths, needs an authenticated request. Bearer tokens are verified
/// only when `security.jwt` is configured; without it every protected request
/// gets 401.
pub struct PublicPaths;

impl SecurityCustomizer for PublicPaths {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| {
            a.get("/api/hello/**")
                .permit_all()
                .get("/api/greetings/**")
                .permit_all()
                .any_request()
                .authenticated()
        })
    }
}
