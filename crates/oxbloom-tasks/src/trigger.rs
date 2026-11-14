//! Triggers: when a scheduled task runs next.

use std::fmt;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;

/// The last year the `cron` crate evaluates; later instants have no next fire time.
const LAST_CRON_YEAR: i32 = 2100;

/// An error building a trigger, a zone or a duration.
///
/// The message names what was wrong but never repeats the offending value, so a value that
/// came from a secret or a mistyped property does not leak into logs.
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::CronExpr;
///
/// let err = CronExpr::parse("*/5 * * * *").unwrap_err();
/// assert!(err.to_string().contains("six fields"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerError {
    message: &'static str,
}

impl TriggerError {
    const fn new(message: &'static str) -> Self {
        Self { message }
    }
}

impl fmt::Display for TriggerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for TriggerError {}

/// The time zone a cron expression is evaluated in.
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::Zone;
///
/// assert_eq!(Zone::parse("UTC")?, Zone::Utc);
/// assert_eq!(Zone::parse("Europe/Amsterdam")?.to_string(), "Europe/Amsterdam");
/// assert!(Zone::parse("Mars/Olympus_Mons").is_err());
/// # Ok::<(), oxbloom_tasks::TriggerError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zone {
    /// Coordinated Universal Time, the default.
    #[default]
    Utc,
    /// A named IANA zone such as `Europe/Amsterdam`, with its daylight saving rules.
    Named(Tz),
}

impl Zone {
    /// Parses `UTC` or an IANA zone name such as `Europe/Amsterdam`.
    ///
    /// # Errors
    ///
    /// Returns a [`TriggerError`] when the name is not a known zone.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::Zone;
    ///
    /// let zone = Zone::parse("America/New_York")?;
    /// assert_eq!(zone, Zone::Named(chrono_tz::America::New_York));
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    pub fn parse(name: &str) -> Result<Zone, TriggerError> {
        let name = name.trim();
        if name == "UTC" {
            return Ok(Zone::Utc);
        }
        Tz::from_str(name)
            .map(Zone::Named)
            .map_err(|_| TriggerError::new("unknown time zone, expected UTC or an IANA zone name"))
    }
}

impl fmt::Display for Zone {
    /// Writes `UTC` or the IANA name.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(oxbloom_tasks::Zone::Utc.to_string(), "UTC");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Zone::Utc => f.write_str("UTC"),
            Zone::Named(tz) => f.write_str(tz.name()),
        }
    }
}

/// A six-field cron expression: second, minute, hour, day of month, month, day of week.
///
/// Five-field (Unix) and seven-field (with a year) expressions are rejected, as are macros
/// such as `@daily`. Months are `1`-`12` or `JAN`-`DEC`.
///
/// Days of the week follow Spring: `1` is Monday through `6` Saturday, and both `0` and `7`
/// are Sunday; the names `SUN`-`SAT` (any case) work too. Numeric days are translated to the
/// evaluating library's own numbering (which starts at Sunday = `1`), so `1-5` means Monday to
/// Friday, `5-7` Friday to Sunday, and a step such as `*/2` counts from Monday (Monday,
/// Wednesday, Friday, Sunday). A day number of `8` or more is an error. [`CronExpr::as_str`]
/// returns the expression as written, not the translation.
///
/// ```
/// use std::time::{Duration, UNIX_EPOCH};
/// use oxbloom_tasks::{Trigger, Zone};
///
/// let weekdays = Trigger::cron("0 0 9 * * 1-5", Zone::Utc)?;
/// let saturday_noon = UNIX_EPOCH + Duration::from_secs(1_794_657_600); // 2026-11-14 12:00Z
/// let monday_nine = UNIX_EPOCH + Duration::from_secs(1_794_819_600); // 2026-11-16 09:00Z
/// assert_eq!(weekdays.next(None, None, saturday_noon), Some(monday_nine));
/// # Ok::<(), oxbloom_tasks::TriggerError>(())
/// ```
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::CronExpr;
///
/// let every_quarter_hour = CronExpr::parse("0 */15 * * * *")?;
/// assert_eq!(every_quarter_hour.as_str(), "0 */15 * * * *");
/// assert!(CronExpr::parse("*/15 * * * *").is_err());
/// # Ok::<(), oxbloom_tasks::TriggerError>(())
/// ```
#[derive(Debug, Clone)]
pub struct CronExpr {
    source: String,
    // Boxed: a parsed schedule is a few hundred bytes and would bloat every `Trigger`.
    schedule: Box<cron::Schedule>,
}

