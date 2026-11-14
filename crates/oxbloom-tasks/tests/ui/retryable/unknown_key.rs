use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable(attempts = 3)]
    async fn fetch(&self) -> Result<u32, String> {
        Ok(1)
    }
}

fn main() {}
