use oxbloom_security::method_security;

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(role = "ADMIN")]
    fn close(&self) {}

    #[pre_authorize(role = "ADMIN")]
    fn count(&self) -> (u32, u32) {
        (1, 2)
    }
}

fn main() {}
