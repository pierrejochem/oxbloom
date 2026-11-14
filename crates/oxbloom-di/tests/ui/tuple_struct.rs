#![allow(unused_imports, dead_code)]
use oxbloom_di::component;
use std::sync::Arc;
struct X;
#[component]
struct T(Arc<X>);
fn main() {}
