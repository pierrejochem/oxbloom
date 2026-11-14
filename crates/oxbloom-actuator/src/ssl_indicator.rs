//! The health indicator of the SSL bundles.

use std::sync::Arc;

use async_trait::async_trait;
use oxbloom_core::Clock;
use oxbloom_ssl::SslBundles;
use serde_json::json;

use crate::{Health, HealthIndicator, Status};

/// Reports the validity of the certificates of an [`SslBundles`] as the `ssl` component.
///
/// The status is `Down` when a certificate of any bundle has expired, and `Up` otherwise. The
/// details list `bundles` (every bundle name), `expired` (the bundles with an expired
/// certificate, when any) and `warning` (the bundles with a certificate that is not valid yet
/// or expires within [`SslBundles::expiry_warning`], the same window as the log warnings of
/// `oxbloom-ssl`, when any). They name bundles only: never a certificate subject or any other
/// certificate content.
///
/// Available with the cargo feature `ssl`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_actuator::{HealthIndicator, SslHealthIndicator, Status};
/// use oxbloom_config::Config;
/// use oxbloom_core::SystemClock;
/// use oxbloom_ssl::SslBundles;
///
/// # tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
/// let bundles = SslBundles::from_config(&Config::from_str("").unwrap()).unwrap();
/// let indicator = SslHealthIndicator::new(Arc::new(bundles), Arc::new(SystemClock));
/// assert_eq!(indicator.name(), "ssl");
/// assert_eq!(indicator.health().await.status, Status::Up);
/// # });
/// ```
pub struct SslHealthIndicator {
    bundles: Arc<SslBundles>,
    clock: Arc<dyn Clock>,
}

impl SslHealthIndicator {
    /// Creates the indicator over `bundles`, reading the time from `clock`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_actuator::SslHealthIndicator;
    /// use oxbloom_config::Config;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// let bundles = SslBundles::from_config(&Config::from_str("").unwrap()).unwrap();
    /// let _indicator = SslHealthIndicator::new(Arc::new(bundles), Arc::new(SystemClock));
    /// ```
    pub fn new(bundles: Arc<SslBundles>, clock: Arc<dyn Clock>) -> Self {
        Self { bundles, clock }
    }
}

#[async_trait]
impl HealthIndicator for SslHealthIndicator {
    fn name(&self) -> &str {
        "ssl"
    }

    async fn health(&self) -> Health {
        let now = self.clock.now();
        let window = self.bundles.expiry_warning();
        let mut expired = Vec::new();
        let mut warning = Vec::new();
        for name in self.bundles.names() {
            let Ok(bundle) = self.bundles.get(&name) else {
                continue;
            };
            let validity = bundle.validity();
            if validity.iter().any(|v| now > v.not_after) {
                expired.push(name);
            } else if validity.iter().any(|v| {
                now < v.not_before || v.not_after.duration_since(now).is_ok_and(|l| l <= window)
            }) {
                warning.push(name);
            }
        }
        let mut health = Health::new(if expired.is_empty() {
            Status::Up
        } else {
            Status::Down
        });
        health
            .details
            .insert("bundles".into(), json!(self.bundles.names()));
        if !expired.is_empty() {
            health.details.insert("expired".into(), json!(expired));
        }
        if !warning.is_empty() {
            health.details.insert("warning".into(), json!(warning));
        }
        health
    }
}
