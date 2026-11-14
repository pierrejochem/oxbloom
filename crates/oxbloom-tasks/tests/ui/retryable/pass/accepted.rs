//! Every method shape `#[retryable]` accepts.

use std::io;

use oxbloom_tasks::retryable;

#[derive(Debug)]
struct Failure;

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("failure")
    }
}

impl From<std::num::ParseIntError> for Failure {
    fn from(_: std::num::ParseIntError) -> Self {
        Failure
    }
}

fn always(_: &Failure) -> bool {
    true
}

mod policies {
    pub fn never(_: &std::io::Error) -> bool {
        false
    }
}

struct Client {
    base: String,
}

impl Client {
    #[retryable]
    async fn plain(&self) -> Result<(), Failure> {
        Ok(())
    }

    #[retryable(
        max_attempts = 5,
        backoff = "exponential",
        initial = "200ms",
        multiplier = 1.5,
        max_delay = "10s",
        jitter = "full",
        retry_if = "always",
    )]
    async fn every_key(&self, id: u64) -> Result<String, Failure> {
        Ok(format!("{}/{id}", self.base))
    }

    #[retryable(backoff = "fixed", initial = "1s", multiplier = 3, jitter = "equal")]
    async fn integer_multiplier(&self) -> Result<u32, Failure> {
        let n = "7".parse::<u32>()?;
        Ok(n)
    }

    #[retryable(max_attempts = 1, jitter = "none", retry_if = "policies::never")]
    async fn io_alias(&self, path: &str) -> io::Result<usize> {
        Ok(path.len())
    }

    #[retryable(initial = "0ms", max_delay = "1d")]
    async fn associated(value: u32) -> std::result::Result<u32, Failure> {
        Ok(value)
    }

    #[retryable(retry_if = "Self::is_retryable")]
    pub(crate) async fn owned_argument(&self, name: String) -> Result<String, Failure> {
        let name = name.clone();
        if name.is_empty() {
            return Err(Failure);
        }
        Ok(name)
    }

    fn is_retryable(_: &Failure) -> bool {
        true
    }

    #[retryable]
    async fn explicit_receiver(self: &Self) -> Result<&str, Failure> {
        Ok(&self.base)
    }

    #[retryable(max_attempts = 1)]
    async fn coerced_return(&self, short: bool) -> Result<&str, Box<dyn std::error::Error>> {
        if short {
            return Ok(&self.base);
        }
        if self.base.is_empty() {
            return Err("empty base".into());
        }
        Ok("long")
    }

    #[retryable(max_attempts = 1)]
    async fn diverging(&self) -> Result<u32, Failure> {
        return Err(Failure);
    }
}

fn main() {
    let client = Client { base: "api".to_owned() };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        client.plain().await.unwrap();
        assert_eq!(client.every_key(3).await.unwrap(), "api/3");
        assert_eq!(client.integer_multiplier().await.unwrap(), 7);
        assert_eq!(client.io_alias("abc").await.unwrap(), 3);
        assert_eq!(Client::associated(4).await.unwrap(), 4);
        assert_eq!(client.owned_argument("x".to_owned()).await.unwrap(), "x");
        assert_eq!(client.explicit_receiver().await.unwrap(), "api");
        assert_eq!(client.coerced_return(true).await.unwrap(), "api");
        assert_eq!(client.coerced_return(false).await.unwrap(), "long");
        assert!(client.diverging().await.is_err());
    });
}
