use oxbloom_tasks::scheduling;

struct Reports;

trait Refresh {
    fn refresh(&self);
}

#[scheduling]
impl Refresh for Reports {
    #[scheduled(fixed_rate = "30s")]
    fn refresh(&self) {}
}

fn main() {}
