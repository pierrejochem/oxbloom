#![allow(unused_imports, dead_code)]
use oxbloom_boot::auto_configuration;
struct Auto;
trait Named {}
#[auto_configuration]
impl Named for Auto {}
fn main() {}
