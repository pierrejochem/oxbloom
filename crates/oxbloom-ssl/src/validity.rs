//! The validity dates of a bundle's certificates and the warnings about expired, not yet valid
//! and soon expiring ones.

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use rustls::pki_types::CertificateDer;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::X509Certificate;

use crate::bundle::Material;
use crate::reload::lock;

/// The log target of every validity warning.
const TARGET: &str = "oxbloom::ssl";

/// How long a warning about one certificate stays quiet while its condition holds.
const REPEAT: Duration = Duration::from_secs(24 * 60 * 60);

/// The subject of a certificate that has neither a common name nor a DNS name.
const NO_SUBJECT: &str = "(no subject)";

/// The subject and validity dates of one certificate of a bundle, from
/// [`SslBundle::validity`](crate::SslBundle::validity).
///
/// The dates have whole seconds, as the certificate stores them.
///
/// # Examples
///
/// ```
/// use std::time::SystemTime;
/// use oxbloom_config::Config;
/// use oxbloom_ssl::{CertValidity, SslBundles};
///
/// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
/// # let ca_pem = generated.cert.pem();
/// let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"))?;
/// let validity: Vec<CertValidity> = SslBundles::from_config(&config)?.get("client")?.validity();
/// let now = SystemTime::now();
/// assert!(validity[0].not_before <= now && now <= validity[0].not_after);
/// assert!(!validity[0].subject.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertValidity {
    /// The common name of the certificate's subject, else its first DNS name, else
    /// `(no subject)`.
    pub subject: String,
    /// The first moment the certificate is valid.
    pub not_before: SystemTime,
    /// The last moment the certificate is valid.
    pub not_after: SystemTime,
}

/// What is wrong with a certificate at a given time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Condition {
    NotYetValid,
    Expired,
    ExpiresSoon,
}

/// The last warning about one certificate: the condition it named and the clock time.
#[derive(Clone, Copy)]
struct Warned {
    condition: Condition,
    at: SystemTime,
}

/// The last warning per certificate of one loaded material, in the order of its validity
/// list. A reloaded material starts empty, so a successful reload warns again at once.
pub(crate) struct Warnings(Mutex<Vec<Option<Warned>>>);

impl Warnings {
    pub(crate) fn new(len: usize) -> Warnings {
        Warnings(Mutex::new(vec![None; len]))
    }
}

/// The validity of every certificate in `certs` that can be parsed, in order, each distinct
/// certificate once.
pub(crate) fn of_certificates<'a>(
    certs: impl IntoIterator<Item = &'a CertificateDer<'a>>,
) -> Vec<CertValidity> {
    let mut seen: Vec<&[u8]> = Vec::new();
    let mut list = Vec::new();
    for cert in certs {
        let der = cert.as_ref();
        if seen.contains(&der) {
            continue;
        }
        seen.push(der);
        // rustls accepted the certificate, so it parses; one that does not is left out.
        if let Ok((_, parsed)) = x509_parser::parse_x509_certificate(der) {
            list.push(CertValidity {
                subject: subject(&parsed),
                not_before: system_time(parsed.validity().not_before.timestamp()),
                not_after: system_time(parsed.validity().not_after.timestamp()),
            });
        }
    }
    list
}

/// The common name of `cert`, else its first DNS name, else [`NO_SUBJECT`].
fn subject(cert: &X509Certificate<'_>) -> String {
    let common_name = cert
        .subject()
        .iter_common_name()
        .find_map(|cn| cn.as_str().ok().map(str::to_string));
    let dns_name = || {
        let san = cert.subject_alternative_name().ok().flatten()?;
        san.value.general_names.iter().find_map(|name| match name {
            GeneralName::DNSName(dns) => Some(dns.to_string()),
            _ => None,
        })
    };
    common_name
        .or_else(dns_name)
        .unwrap_or_else(|| NO_SUBJECT.to_string())
}

/// The time `seconds` after (or, when negative, before) the Unix epoch.
fn system_time(seconds: i64) -> SystemTime {
    let offset = Duration::from_secs(seconds.unsigned_abs());
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH + offset
    } else {
        SystemTime::UNIX_EPOCH - offset
    }
}

/// What is wrong with `validity` at `now`, with `window` as the expiry warning time.
fn condition(validity: &CertValidity, now: SystemTime, window: Duration) -> Option<Condition> {
    if now < validity.not_before {
        Some(Condition::NotYetValid)
    } else if now > validity.not_after {
        Some(Condition::Expired)
    } else if validity
        .not_after
        .duration_since(now)
        .is_ok_and(|left| left <= window)
    {
        Some(Condition::ExpiresSoon)
    } else {
        None
    }
}

