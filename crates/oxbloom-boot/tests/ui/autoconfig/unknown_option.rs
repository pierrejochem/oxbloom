#![allow(unused_imports, dead_code)]
use oxbloom_boot::auto_configuration;
struct Auto;
#[auto_configuration(on_class(Auto))]
impl Auto {}
fn main() {}
