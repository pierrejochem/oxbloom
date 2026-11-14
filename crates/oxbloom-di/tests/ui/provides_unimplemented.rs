#![allow(unused_imports, dead_code)]
use oxbloom_di::component;
use std::sync::Arc;
struct X;
trait Tr: Send + Sync {}
#[component(provides(dyn Tr))]
struct T;
fn main() {}