impl CronExpr {
    /// Parses a six-field cron expression, seconds first.
    ///
    /// # Errors
    ///
    /// Returns a [`TriggerError`] when the expression does not have exactly six fields or a
    /// field is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::CronExpr;
    ///
    /// assert!(CronExpr::parse("0 0 9 * * MON-FRI").is_ok());
    /// assert!(CronExpr::parse("0 0 9 * * MON-FRI 2030").is_err());
    /// ```
    pub fn parse(expression: &str) -> Result<CronExpr, TriggerError> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        if fields.len() != 6 {
            return Err(TriggerError::new(
                "cron expression must have exactly six fields: second, minute, hour, day of \
                 month, month, day of week",
            ));
        }
        let source = fields.join(" ");
        let translated = format!(
            "{} {}",
            fields[..5].join(" "),
            translate_day_of_week(fields[5])?
        );
        let schedule = cron::Schedule::from_str(&translated)
            .map_err(|_| TriggerError::new("invalid cron expression"))?;
        Ok(CronExpr {
            source,
            schedule: Box::new(schedule),
        })
    }

    /// The expression, with its fields separated by single spaces.
    ///
    /// # Examples
    ///
    /// ```
    /// let expr = oxbloom_tasks::CronExpr::parse("0  0   12 * * *")?;
    /// assert_eq!(expr.as_str(), "0 0 12 * * *");
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// The first matching instant strictly after `now` in `zone`.
    ///
    /// A local time skipped by a daylight saving change does not fire; a local time that
    /// happens twice fires only at its first occurrence.
    fn next_after(&self, now: SystemTime, zone: Zone) -> Option<SystemTime> {
        let now = to_utc(now)?;
        if now.year() > LAST_CRON_YEAR {
            return None;
        }
        let next = match zone {
            Zone::Utc => self.next_in(now, Utc),
            Zone::Named(tz) => self.next_in(now, tz),
        }?;
        from_utc(next)
    }

    fn next_in<Z: TimeZone>(&self, now: DateTime<Utc>, zone: Z) -> Option<DateTime<Utc>> {
        let mut cursor = now.with_timezone(&zone);
        // Each round moves the cursor past one repeated local time, so the loop ends; the
        // bound only guards against a library change.
        for _ in 0..100_000 {
            let candidate = self.schedule.after(&cursor).next()?;
            let candidate_utc = candidate.with_timezone(&Utc);
            if candidate_utc > now {
                return Some(candidate_utc);
            }
            // The candidate is the first occurrence of a repeated local time that already
            // passed: skip its second occurrence too.
            match zone.from_local_datetime(&candidate.naive_local()) {
                LocalResult::Ambiguous(_, later) if later.with_timezone(&Utc) > now => {
                    cursor = later;
                }
                _ => cursor = now.with_timezone(&zone),
            }
            if cursor.with_timezone(&Utc) <= candidate_utc {
                return None;
            }
        }
        None
    }
}

