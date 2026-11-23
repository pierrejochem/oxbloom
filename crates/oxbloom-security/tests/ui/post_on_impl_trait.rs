use oxbloom_security::{AccessDenied, method_security};

struct Accounts;

#[method_security]
impl Accounts {
    #[post_authorize(authenticated)]
    fn ids(&self) -> impl Iterator<Item = u64> {
        [1, 2].into_iter()
    }

    #[post_filter(custom = keep)]
    fn result(&self) -> impl Into<Result<Vec<u64>, AccessDenied>> {
        Ok(vec![1])
    }
}

fn main() {}
