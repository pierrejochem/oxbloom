use oxbloom_web::{controller, IntoResponse, Request};
struct C;
#[controller("/c")]
impl C {
    #[get("no-slash")]
    async fn a(&self, _r: Request) -> impl IntoResponse { "x" }
}
fn main() {}