impl PartialEq for CronExpr {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for CronExpr {}

impl fmt::Display for CronExpr {
    /// Writes the expression.
    ///
    /// # Examples
    ///
    /// ```
    /// let expr = oxbloom_tasks::CronExpr::parse("0 0 12 * * *")?;
    /// assert_eq!(expr.to_string(), "0 0 12 * * *");
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

/// When a scheduled task runs next.
///
/// # Examples
///
/// ```
/// use std::time::{Duration, UNIX_EPOCH};
/// use oxbloom_tasks::Trigger;
///
/// let trigger = Trigger::fixed_delay(Duration::from_secs(5))?;
/// let start = UNIX_EPOCH + Duration::from_secs(100);
/// let end = start + Duration::from_secs(2);
/// assert_eq!(trigger.next(Some(start), Some(end), end), Some(end + Duration::from_secs(5)));
/// # Ok::<(), oxbloom_tasks::TriggerError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// Runs every period, measured from the start of the last run.
    FixedRate(Duration),
    /// Runs after a delay, measured from the end of the last run.
    FixedDelay(Duration),
    /// Runs at the instants matching a cron expression in a zone.
    Cron(CronExpr, Zone),
}

impl Trigger {
    /// A fixed-rate trigger: the next run starts `period` after the last one started.
    ///
    /// # Errors
    ///
    /// Returns a [`TriggerError`] when `period` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::Trigger;
    ///
    /// assert!(Trigger::fixed_rate(Duration::from_secs(30)).is_ok());
    /// assert!(Trigger::fixed_rate(Duration::ZERO).is_err());
    /// ```
    pub fn fixed_rate(period: Duration) -> Result<Trigger, TriggerError> {
        if period.is_zero() {
            return Err(TriggerError::new("fixed rate must be greater than zero"));
        }
        Ok(Trigger::FixedRate(period))
    }

    /// A fixed-delay trigger: the next run starts `delay` after the last one ended.
    ///
    /// # Errors
    ///
    /// Returns a [`TriggerError`] when `delay` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::Trigger;
    ///
    /// assert!(Trigger::fixed_delay(Duration::from_millis(500)).is_ok());
    /// assert!(Trigger::fixed_delay(Duration::ZERO).is_err());
    /// ```
    pub fn fixed_delay(delay: Duration) -> Result<Trigger, TriggerError> {
        if delay.is_zero() {
            return Err(TriggerError::new("fixed delay must be greater than zero"));
        }
        Ok(Trigger::FixedDelay(delay))
    }

    /// A cron trigger from a six-field expression evaluated in `zone`.
    ///
    /// # Errors
    ///
    /// Returns a [`TriggerError`] when the expression is invalid (see [`CronExpr::parse`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::{Trigger, Zone};
    ///
    /// let nightly = Trigger::cron("0 0 3 * * *", Zone::parse("Europe/Amsterdam")?)?;
    /// assert_eq!(nightly.describe(), "cron 0 0 3 * * * (Europe/Amsterdam)");
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    pub fn cron(expression: &str, zone: Zone) -> Result<Trigger, TriggerError> {
        Ok(Trigger::Cron(CronExpr::parse(expression)?, zone))
    }

    /// A short description for logs and endpoints, such as `fixed rate 5s`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::Trigger;
    ///
    /// let trigger = Trigger::fixed_rate(Duration::from_millis(1500))?;
    /// assert_eq!(trigger.describe(), "fixed rate 1.5s");
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    pub fn describe(&self) -> String {
        match self {
            Trigger::FixedRate(period) => format!("fixed rate {period:?}"),
            Trigger::FixedDelay(delay) => format!("fixed delay {delay:?}"),
            Trigger::Cron(expr, zone) => format!("cron {expr} ({zone})"),
        }
    }

