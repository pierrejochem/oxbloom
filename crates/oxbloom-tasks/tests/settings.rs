//! `TasksSettings`: the `tasks.scheduling.*` keys, their defaults and their errors.

use std::time::Duration;

use oxbloom_config::Config;
use oxbloom_tasks::{SchedulerConfig, TasksSettings, Zone};

fn read(toml: &str) -> Result<TasksSettings, oxbloom_config::ConfigError> {
    TasksSettings::from_config(&Config::from_str(toml).unwrap())
}

#[test]
fn defaults() {
    let settings = read("").unwrap();
    assert_eq!(settings, TasksSettings::default());
    assert!(settings.enabled);
    assert_eq!(settings.max_concurrent, 8);
    assert_eq!(settings.shutdown_timeout, Duration::from_secs(10));
    assert_eq!(settings.zone, Zone::Utc);
    assert_eq!(settings.initial_delay, Duration::ZERO);
    // An empty section is the same as none.
    assert_eq!(read("[tasks.scheduling]\n").unwrap(), settings);
    assert_eq!(settings.scheduler_config(), SchedulerConfig::default());
}

#[test]
fn reads_every_key() {
    let settings = read(
        "[tasks.scheduling]\n\
         enabled = false\n\
         max-concurrent = 3\n\
         shutdown-timeout = \"2s\"\n\
         zone = \"Europe/Amsterdam\"\n\
         initial-delay = \"1500ms\"\n",
    )
    .unwrap();
    assert!(!settings.enabled);
    assert_eq!(settings.max_concurrent, 3);
    assert_eq!(settings.shutdown_timeout, Duration::from_secs(2));
    assert_eq!(settings.zone, Zone::parse("Europe/Amsterdam").unwrap());
    assert_eq!(settings.initial_delay, Duration::from_millis(1500));
    assert_eq!(
        settings.scheduler_config(),
        SchedulerConfig {
            max_concurrent: 3,
            shutdown_timeout: Duration::from_secs(2),
            zone: Zone::parse("Europe/Amsterdam").unwrap(),
            initial_delay: Duration::from_millis(1500),
        }
    );
}

#[test]
fn unknown_key_under_tasks_is_rejected_naming_it() {
    let err = read("[tasks]\nbogus-key = \"MARKER-41\"\n").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("bogus-key"), "{text}");
    assert!(
        !text.contains("MARKER-41"),
        "the value is not echoed: {text}"
    );
}

#[test]
fn unknown_key_under_scheduling_is_rejected() {
    let err = read("[tasks.scheduling]\nmax-concurent = 2\n").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("max-concurent"), "{text}");
}

#[test]
fn zero_max_concurrent_is_rejected() {
    let err = read("[tasks.scheduling]\nmax-concurrent = 0\n").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("tasks.scheduling.max-concurrent"), "{text}");
}

#[test]
fn bad_duration_names_the_key_not_the_value() {
    for key in ["shutdown-timeout", "initial-delay"] {
        let err = read(&format!(
            "[tasks.scheduling]\n{key} = \"MARKER-77 fortnights\"\n"
        ))
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains(&format!("tasks.scheduling.{key}")), "{text}");
        assert!(
            !text.contains("MARKER-77"),
            "the value is not echoed: {text}"
        );
    }
    let err = read("[tasks.scheduling]\nzone = \"MARKER-77/Nowhere\"\n").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("tasks.scheduling.zone"), "{text}");
    assert!(
        !text.contains("MARKER-77"),
        "the value is not echoed: {text}"
    );
}
