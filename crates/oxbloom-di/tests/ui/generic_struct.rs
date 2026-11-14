#![allow(unused_imports, dead_code)]
use oxbloom_di::component;
use std::sync::Arc;
struct X;
#[component]
struct T<G> { x: Arc<G> }
fn main() {}
