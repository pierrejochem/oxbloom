use oxbloom_tasks::retryable;

struct Client;

fn is_transient(_: &String) -> bool {
    true
}

impl Client {
    #[retryable(retry_if = is_transient)]
    async fn fetch(&self) -> Result<u32, String> {
        Ok(1)
    }

    #[retryable(retry_if = "is transient")]
    async fn send(&self) -> Result<u32, String> {
        Ok(1)
    }
}

fn main() {}
