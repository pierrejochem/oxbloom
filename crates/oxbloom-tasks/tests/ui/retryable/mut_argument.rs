use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable]
    async fn fetch(&self, mut id: u64) -> Result<u64, String> {
        id += 1;
        Ok(id)
    }
}

fn main() {}
