//! A second `LoggingSystem::init` in the same process is refused.

use oxbloom_config::Config;
use oxbloom_logging::{LoggingError, LoggingSystem};

#[test]
fn second_init_is_already_installed() {
    let config = Config::from_str("[logging.console]\ncolor = \"never\"").unwrap();
    let first = LoggingSystem::init_with_writer(&config, std::io::sink);
    assert!(first.is_ok());
    assert!(matches!(
        LoggingSystem::init(&config),
        Err(LoggingError::AlreadyInstalled)
    ));
    assert!(matches!(
        LoggingSystem::init_with_writer(&config, std::io::sink),
        Err(LoggingError::AlreadyInstalled)
    ));
}
