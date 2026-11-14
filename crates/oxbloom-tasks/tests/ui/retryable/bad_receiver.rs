use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable]
    async fn reset(&mut self) -> Result<u32, String> {
        Ok(1)
    }

    #[retryable]
    async fn consume(self) -> Result<u32, String> {
        Ok(1)
    }
}

fn main() {}
