use oxbloom_security::method_security;

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(authenticated)]
    fn count(&self) -> u32 {
        1
    }
}

fn main() {}