/// Whether a warning about `condition` at `now` is due after the `last` one: always for a new
/// condition, and otherwise once the clock is 24 hours away from the last warning, in either
/// direction, so a clock set back far does not silence the warning.
fn due(last: Option<Warned>, condition: Condition, now: SystemTime) -> bool {
    match last {
        None => true,
        Some(last) if last.condition != condition => true,
        Some(last) => {
            let distance = now.duration_since(last.at).unwrap_or_else(|e| e.duration());
            distance >= REPEAT
        }
    }
}

/// Warns on `oxbloom::ssl` about every certificate of `material` that is not valid at `now`
/// or expires within `window`, unless the same warning was given less than 24 hours ago. The
/// event names the bundle and the subject, never key material.
pub(crate) fn check(bundle: &str, material: &Material, now: SystemTime, window: Duration) {
    let mut warned = lock(&material.warned.0);
    for (validity, last) in material.validity.iter().zip(warned.iter_mut()) {
        let Some(condition) = condition(validity, now, window) else {
            *last = None;
            continue;
        };
        if !due(*last, condition, now) {
            continue;
        }
        *last = Some(Warned { condition, at: now });
        let subject = validity.subject.as_str();
        match condition {
            Condition::NotYetValid => tracing::warn!(
                target: TARGET,
                bundle,
                subject,
                not_before = %utc(validity.not_before),
                "a certificate of the SSL bundle is not valid yet"
            ),
            Condition::Expired => tracing::warn!(
                target: TARGET,
                bundle,
                subject,
                not_after = %utc(validity.not_after),
                "a certificate of the SSL bundle has expired"
            ),
            Condition::ExpiresSoon => tracing::warn!(
                target: TARGET,
                bundle,
                subject,
                not_after = %utc(validity.not_after),
                "a certificate of the SSL bundle expires soon"
            ),
        }
    }
}

/// `time` as an RFC 3339 UTC date with whole seconds, such as `2030-01-31T12:00:00Z`.
fn utc(time: SystemTime) -> String {
    let seconds = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => after.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    };
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The year, month and day of the day `days` after 1970-01-01, in the proleptic Gregorian
/// calendar (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validity(not_before: u64, not_after: u64) -> CertValidity {
        CertValidity {
            subject: "web.test".into(),
            not_before: SystemTime::UNIX_EPOCH + Duration::from_secs(not_before),
            not_after: SystemTime::UNIX_EPOCH + Duration::from_secs(not_after),
        }
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn dates_are_written_as_utc_rfc_3339() {
        assert_eq!(utc(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(utc(at(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(utc(at(1_893_456_000 + 45_296)), "2030-01-01T12:34:56Z");
        assert_eq!(
            utc(SystemTime::UNIX_EPOCH - Duration::from_secs(1)),
            "1969-12-31T23:59:59Z"
        );
    }

    #[test]
    fn the_condition_depends_on_the_time_and_the_window() {
        let v = validity(100, 1000);
        let window = Duration::from_secs(100);
        assert_eq!(condition(&v, at(99), window), Some(Condition::NotYetValid));
        assert_eq!(condition(&v, at(100), window), None);
        assert_eq!(condition(&v, at(899), window), None);
        assert_eq!(condition(&v, at(900), window), Some(Condition::ExpiresSoon));
        assert_eq!(
            condition(&v, at(1000), window),
            Some(Condition::ExpiresSoon)
        );
        assert_eq!(condition(&v, at(1001), window), Some(Condition::Expired));
        assert_eq!(condition(&v, at(500), Duration::ZERO), None);
    }

    #[test]
    fn a_warning_repeats_only_after_24_hours_or_for_a_new_condition() {
        let day = REPEAT.as_secs();
        let last = Some(Warned {
            condition: Condition::Expired,
            at: at(10 * day),
        });
        assert!(due(None, Condition::Expired, at(0)));
        assert!(!due(last, Condition::Expired, at(11 * day - 1)));
        assert!(due(last, Condition::Expired, at(11 * day)));
        assert!(due(last, Condition::ExpiresSoon, at(10 * day)));
        assert!(!due(last, Condition::Expired, at(9 * day + 1)));
        assert!(due(last, Condition::Expired, at(9 * day)));
    }

    #[test]
    fn negative_timestamps_are_before_the_epoch() {
        assert_eq!(
            system_time(-5),
            SystemTime::UNIX_EPOCH - Duration::from_secs(5)
        );
        assert_eq!(system_time(5), at(5));
    }
}
