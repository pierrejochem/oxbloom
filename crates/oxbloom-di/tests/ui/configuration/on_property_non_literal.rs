#![allow(unused_imports, dead_code)]
use oxbloom_di::{bean, configuration};
const KEY: &str = "cache.enabled";
struct Cache;
struct Cfg;
#[configuration]
impl Cfg {
    #[bean(on_property(KEY))]
    fn cache(&self) -> Cache { Cache }
}
fn main() {}
