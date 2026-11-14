#![cfg(feature = "di")]

use std::sync::Arc;

use oxbloom_di::{ContainerBuilder, DiError, component};
use oxbloom_web::{App, AppError, HttpMethod, IntoResponse, Request, RouteError, controller};

#[component]
struct Greeter {}

impl Greeter {
    fn text(&self) -> &'static str {
        "hi"
    }
}

#[component]
struct GreetController {
    greeter: Arc<Greeter>,
}

#[controller("/g")]
impl GreetController {
    #[get("")]
    async fn greet(&self, _req: Request) -> impl IntoResponse {
        self.greeter.text()
    }
}

fn container() -> oxbloom_di::Container {
    ContainerBuilder::new()
        .component::<GreetController>()
        .component::<Greeter>()
        .build()
        .unwrap()
}

#[tokio::test]
async fn controller_gets_service_from_container() {
    let app = App::new()
        .controller::<GreetController>(&container())
        .unwrap();
    let res = app.handle(Request::new(HttpMethod::Get, "/g")).await;
    assert_eq!(res.body(), "hi");
}

#[test]
fn unregistered_controller_is_di_missing() {
    let empty = ContainerBuilder::new().build().unwrap();
    let err = App::new()
        .controller::<GreetController>(&empty)
        .err()
        .unwrap();
    assert!(
        matches!(err, AppError::Di(DiError::Missing { .. })),
        "{err}"
    );
}

#[test]
fn registering_controller_twice_is_route_error() {
    let c = container();
    let err = App::new()
        .controller::<GreetController>(&c)
        .unwrap()
        .controller::<GreetController>(&c)
        .err()
        .unwrap();
    assert!(
        matches!(err, AppError::Route(RouteError::Duplicate(..))),
        "{err}"
    );
}

#[test]
fn app_error_source_returns_the_di_error_and_route_error() {
    use std::error::Error;

    let empty = ContainerBuilder::new().build().unwrap();
    let di = App::new()
        .controller::<GreetController>(&empty)
        .err()
        .unwrap();
    let source = di.source().expect("Di has a source");
    let inner = source.downcast_ref::<DiError>().expect("source is DiError");
    assert!(matches!(inner, DiError::Missing { .. }), "{inner}");

    let c = container();
    let route = App::new()
        .controller::<GreetController>(&c)
        .unwrap()
        .controller::<GreetController>(&c)
        .err()
        .unwrap();
    let source = route.source().expect("Route has a source");
    let inner = source
        .downcast_ref::<RouteError>()
        .expect("source is RouteError");
    assert!(matches!(inner, RouteError::Duplicate(..)), "{inner}");
}
