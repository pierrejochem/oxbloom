#![allow(unused_imports, dead_code)]
use std::sync::Arc;
use oxbloom_di::{DiError, bean, configuration};
struct Pool;
struct Cfg;
trait Store: Send + Sync {}
#[configuration]
impl Cfg {
    #[bean(provides(dyn Store))]
    fn pool(&self) -> Pool { Pool }
}
fn main() {}
