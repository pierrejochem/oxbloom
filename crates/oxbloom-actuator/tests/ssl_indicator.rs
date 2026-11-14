use std::sync::Arc;
use std::time::{Duration, SystemTime};

use oxbloom_actuator::{HealthIndicator, SslHealthIndicator, Status};
use oxbloom_config::Config;
use oxbloom_core::Clock;
use oxbloom_ssl::SslBundles;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const SUBJECT: &str = "Secret Subject CN";

struct FixedClock(SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

fn secs(t: SystemTime) -> Duration {
    Duration::from_secs(t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs())
}

/// A bundle named `web` whose only certificate is valid from `from` to `to`.
fn indicator(from: SystemTime, to: SystemTime, now: SystemTime) -> SslHealthIndicator {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["web.test".to_string()]).unwrap();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, SUBJECT);
    params.distinguished_name = name;
    params.not_before = rcgen::date_time_ymd(1970, 1, 1) + secs(from);
    params.not_after = rcgen::date_time_ymd(1970, 1, 1) + secs(to);
    let pem = params.self_signed(&key).unwrap().pem();
    let config = Config::from_str(&format!("[ssl.bundle.pem.web]\ntrust = '''\n{pem}'''")).unwrap();
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(now));
    let bundles = SslBundles::from_config(&config)
        .unwrap()
        .with_clock(Arc::clone(&clock));
    SslHealthIndicator::new(Arc::new(bundles), clock)
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

#[tokio::test]
async fn name_is_ssl() {
    let i = indicator(now() - 10 * DAY, now() + 100 * DAY, now());
    assert_eq!(i.name(), "ssl");
}

#[tokio::test]
async fn expired_is_down() {
    let i = indicator(now() - 100 * DAY, now() - DAY, now());
    let health = i.health().await;
    assert_eq!(health.status, Status::Down);
    assert_eq!(health.details["expired"], serde_json::json!(["web"]));
}

#[tokio::test]
async fn expiring_soon_is_up_with_warning() {
    let i = indicator(now() - 100 * DAY, now() + DAY, now());
    let health = i.health().await;
    assert_eq!(health.status, Status::Up);
    assert_eq!(health.details["warning"], serde_json::json!(["web"]));
}

#[tokio::test]
async fn valid_is_up() {
    let i = indicator(now() - 10 * DAY, now() + 100 * DAY, now());
    let health = i.health().await;
    assert_eq!(health.status, Status::Up);
    assert!(!health.details.contains_key("warning"));
    assert!(!health.details.contains_key("expired"));
    assert_eq!(health.details["bundles"], serde_json::json!(["web"]));
}

#[tokio::test]
async fn no_bundles_is_up() {
    let bundles = SslBundles::from_config(&Config::from_str("").unwrap()).unwrap();
    let i = SslHealthIndicator::new(Arc::new(bundles), Arc::new(FixedClock(now())));
    assert_eq!(i.health().await.status, Status::Up);
}

#[tokio::test]
async fn details_do_not_contain_the_subject() {
    for (from, to) in [
        (now() - 100 * DAY, now() - DAY),
        (now() - 100 * DAY, now() + DAY),
        (now() - 10 * DAY, now() + 100 * DAY),
    ] {
        let health = indicator(from, to, now()).health().await;
        let text = serde_json::to_string(&health.details).unwrap();
        assert!(!text.contains(SUBJECT), "{text}");
        assert!(!text.contains("web.test"), "{text}");
    }
}
