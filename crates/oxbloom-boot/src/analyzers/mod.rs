//! The built-in failure analyzers, asked after the builder and bean analyzers.

use std::sync::Arc;

mod autoconfig;
mod beans;
mod config;
#[cfg(feature = "logging")]
mod logging;
#[cfg(feature = "web")]
mod port;
#[cfg(feature = "security")]
mod security;
#[cfg(feature = "ssl")]
mod ssl;

use crate::failure::FailureAnalyzer;

/// The built-in analyzers in match order.
pub(crate) fn builtin() -> Vec<Arc<dyn FailureAnalyzer>> {
    #[allow(unused_mut)]
    let mut analyzers: Vec<Arc<dyn FailureAnalyzer>> = vec![
        Arc::new(config::MissingConfigKeyAnalyzer),
        Arc::new(beans::BeanWiringAnalyzer),
        Arc::new(autoconfig::AutoConfigurationCycleAnalyzer),
    ];
    #[cfg(feature = "web")]
    analyzers.push(Arc::new(port::PortInUseAnalyzer));
    #[cfg(feature = "security")]
    analyzers.push(Arc::new(security::SecurityAnalyzer));
    #[cfg(feature = "security")]
    analyzers.push(Arc::new(security::JwtConfigAnalyzer));
    #[cfg(feature = "logging")]
    analyzers.push(Arc::new(logging::LoggingConfigAnalyzer));
    #[cfg(feature = "ssl")]
    analyzers.push(Arc::new(ssl::SslAnalyzer));
    analyzers
}
