// `exclude` takes an auto-configuration only, so a typo in the type is a compile error.
use oxbloom_boot::OxbloomApplication;

struct Metrics;

fn main() {
    let _ = OxbloomApplication::builder().exclude::<Metrics>();
}
