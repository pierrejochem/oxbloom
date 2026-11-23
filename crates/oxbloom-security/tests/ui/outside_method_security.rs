use oxbloom_security::{AccessDenied, post_authorize, post_filter, pre_authorize, pre_filter};

struct Accounts;

impl Accounts {
    #[pre_authorize(role = "ADMIN")]
    fn close(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[post_authorize(authenticated)]
    fn open(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_filter(arg = items, custom = keep)]
    fn save(&self, items: Vec<u32>) -> Result<(), AccessDenied> {
        let _ = items;
        Ok(())
    }

    #[post_filter(custom = keep)]
    fn list(&self) -> Result<Vec<u32>, AccessDenied> {
        Ok(Vec::new())
    }
}

fn main() {}
