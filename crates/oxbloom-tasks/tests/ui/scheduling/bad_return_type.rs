use oxbloom_tasks::scheduling;

struct Reports;

#[scheduling]
impl Reports {
    #[scheduled(fixed_rate = "30s")]
    fn refresh(&self) -> u32 {
        7
    }
}

fn main() {}
