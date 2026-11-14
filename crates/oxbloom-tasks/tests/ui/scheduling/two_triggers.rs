use oxbloom_tasks::scheduling;

struct Reports;

#[scheduling]
impl Reports {
    #[scheduled(cron = "0 0 * * * *", fixed_rate = "30s")]
    async fn refresh(&self) {}
}

fn main() {}
