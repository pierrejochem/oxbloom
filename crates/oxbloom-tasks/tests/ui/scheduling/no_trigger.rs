use oxbloom_tasks::scheduling;

struct Reports;

#[scheduling]
impl Reports {
    #[scheduled(initial_delay = "5s")]
    async fn refresh(&self) {}
}

fn main() {}
