use oxbloom_tasks::scheduling;

struct Reports;

#[scheduling]
impl Reports {
    #[scheduled(fixed_rate = "30s")]
    async fn refresh(&self, limit: u32) {
        let _ = limit;
    }
}

fn main() {}
