#![allow(unused_imports, dead_code)]
// Condition options belong to `#[bean]`; `#[component]` does not accept them.
use oxbloom_di::component;
struct Pool;
#[component(on_bean(Pool))]
struct Cache;
fn main() {}
