use oxbloom_security::{AccessDenied, Authentication, method_security};

fn mine(_auth: Option<&Authentication>, _value: &u64) -> bool {
    true
}

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(custom = mine(returned))]
    fn balance(&self) -> Result<u64, AccessDenied> {
        Ok(1)
    }
}

fn main() {}
