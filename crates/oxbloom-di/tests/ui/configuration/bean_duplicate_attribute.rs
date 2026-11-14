#![allow(unused_imports, dead_code)]
use std::sync::Arc;
use oxbloom_di::{DiError, bean, configuration};
struct Pool;
struct Cfg;
#[configuration]
impl Cfg {
    #[bean]
    #[bean(lazy)]
    fn pool(&self) -> Pool { Pool }
}
fn main() {}
