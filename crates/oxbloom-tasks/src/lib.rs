//! Task scheduling for Oxbloom: fixed-rate, fixed-delay and cron triggers.
//!
//! A [`Trigger`] decides when a task runs next. [`Trigger::fixed_rate`] measures the period
//! from the start of the last run, [`Trigger::fixed_delay`] from its end, and [`Trigger::cron`]
//! takes a six-field cron expression (seconds first) evaluated in a [`Zone`] (UTC by default,
//! or an IANA zone name).
//!
//! # Examples
//!
//! ```
//! use std::time::{Duration, UNIX_EPOCH};
//! use oxbloom_tasks::{Trigger, Zone};
//!
//! let trigger = Trigger::cron("0 30 * * * *", Zone::Utc)?;
//! let now = UNIX_EPOCH + Duration::from_secs(10 * 60);
//! assert_eq!(trigger.next(None, None, now), Some(UNIX_EPOCH + Duration::from_secs(30 * 60)));
//! # Ok::<(), oxbloom_tasks::TriggerError>(())
//! ```

mod trigger;

pub use trigger::{CronExpr, Trigger, TriggerError, Zone, parse_duration};
