#![allow(unused_imports, dead_code)]
use std::sync::Arc;
use oxbloom_di::{DiError, bean, configuration};
struct Pool;
struct Cfg;
#[bean(lazy)]
fn pool() -> Pool { Pool }
fn main() {}
