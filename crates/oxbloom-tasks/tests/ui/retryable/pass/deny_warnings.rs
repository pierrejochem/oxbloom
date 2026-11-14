//! The generated code raises no warning, for a one-line body and for a longer one.

#![deny(warnings)]

use oxbloom_tasks::retryable;

struct Client;

impl Client {
    #[retryable]
    async fn one_line(&self) -> Result<u32, String> { Ok(1) }

    #[retryable(max_attempts = 2)]
    async fn several(&self, text: &str) -> Result<u32, std::num::ParseIntError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(0);
        }
        let number = trimmed.parse::<u32>()?;
        Ok(number)
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert_eq!(Client.one_line().await, Ok(1));
        assert_eq!(Client.several(" 7 ").await, Ok(7));
        assert_eq!(Client.several("").await, Ok(0));
    });
}
