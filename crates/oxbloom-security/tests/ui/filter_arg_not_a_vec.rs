use std::collections::HashSet;

use oxbloom_security::{AccessDenied, Authentication, method_security};

fn keep(_auth: Option<&Authentication>, _id: &u64) -> bool {
    true
}

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_filter(arg = ids, custom = keep)]
    fn by_set(&self, ids: HashSet<u64>) -> Result<usize, AccessDenied> {
        Ok(ids.len())
    }

    #[pre_filter(arg = ids, custom = keep)]
    fn by_reference(&self, ids: &mut Vec<u64>) -> Result<usize, AccessDenied> {
        Ok(ids.len())
    }
}

fn main() {}
