#![allow(unused_imports, dead_code)]
use std::sync::Arc;
use oxbloom_di::{DiError, bean, configuration};
struct Pool;
struct Cfg;
impl Cfg {
    #[bean]
    fn pool(&self) -> Pool { Pool }
}
fn main() {}
