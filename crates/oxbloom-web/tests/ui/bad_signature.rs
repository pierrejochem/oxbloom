use oxbloom_web::{controller, IntoResponse, Request};
struct C;
#[controller("/c")]
impl C {
    #[get("/a")]
    fn a(&self, _r: Request) -> &'static str { "x" }
}
fn main() {}
