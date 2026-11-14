#![allow(unused_imports, dead_code)]
use oxbloom_di::component;
use std::sync::Arc;
struct X;
#[component(primary, order = 1, primary)]
struct T { a: Arc<X> }
fn main() {}
