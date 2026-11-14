use oxbloom_di::ContainerBuilder;

struct Port(u16);

fn main() {
    let _ = ContainerBuilder::new().try_bean(|| Port(80));
}
