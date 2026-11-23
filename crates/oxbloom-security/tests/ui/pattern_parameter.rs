use oxbloom_security::{AccessDenied, Authentication, method_security};

fn owns(_auth: Option<&Authentication>, _id: &u64) -> bool {
    true
}

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(custom = owns(id))]
    fn close(&self, (id, _name): (u64, String)) -> Result<u64, AccessDenied> {
        Ok(id)
    }
}

fn main() {}
