#![allow(unused_imports, dead_code)]
use oxbloom_di::component;
use std::sync::Arc;
struct X;
use std::rc::Rc;
#[component]
struct T { r: Rc<X> }
fn main() {}
