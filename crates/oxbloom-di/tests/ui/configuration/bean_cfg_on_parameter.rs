#![allow(unused_imports, dead_code)]
use std::sync::Arc;
use oxbloom_di::{DiError, bean, configuration};
struct Pool;
struct Cfg;
#[configuration]
impl Cfg {
    #[bean]
    fn pool(&self, #[cfg(test)] _n: Arc<u8>) -> Pool { Pool }
}
fn main() {}
