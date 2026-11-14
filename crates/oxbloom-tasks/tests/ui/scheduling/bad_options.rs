use oxbloom_tasks::scheduling;

struct Reports;

#[scheduling]
impl Reports {
    #[scheduled(fixed_rate = "30s", every = "1s")]
    async fn refresh(&self) {}
}

fn main() {}
