use oxbloom_security::{AccessDenied, Authentication, method_security};

fn same(_auth: Option<&Authentication>, _value: &String, _name: &String) -> bool {
    true
}

struct Accounts;

#[method_security]
impl Accounts {
    // The body moves `name`, so the post check cannot read it any more.
    #[post_authorize(custom = same(returned, name))]
    fn rename(&self, name: String) -> Result<String, AccessDenied> {
        Ok(name)
    }
}

fn main() {}
