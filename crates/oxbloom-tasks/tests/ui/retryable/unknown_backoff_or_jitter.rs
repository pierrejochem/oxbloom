use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable(backoff = "linear")]
    async fn fetch(&self) -> Result<u32, String> {
        Ok(1)
    }

    #[retryable(jitter = "random")]
    async fn send(&self) -> Result<u32, String> {
        Ok(1)
    }
}

fn main() {}
