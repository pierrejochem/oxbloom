#![allow(unused_imports, dead_code)]
use oxbloom_boot::auto_configuration;
struct Auto<T>(T);
#[auto_configuration]
impl<T> Auto<T> {}
fn main() {}
