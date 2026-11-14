#![allow(unused_imports, dead_code)]
use oxbloom_di::{bean, configuration};
struct Pool;
struct Cache;
struct Cfg;
#[configuration]
impl Cfg {
    #[bean(on_bean(Pool, Cache))]
    fn cache(&self) -> Cache { Cache }
}
fn main() {}