    /// The next time the task should run.
    ///
    /// A fixed rate is `last_start + period` and a fixed delay is `last_end + delay`; both
    /// are `now` when there is no such previous run. A cron trigger returns the first
    /// matching instant strictly after `now` and ignores the history. Missed instants are
    /// not caught up: the result is never earlier than `now`. Returns `None` when there is
    /// no next time or it does not fit in a [`SystemTime`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, UNIX_EPOCH};
    /// use oxbloom_tasks::Trigger;
    ///
    /// let trigger = Trigger::fixed_rate(Duration::from_secs(10))?;
    /// let start = UNIX_EPOCH + Duration::from_secs(1_000);
    /// let now = start + Duration::from_secs(4);
    /// assert_eq!(trigger.next(None, None, now), Some(now));
    /// assert_eq!(trigger.next(Some(start), None, now), Some(start + Duration::from_secs(10)));
    /// // Far behind schedule: run now rather than at a past instant.
    /// let late = start + Duration::from_secs(3_600);
    /// assert_eq!(trigger.next(Some(start), None, late), Some(late));
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    pub fn next(
        &self,
        last_start: Option<SystemTime>,
        last_end: Option<SystemTime>,
        now: SystemTime,
    ) -> Option<SystemTime> {
        let after = |last: Option<SystemTime>, period: &Duration| match last {
            None => Some(now),
            Some(last) => last.checked_add(*period).map(|next| next.max(now)),
        };
        match self {
            Trigger::FixedRate(period) => after(last_start, period),
            Trigger::FixedDelay(delay) => after(last_end, delay),
            Trigger::Cron(expr, zone) => expr.next_after(now, *zone),
        }
    }
}

/// Parses a duration such as `250ms`, `5s`, `2m`, `1h` or `1d`.
///
/// The value is a whole number followed by one of the units `ms`, `s`, `m`, `h` or `d`;
/// surrounding whitespace is ignored. Zero is a valid duration.
///
/// # Errors
///
/// Returns a [`TriggerError`] for a missing or unknown unit, a missing number, a fraction or
/// sign, or a value too large for a [`Duration`].
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_tasks::parse_duration;
///
/// assert_eq!(parse_duration("90s")?, Duration::from_secs(90));
/// assert_eq!(parse_duration("250ms")?, Duration::from_millis(250));
/// assert!(parse_duration("5").is_err());
/// # Ok::<(), oxbloom_tasks::TriggerError>(())
/// ```
pub fn parse_duration(text: &str) -> Result<Duration, TriggerError> {
    const INVALID: TriggerError =
        TriggerError::new("invalid duration, expected a whole number with ms, s, m, h or d");
    const TOO_LARGE: TriggerError = TriggerError::new("duration out of range");
    let text = text.trim();
    let digits_end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(digits_end);
    if digits.is_empty() {
        return Err(INVALID);
    }
    let millis_per_unit: u64 = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(INVALID),
    };
    let amount: u64 = digits.parse().map_err(|_| TOO_LARGE)?;
    amount
        .checked_mul(millis_per_unit)
        .map(Duration::from_millis)
        .ok_or(TOO_LARGE)
}

/// The day names in the evaluating library's order, Sunday first; index = Spring day % 7.
const DAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// Translates a Spring day-of-week field (`0`/`7` = Sunday, `1` = Monday) into one the `cron`
/// crate reads the same way. Wildcards and elements with day names pass through unchanged;
/// numeric elements (single, range, step) become explicit day-name lists, which also keeps a
/// range such as `5-7` that crosses Sunday intact.
fn translate_day_of_week(field: &str) -> Result<String, TriggerError> {
    if field == "*" || field == "?" || !field.bytes().any(|b| b.is_ascii_digit()) {
        return Ok(field.to_owned());
    }
    let mut out = Vec::new();
    for element in field.split(',') {
        if element.bytes().any(|b| b.is_ascii_alphabetic()) {
            out.push(element.to_owned());
            continue;
        }
        let days = spring_days(element)?;
        out.extend(
            days.into_iter()
                .map(|day| DAY_NAMES[day as usize % 7].to_owned()),
        );
    }
    Ok(out.join(","))
}

