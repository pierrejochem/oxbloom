use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable(multiplier = 0.5)]
    async fn fetch(&self) -> Result<u32, String> {
        Ok(1)
    }

    #[retryable(multiplier = 1e999)]
    async fn send(&self) -> Result<u32, String> {
        Ok(1)
    }

    #[retryable(multiplier = "2")]
    async fn store(&self) -> Result<u32, String> {
        Ok(1)
    }
}

fn main() {}
