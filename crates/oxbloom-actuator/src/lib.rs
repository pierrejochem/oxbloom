//! Production-ready endpoints for Oxbloom: health, availability and application info.
//!
//! [`Availability`] holds the liveness and readiness states, and a [`HealthIndicator`]
//! reports the [`Health`] of one dependency. [`Status::worst`] combines several statuses
//! into one.
//!
//! # Examples
//!
//! ```
//! use oxbloom_actuator::{Availability, Readiness, Status};
//!
//! let availability = Availability::new();
//! availability.set_readiness(Readiness::AcceptingTraffic);
//! assert_eq!(availability.readiness(), Readiness::AcceptingTraffic);
//! assert_eq!(Status::worst([Status::Up, Status::Down]), Status::Down);
//! ```

mod availability;
pub mod build;
mod health;
mod info;
mod settings;

#[cfg(feature = "web")]
mod controller;

pub use availability::{Availability, Liveness, Readiness};
pub use health::{Health, HealthIndicator, HealthRunner, Status};
pub use info::{BuildInfo, Info, InfoContributor, Platform};
pub use settings::ActuatorSettings;

#[cfg(feature = "web")]
pub use controller::ActuatorController;
