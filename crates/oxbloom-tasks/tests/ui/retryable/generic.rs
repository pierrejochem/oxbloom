use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable]
    async fn fetch<T: Default>(&self) -> Result<T, String> {
        Ok(T::default())
    }
}

fn main() {}
