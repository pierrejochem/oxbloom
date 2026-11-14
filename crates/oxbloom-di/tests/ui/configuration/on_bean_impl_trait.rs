#![allow(unused_imports, dead_code)]
use oxbloom_di::{bean, configuration};
trait Store {}
struct Cache;
struct Cfg;
#[configuration]
impl Cfg {
    #[bean(on_bean(impl Store))]
    fn cache(&self) -> Cache { Cache }
}
fn main() {}
