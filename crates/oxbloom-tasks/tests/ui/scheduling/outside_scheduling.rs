use oxbloom_tasks::scheduled;

struct Reports;

impl Reports {
    #[scheduled(fixed_rate = "30s")]
    async fn refresh(&self) {}
}

fn main() {}
