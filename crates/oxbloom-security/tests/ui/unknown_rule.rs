use oxbloom_security::{AccessDenied, method_security};

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(admin)]
    fn close(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(all(role = "ADMIN", superuser = "yes"))]
    fn wipe(&self) -> Result<(), AccessDenied> {
        Ok(())
    }
}

fn main() {}