/// The Spring day numbers (0-7) one numeric day-of-week element selects.
fn spring_days(element: &str) -> Result<Vec<u32>, TriggerError> {
    const INVALID: TriggerError =
        TriggerError::new("invalid day of week, expected 0-7 (0 and 7 are Sunday) or SUN-SAT");
    let number = |text: &str| -> Result<u32, TriggerError> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(INVALID);
        }
        match text.parse::<u32>() {
            Ok(day) if day <= 7 => Ok(day),
            _ => Err(INVALID),
        }
    };
    let (base, step) = match element.split_once('/') {
        Some((base, step)) => {
            let step: u32 = step.parse().map_err(|_| INVALID)?;
            if step == 0
                || !element[base.len() + 1..]
                    .bytes()
                    .all(|b| b.is_ascii_digit())
            {
                return Err(INVALID);
            }
            (base, Some(step))
        }
        None => (element, None),
    };
    // Spring's full range for a wildcard or an open start runs from Monday (1) to Sunday (7).
    let (start, end) = match base {
        "*" | "?" => (1, 7),
        _ => match base.split_once('-') {
            Some((from, to)) => (number(from)?, number(to)?),
            None if step.is_some() => (number(base)?, 7),
            None => {
                let day = number(base)?;
                (day, day)
            }
        },
    };
    if start > end {
        return Err(INVALID);
    }
    let step = step.unwrap_or(1) as usize;
    Ok((start..=end).step_by(step).collect())
}

/// A [`SystemTime`] as a UTC date-time, or `None` outside chrono's range.
fn to_utc(time: SystemTime) -> Option<DateTime<Utc>> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => {
            let secs = i64::try_from(since.as_secs()).ok()?;
            DateTime::from_timestamp(secs, since.subsec_nanos())
        }
        Err(before) => {
            let before = before.duration();
            let secs = i64::try_from(before.as_secs()).ok()?;
            let nanos = before.subsec_nanos();
            // `before` seconds and nanos earlier: borrow one second for a nanosecond part.
            if nanos == 0 {
                DateTime::from_timestamp(secs.checked_neg()?, 0)
            } else {
                DateTime::from_timestamp(secs.checked_neg()?.checked_sub(1)?, 1_000_000_000 - nanos)
            }
        }
    }
}

