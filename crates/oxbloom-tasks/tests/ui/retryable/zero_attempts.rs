use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable(max_attempts = 0)]
    async fn fetch(&self) -> Result<u32, String> {
        Ok(1)
    }

    #[retryable(max_attempts = "3")]
    async fn send(&self) -> Result<u32, String> {
        Ok(1)
    }
}

fn main() {}
