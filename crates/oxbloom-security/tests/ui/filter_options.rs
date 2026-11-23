use oxbloom_security::{AccessDenied, Authentication, method_security};

fn keep(_auth: Option<&Authentication>, _id: &u64) -> bool {
    true
}

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_filter(custom = keep)]
    fn no_arg(&self, ids: Vec<u64>) -> Result<usize, AccessDenied> {
        Ok(ids.len())
    }

    #[pre_filter(arg = ids, custom = keep, custom = keep)]
    fn repeated_option(&self, ids: Vec<u64>) -> Result<usize, AccessDenied> {
        Ok(ids.len())
    }

    #[post_filter(arg = ids, custom = keep)]
    fn arg_on_post_filter(&self, ids: Vec<u64>) -> Result<Vec<u64>, AccessDenied> {
        Ok(ids)
    }

    #[post_filter(custom = keep(ids))]
    fn custom_call(&self, ids: Vec<u64>) -> Result<Vec<u64>, AccessDenied> {
        Ok(ids)
    }

    #[post_filter(keep = keep)]
    fn unknown_option(&self, ids: Vec<u64>) -> Result<Vec<u64>, AccessDenied> {
        Ok(ids)
    }

    #[post_filter(custom = keep)]
    #[post_filter(custom = keep)]
    fn repeated_attribute(&self, ids: Vec<u64>) -> Result<Vec<u64>, AccessDenied> {
        Ok(ids)
    }
}

fn main() {}
