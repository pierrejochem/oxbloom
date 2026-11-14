#![allow(unused_imports, dead_code)]
use oxbloom_di::{bean, configuration};
struct Pool;
struct Cfg;
#[configuration]
impl Cfg {
    #[bean(on_property(""))]
    fn pool(&self) -> Pool { Pool }
}
fn main() {}
