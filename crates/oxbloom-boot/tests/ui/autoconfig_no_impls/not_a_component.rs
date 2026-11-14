#![allow(unused_imports, dead_code)]
// An auto-configuration must be a `Component` (e.g. `#[component]`): the application registers
// its instance inside the group.
use oxbloom_boot::auto_configuration;
struct Pool;
struct Auto;
#[auto_configuration]
impl Auto {
    #[bean]
    fn pool(&self) -> Pool { Pool }
}
fn main() {}
