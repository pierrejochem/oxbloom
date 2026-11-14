use oxbloom_tasks::scheduling;

struct Reports;

#[scheduling]
impl Reports {
    #[scheduled(fixed_rate = "30s", zone = "Europe/Amsterdam")]
    async fn refresh(&self) {}
}

fn main() {}
