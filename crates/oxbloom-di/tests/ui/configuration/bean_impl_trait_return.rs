#![allow(unused_imports, dead_code)]
use std::sync::Arc;
use oxbloom_di::{DiError, bean, configuration};
struct Pool;
struct Cfg;
trait Store {}
impl Store for Pool {}
#[configuration]
impl Cfg {
    #[bean]
    fn pool(&self) -> impl Store { Pool }
}
fn main() {}
