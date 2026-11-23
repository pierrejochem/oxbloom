use oxbloom_security::{AccessDenied, method_security};

trait Close {
    fn close(&self) -> Result<(), AccessDenied>;
}

struct Accounts;

#[method_security]
impl Close for Accounts {
    fn close(&self) -> Result<(), AccessDenied> {
        Ok(())
    }
}

fn main() {}
