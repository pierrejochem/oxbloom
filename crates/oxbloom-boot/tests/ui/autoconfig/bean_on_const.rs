#![allow(unused_imports, dead_code)]
use oxbloom_boot::auto_configuration;
use oxbloom_di::{bean, component};
#[component]
struct Auto;
#[auto_configuration]
impl Auto {
    #[bean]
    const POOL: u8 = 1;
}
fn main() {}
