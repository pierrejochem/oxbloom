use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable(max_attempts = 3, max_attempts = 4)]
    async fn fetch(&self) -> Result<u32, String> {
        Ok(1)
    }
}

fn main() {}
