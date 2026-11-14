#![allow(unused_imports, dead_code)]
use oxbloom_di::{bean, configuration};
struct Pool;
struct Cfg;
#[configuration]
impl Cfg {
    #[bean(on_class(Pool))]
    fn pool(&self) -> Pool { Pool }
}
fn main() {}
