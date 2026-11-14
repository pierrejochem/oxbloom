use oxbloom_di::ContainerBuilder;

struct Greeting(String);

fn main() {
    let _ = ContainerBuilder::new().bean(|name: String| Greeting(name));
}