/// A UTC date-time as a [`SystemTime`], or `None` when it does not fit.
fn from_utc(time: DateTime<Utc>) -> Option<SystemTime> {
    let secs = time.timestamp();
    let nanos = Duration::from_nanos(u64::from(time.timestamp_subsec_nanos()));
    let base = if secs >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(secs.unsigned_abs()))?
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(secs.unsigned_abs()))?
    };
    base.checked_add(nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// The instant of a UTC wall-clock time.
    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> SystemTime {
        let dt = Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap();
        UNIX_EPOCH + Duration::from_secs(dt.timestamp() as u64)
    }

    #[test]
    fn cron_needs_exactly_six_fields() {
        assert!(CronExpr::parse("0 30 * * * *").is_ok());
        assert!(CronExpr::parse("30 * * * *").is_err(), "five fields");
        assert!(
            CronExpr::parse("0 30 * * * * 2030").is_err(),
            "seven fields"
        );
        assert!(CronExpr::parse("@daily").is_err());
        assert!(CronExpr::parse("").is_err());
        assert!(
            CronExpr::parse("0 61 * * * *").is_err(),
            "minute out of range"
        );
    }

    #[test]
    fn cron_next_fire_in_utc() {
        let trigger = Trigger::cron("0 30 * * * *", Zone::Utc).unwrap();
        let now = utc(2026, 11, 14, 10, 10, 0);
        assert_eq!(
            trigger.next(None, None, now),
            Some(utc(2026, 11, 14, 10, 30, 0))
        );
        // Strictly after now: at 10:30:00 itself the next one is 11:30:00.
        let at = utc(2026, 11, 14, 10, 30, 0);
        assert_eq!(
            trigger.next(None, None, at),
            Some(utc(2026, 11, 14, 11, 30, 0))
        );
        // Sub-second precision: half a second before still fires at 10:30:00.
        let just_before = at - Duration::from_millis(500);
        assert_eq!(trigger.next(None, None, just_before), Some(at));
    }

    #[test]
    fn cron_next_fire_in_a_dst_zone() {
        let zone = Zone::parse("Europe/Amsterdam").unwrap();
        let trigger = Trigger::cron("0 30 2 * * *", zone).unwrap();

        // Spring forward on 2026-03-29: 02:00 CET jumps to 03:00 CEST, 02:30 does not exist.
        let before_spring = utc(2026, 3, 28, 23, 0, 0); // 00:00 CET
        assert_eq!(
            trigger.next(None, None, before_spring),
            Some(utc(2026, 3, 30, 0, 30, 0)), // 02:30 CEST on the next day
        );

        // Fall back on 2026-10-25: 03:00 CEST goes back to 02:00 CET, 02:30 happens twice.
        let before_fall = utc(2026, 10, 24, 22, 0, 0); // 00:00 CEST
        let first = trigger.next(None, None, before_fall);
        assert_eq!(first, Some(utc(2026, 10, 25, 0, 30, 0))); // 02:30 CEST
        let next_day = Some(utc(2026, 10, 26, 1, 30, 0)); // 02:30 CET on the next day
        assert_eq!(trigger.next(None, None, first.unwrap()), next_day);
        // Inside the repeated hour (02:00 CET, the second pass) the repeat is not fired.
        let second_pass = utc(2026, 10, 25, 1, 0, 0);
        assert_eq!(trigger.next(None, None, second_pass), next_day);
    }

    #[test]
    fn cron_month_end_and_leap_day() {
        let trigger = Trigger::cron("0 0 0 29 2 *", Zone::Utc).unwrap();
        let now = utc(2026, 11, 14, 0, 0, 0);
        assert_eq!(
            trigger.next(None, None, now),
            Some(utc(2028, 2, 29, 0, 0, 0))
        );
        let month_end = Trigger::cron("0 0 0 31 * *", Zone::Utc).unwrap();
        assert_eq!(
            month_end.next(None, None, now),
            Some(utc(2026, 12, 31, 0, 0, 0))
        );
    }

    #[test]
    fn fixed_rate_is_measured_from_the_start() {
        let t = utc(2026, 11, 14, 10, 0, 0);
        let trigger = Trigger::fixed_rate(Duration::from_secs(10)).unwrap();
        let next = trigger.next(
            Some(t),
            Some(t + Duration::from_secs(3)),
            t + Duration::from_secs(4),
        );
        assert_eq!(next, Some(t + Duration::from_secs(10)));
    }

    #[test]
    fn fixed_delay_is_measured_from_the_end() {
        let t = utc(2026, 11, 14, 10, 0, 0);
        let trigger = Trigger::fixed_delay(Duration::from_secs(10)).unwrap();
        let next = trigger.next(
            Some(t),
            Some(t + Duration::from_secs(3)),
            t + Duration::from_secs(4),
        );
        assert_eq!(next, Some(t + Duration::from_secs(13)));
    }

    #[test]
    fn first_run_is_now_without_history() {
        let now = utc(2026, 11, 14, 10, 0, 0);
        let rate = Trigger::fixed_rate(Duration::from_secs(10)).unwrap();
        let delay = Trigger::fixed_delay(Duration::from_secs(10)).unwrap();
        assert_eq!(rate.next(None, None, now), Some(now));
        assert_eq!(delay.next(None, None, now), Some(now));
    }

    #[test]
    fn zero_period_is_rejected() {
        assert!(Trigger::fixed_rate(Duration::ZERO).is_err());
        assert!(Trigger::fixed_delay(Duration::ZERO).is_err());
        assert!(Trigger::fixed_rate(Duration::from_nanos(1)).is_ok());
    }

    #[test]
    fn next_never_goes_back_in_time() {
        let t = utc(2026, 11, 14, 10, 0, 0);
        let late = t + Duration::from_secs(3600);
        let rate = Trigger::fixed_rate(Duration::from_secs(10)).unwrap();
        assert_eq!(rate.next(Some(t), Some(t), late), Some(late));
        let delay = Trigger::fixed_delay(Duration::from_secs(10)).unwrap();
        assert_eq!(delay.next(Some(t), Some(t), late), Some(late));
    }

    #[test]
    fn huge_duration_does_not_overflow() {
        let t = utc(2026, 11, 14, 10, 0, 0);
        let rate = Trigger::fixed_rate(Duration::MAX).unwrap();
        assert_eq!(rate.next(Some(t), Some(t), t), None);
        let delay = Trigger::fixed_delay(Duration::MAX).unwrap();
        assert_eq!(delay.next(Some(t), Some(t), t), None);
        assert!(parse_duration("99999999999999999999d").is_err());
        assert!(parse_duration("213503982334602d").is_err());

        // A `now` beyond what chrono or the cron crate can represent has no next time.
        let far = UNIX_EPOCH.checked_add(Duration::from_secs(i64::MAX as u64 / 2));
        let cron = Trigger::cron("* * * * * *", Zone::Utc).unwrap();
        if let Some(far) = far {
            assert_eq!(cron.next(None, None, far), None);
        }
        let year_9999 = utc(9999, 12, 31, 23, 59, 59);
        assert_eq!(cron.next(None, None, year_9999), None);
    }

    #[test]
    fn durations_parse_with_units() {
        assert_eq!(parse_duration("250ms").unwrap(), Duration::from_millis(250));
        assert_eq!(parse_duration("5s").unwrap(), Duration::from_secs(5));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration(" 1d ").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_duration("0s").unwrap(), Duration::ZERO);
        for bad in ["", "5", "s", "-5s", "1.5s", "5 s", "5w", "5sec"] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn errors_do_not_echo_the_value() {
        let errors = [
            CronExpr::parse("MARKER-91 * * * * *").unwrap_err(),
            CronExpr::parse("0 MARKER-91 * * *").unwrap_err(),
            Zone::parse("MARKER-91/Zone").unwrap_err(),
            parse_duration("MARKER-91s").unwrap_err(),
            parse_duration("5MARKER-91").unwrap_err(),
            Trigger::cron("MARKER-91", Zone::Utc).unwrap_err(),
        ];
        for err in errors {
            let text = err.to_string();
            assert!(!text.is_empty());
            assert!(!text.contains("MARKER"), "{text}");
            assert!(!format!("{err:?}").contains("MARKER"), "{err:?}");
        }
    }

    #[test]
    fn unknown_zone_is_an_error() {
        assert!(Zone::parse("Mars/Olympus_Mons").is_err());
        assert!(Zone::parse("").is_err());
        assert_eq!(Zone::parse("UTC").unwrap(), Zone::Utc);
        assert_eq!(
            Zone::parse("Europe/Amsterdam").unwrap(),
            Zone::Named(chrono_tz::Europe::Amsterdam)
        );
    }

    #[test]
    fn system_time_conversion_round_trips() {
        let before_epoch = UNIX_EPOCH - Duration::from_millis(1500);
        let dt = to_utc(before_epoch).unwrap();
        assert_eq!(dt.timestamp_millis(), -1500);
        assert_eq!(from_utc(dt), Some(before_epoch));
        let after_epoch = UNIX_EPOCH + Duration::new(1_800_000_000, 123);
        assert_eq!(from_utc(to_utc(after_epoch).unwrap()), Some(after_epoch));
    }

    /// The next fire time of a UTC cron expression after `now`.
    fn next_utc(expression: &str, now: SystemTime) -> Option<SystemTime> {
        Trigger::cron(expression, Zone::Utc)
            .unwrap()
            .next(None, None, now)
    }

    /// Every fire time in the week starting on Sunday 2026-11-15 00:00 UTC, as weekday
    /// numbers from Sunday (0) to Saturday (6), for a cron expression firing daily at 09:00.
    fn weekdays_fired(day_of_week: &str) -> Vec<u32> {
        let expression = format!("0 0 9 * * {day_of_week}");
        let start = utc(2026, 11, 15, 0, 0, 0); // a Sunday
        let end = start + Duration::from_secs(7 * 86_400);
        let mut days = Vec::new();
        let mut now = start;
        while let Some(next) = next_utc(&expression, now) {
            if next >= end {
                break;
            }
            let dt = to_utc(next).unwrap();
            days.push(dt.weekday().num_days_from_sunday());
            now = next;
        }
        days
    }

    #[test]
    fn spring_numeric_day_of_week_one_to_five_fires_on_monday_not_sunday() {
        let saturday = utc(2026, 11, 14, 12, 0, 0);
        let monday_nine = utc(2026, 11, 16, 9, 0, 0);
        assert_eq!(next_utc("0 0 9 * * 1-5", saturday), Some(monday_nine));
        let friday_nine = utc(2026, 11, 20, 9, 0, 0);
        assert_eq!(
            next_utc("0 0 9 * * 1-5", friday_nine),
            Some(utc(2026, 11, 23, 9, 0, 0))
        );
        assert_eq!(weekdays_fired("1-5"), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn zero_and_seven_are_sunday() {
        assert_eq!(weekdays_fired("0"), vec![0]);
        assert_eq!(weekdays_fired("7"), vec![0]);
        assert_eq!(weekdays_fired("1"), vec![1]);
        assert_eq!(weekdays_fired("6"), vec![6]);
        assert_eq!(weekdays_fired("0-2"), vec![0, 1, 2]);
    }

    #[test]
    fn list_range_and_step_forms_translate() {
        assert_eq!(weekdays_fired("1,3,5"), vec![1, 3, 5]);
        assert_eq!(weekdays_fired("5-7"), vec![0, 5, 6]);
        assert_eq!(weekdays_fired("6-7"), vec![0, 6]);
        // Spring steps over MON (1) to SUN (7).
        assert_eq!(weekdays_fired("*/2"), vec![0, 1, 3, 5]);
        assert_eq!(weekdays_fired("1-5/2"), vec![1, 3, 5]);
        assert_eq!(weekdays_fired("1/2"), vec![0, 1, 3, 5]);
        assert_eq!(weekdays_fired("2/3"), vec![2, 5]);
        assert_eq!(weekdays_fired("0,6"), vec![0, 6]);
        assert_eq!(weekdays_fired("MON,3,5-7"), vec![0, 1, 3, 5, 6]);
        assert_eq!(weekdays_fired("*"), vec![0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(weekdays_fired("?"), vec![0, 1, 2, 3, 4, 5, 6]);
        // The stored source stays the user's own expression.
        let expr = CronExpr::parse("0 0 9 * *  1-5").unwrap();
        assert_eq!(expr.as_str(), "0 0 9 * * 1-5");
    }

    #[test]
    fn names_are_untouched() {
        assert_eq!(weekdays_fired("MON-FRI"), vec![1, 2, 3, 4, 5]);
        assert_eq!(weekdays_fired("mon-fri"), vec![1, 2, 3, 4, 5]);
        assert_eq!(weekdays_fired("SUN"), vec![0]);
        assert_eq!(weekdays_fired("SAT,SUN"), vec![0, 6]);
    }

    #[test]
    fn out_of_range_day_number_is_an_error_without_the_value() {
        for bad in [
            "8",
            "1-8",
            "9,1",
            "*/0",
            "5-1",
            "1-",
            "-1",
            "1/",
            "1//2",
            "12345678901",
        ] {
            let err = CronExpr::parse(&format!("0 0 9 * * {bad}")).unwrap_err();
            let text = err.to_string();
            assert!(!text.contains(bad), "{bad:?}: {text}");
        }
        let err = CronExpr::parse("0 0 9 * * 98765").unwrap_err();
        assert!(!err.to_string().contains("98765"));
        assert!(!format!("{err:?}").contains("98765"));
    }

    #[test]
    fn describe_names_the_trigger() {
        let rate = Trigger::fixed_rate(Duration::from_secs(5)).unwrap();
        assert_eq!(rate.describe(), "fixed rate 5s");
        let delay = Trigger::fixed_delay(Duration::from_millis(250)).unwrap();
        assert_eq!(delay.describe(), "fixed delay 250ms");
        let zone = Zone::parse("Europe/Amsterdam").unwrap();
        let cron = Trigger::cron("0 30 2 * * *", zone).unwrap();
        assert_eq!(cron.describe(), "cron 0 30 2 * * * (Europe/Amsterdam)");
    }
}
