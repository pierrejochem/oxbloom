use oxbloom_di::{DiError, Inject, Resolver};

struct Name(String);

impl Inject for Name {
    fn inject(_: &Resolver<'_>) -> Result<Self, DiError> {
        Ok(Name(String::new()))
    }
}

fn main() {}
