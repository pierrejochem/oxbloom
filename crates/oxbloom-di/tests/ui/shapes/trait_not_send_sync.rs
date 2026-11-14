use oxbloom_di::ContainerBuilder;

trait Plain {}

fn main() {
    let c = ContainerBuilder::new().build().unwrap();
    let _ = c.get::<dyn Plain>();
}
