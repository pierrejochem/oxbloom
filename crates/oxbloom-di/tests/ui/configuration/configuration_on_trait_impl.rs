#![allow(unused_imports, dead_code)]
use std::sync::Arc;
use oxbloom_di::{DiError, bean, configuration};
struct Pool;
struct Cfg;
trait Make { fn pool(&self) -> Pool; }
#[configuration]
impl Make for Cfg {
    fn pool(&self) -> Pool { Pool }
}
fn main() {}
