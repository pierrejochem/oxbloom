use oxbloom_security::{AccessDenied, Authentication, method_security};

fn keep(_auth: Option<&Authentication>, _id: &u64) -> bool {
    true
}

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_filter(arg = accounts, custom = keep)]
    fn close_all(&self, ids: Vec<u64>) -> Result<usize, AccessDenied> {
        Ok(ids.len())
    }
}

fn main() {}
