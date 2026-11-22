//! A `dyn Clock` reaches the JWT auto-configuration as a bean, and the logging subscriber
//! through `logging_clock`.
//!
//! Only `logging_clock_is_accepted_by_a_run` installs the global subscriber in this binary;
//! every other run turns logging off.

#![cfg(any(feature = "security", feature = "logging"))]

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxbloom_boot::{Banner, OxbloomApplication};
use oxbloom_config::Config;
use oxbloom_core::Clock;

/// A clock that always answers the same instant.
struct Fixed(SystemTime);

impl Clock for Fixed {
    fn now(&self) -> SystemTime {
        self.0
    }
}

#[cfg(feature = "security")]
mod security {
    use super::*;

    fn real_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    use jsonwebtoken::{EncodingKey, Header, encode};
    use oxbloom_boot::{JwtDecoderAutoConfiguration, OxbloomApplicationBuilder, RunListener};
    use oxbloom_di::Container;
    use oxbloom_security::{Algorithm, AuthenticationError, JwtDecoder};

    const SECRET: &str = "an-example-secret-of-32-bytes!!!";

    /// The `dyn JwtDecoder` the container held when it was ready.
    #[derive(Clone, Default)]
    struct Decoder(Arc<Mutex<Option<Arc<dyn JwtDecoder>>>>);

    impl RunListener for Decoder {
        fn context_ready(&self, c: &Container) {
            *self.0.lock().unwrap() = Some(c.get::<dyn JwtDecoder>().unwrap());
        }
    }

    fn app(seen: Decoder) -> OxbloomApplicationBuilder {
        let toml = format!(
            "oxbloom.logging.enabled = false\n\
             [security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\nclock-skew-secs = 0\n"
        );
        OxbloomApplication::builder()
            .with_config(Config::from_str(&toml).unwrap())
            .banner(Banner::off())
            .output(std::io::sink())
            .listener(seen)
            .auto_configuration::<JwtDecoderAutoConfiguration>()
    }

    fn token(exp: u64) -> String {
        let claims = serde_json::json!({ "sub": "alice", "exp": exp });
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_jwt_auto_configuration_uses_a_clock_bean() {
        let seen = Decoder::default();
        let later = Fixed(UNIX_EPOCH + Duration::from_secs(real_now() + 7200));
        app(seen.clone())
            .configure(move |b| {
                b.instance(later)
                    .provides(|c: Arc<Fixed>| -> Arc<dyn Clock> { c })
            })
            .run()
            .await
            .unwrap();
        let decoder = seen.0.lock().unwrap().clone().unwrap();
        // Valid for one more hour of real time, but the clock bean is two hours ahead.
        assert_eq!(
            decoder.decode(&token(real_now() + 3600)).await.unwrap_err(),
            AuthenticationError::Expired
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn without_a_clock_bean_the_decoder_uses_the_system_time() {
        let seen = Decoder::default();
        app(seen.clone()).run().await.unwrap();
        let decoder = seen.0.lock().unwrap().clone().unwrap();
        assert!(decoder.decode(&token(real_now() + 3600)).await.is_ok());
        assert_eq!(
            decoder.decode(&token(real_now() - 3600)).await.unwrap_err(),
            AuthenticationError::Expired
        );
    }
}

#[cfg(feature = "logging")]
mod logging {
    use std::io::Write;

    use super::*;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn logging_clock_is_accepted_by_a_run() {
        let logs = Buf::default();
        let writer = logs.clone();
        let status = OxbloomApplication::builder()
            .with_config(
                Config::from_str("[logging.console]\nformat = \"json\"\ncolor = \"never\"")
                    .unwrap(),
            )
            .banner(Banner::off())
            .output(std::io::sink())
            .logging_writer(move || writer.clone())
            .logging_clock(Arc::new(Fixed(UNIX_EPOCH + Duration::from_secs(86_400))))
            .run()
            .await
            .unwrap();
        assert_eq!(status.code(), 0);

        // This is the only run in this binary that installs the global subscriber, so the JSON
        // lines it wrote carry the clock's time.
        tracing::info!(target: "app", "after the run");
        let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        let line = text
            .lines()
            .find(|l| l.contains("after the run"))
            .unwrap_or_else(|| panic!("{text}"));
        let line: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(line["timestamp"], "1970-01-02T00:00:00.000Z");
    }
}
