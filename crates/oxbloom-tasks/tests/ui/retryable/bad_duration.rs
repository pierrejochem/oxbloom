use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable(initial = "1.5s")]
    async fn fetch(&self) -> Result<u32, String> {
        Ok(1)
    }

    #[retryable(max_delay = "soon")]
    async fn send(&self) -> Result<u32, String> {
        Ok(1)
    }

    #[retryable(initial = "99999999999999999999d")]
    async fn store(&self) -> Result<u32, String> {
        Ok(1)
    }
}

fn main() {}
