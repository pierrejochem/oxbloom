use oxbloom_security::{AccessDenied, method_security};

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(role = "ADMIN")]
    #[pre_authorize(authenticated)]
    fn close(&self) -> Result<(), AccessDenied> {
        Ok(())
    }
}

fn main() {}
