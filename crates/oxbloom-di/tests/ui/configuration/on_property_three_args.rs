#![allow(unused_imports, dead_code)]
use oxbloom_di::{bean, configuration};
struct Pool;
struct Cfg;
#[configuration]
impl Cfg {
    #[bean(on_property("pool.mode", "fast", "extra"))]
    fn pool(&self) -> Pool { Pool }
}
fn main() {}
