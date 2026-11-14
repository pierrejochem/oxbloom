use oxbloom_web::{controller, IntoResponse, Request};
struct C;
#[controller("/c")]
impl C {
    #[get("/a")]
    async fn a(&self, _r: Request) -> impl IntoResponse { "x" }
    #[get("/a")]
    async fn b(&self, _r: Request) -> impl IntoResponse { "y" }
}
fn main() {}
