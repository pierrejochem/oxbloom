#![allow(unused_imports, dead_code)]
use std::sync::Arc;
use oxbloom_di::{DiError, bean, configuration};
struct Pool;
struct Cfg;
trait Store {}
#[configuration]
impl Cfg {
    #[bean]
    fn pool(&self, _s: impl Store) -> Pool { Pool }
}
fn main() {}
