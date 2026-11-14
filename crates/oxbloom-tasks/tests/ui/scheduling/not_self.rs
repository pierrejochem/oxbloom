use oxbloom_tasks::scheduling;

struct Reports;

#[scheduling]
impl Reports {
    #[scheduled(fixed_rate = "30s")]
    async fn refresh() {}

    #[scheduled(fixed_rate = "30s")]
    async fn consume(self) {}
}

fn main() {}
