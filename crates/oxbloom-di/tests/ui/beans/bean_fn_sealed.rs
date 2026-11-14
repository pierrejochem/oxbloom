use oxbloom_di::{BeanFn, DiError, Resolver, TryBeanFn};

struct Maker;
struct Made;

impl BeanFn<(), Made> for Maker {
    fn call(&self, _: &Resolver<'_>) -> Result<Made, DiError> {
        Ok(Made)
    }
}

impl TryBeanFn<(), Made> for Maker {
    fn call(&self, _: &Resolver<'_>) -> Result<Made, DiError> {
        Ok(Made)
    }
}

fn main() {}
