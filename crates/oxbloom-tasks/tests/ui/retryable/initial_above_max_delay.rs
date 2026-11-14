use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable(initial = "2m", max_delay = "30s")]
    async fn fetch(&self) -> Result<u32, String> {
        Ok(1)
    }
}

fn main() {}
