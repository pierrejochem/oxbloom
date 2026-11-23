use oxbloom_security::{AccessDenied, Authentication, method_security};

fn owns(_auth: Option<&Authentication>, _id: &u64) -> bool {
    true
}

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(custom = owns(account))]
    fn close(&self, id: u64) -> Result<u64, AccessDenied> {
        Ok(id)
    }
}

fn main() {}
