use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable]
    async fn count(&self) -> u32 {
        1
    }

    #[retryable]
    async fn ping(&self) {}

    #[retryable]
    async fn pair(&self) -> (u32, String) {
        (1, String::new())
    }
}

fn main() {}
