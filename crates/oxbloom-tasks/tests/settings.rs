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

mod retry {
    use std::time::Duration;

    use oxbloom_config::{Config, ConfigError};
    use oxbloom_tasks::{Backoff, Jitter, RetryDefaults, TasksSettings};

    fn read(toml: &str) -> Result<RetryDefaults, ConfigError> {
        RetryDefaults::from_config(&Config::from_str(toml).unwrap())
    }

    fn rejected(toml: &str, key: &str, value: &str) {
        let text = read(toml).unwrap_err().to_string();
        assert!(text.contains(key), "{text}");
        assert!(!text.contains(value), "{text}");
    }

    #[test]
    fn retry_defaults_without_a_section_are_the_built_in_values() {
        assert_eq!(read("").unwrap(), RetryDefaults::default());
        assert_eq!(read("[tasks.retry]\n").unwrap(), RetryDefaults::default());
        assert_eq!(
            read("[tasks.scheduling]\nmax-concurrent = 2").unwrap(),
            RetryDefaults::default()
        );
    }

    #[test]
    fn retry_keys_are_read() {
        let defaults = read(
            "[tasks.retry]\n\
             backoff = \"fixed\"\n\
             initial = \"250ms\"\n\
             max-delay = \"5s\"\n\
             multiplier = 3.0\n\
             jitter = \"equal\"\n\
             max-attempts = 4\n",
        )
        .unwrap();
        assert_eq!(defaults.backoff, Backoff::Fixed);
        assert_eq!(defaults.initial, Duration::from_millis(250));
        assert_eq!(defaults.max_delay, Duration::from_secs(5));
        assert_eq!(defaults.multiplier, 3.0);
        assert_eq!(defaults.jitter, Jitter::Equal);
        assert_eq!(defaults.max_attempts, 4);
    }

    #[test]
    fn retry_rejects_zero_attempts() {
        rejected(
            "[tasks.retry]\nmax-attempts = 0",
            "tasks.retry.max-attempts",
            "= 0",
        );
    }

    #[test]
    fn retry_rejects_multiplier_below_one() {
        rejected(
            "[tasks.retry]\nmultiplier = 0.5",
            "tasks.retry.multiplier",
            "0.5",
        );
    }

    #[test]
    fn retry_rejects_nan_multiplier() {
        rejected(
            "[tasks.retry]\nmultiplier = nan",
            "tasks.retry.multiplier",
            "nan",
        );
        rejected(
            "[tasks.retry]\nmultiplier = inf",
            "tasks.retry.multiplier",
            "inf",
        );
    }

    #[test]
    fn retry_rejects_initial_above_max_delay() {
        rejected(
            "[tasks.retry]\ninitial = \"7777ms\"\nmax-delay = \"1s\"",
            "tasks.retry.initial",
            "7777",
        );
    }

    #[test]
    fn retry_rejects_unknown_backoff_and_jitter_names() {
        rejected(
            "[tasks.retry]\nbackoff = \"zigzag\"",
            "tasks.retry.backoff",
            "zigzag",
        );
        rejected(
            "[tasks.retry]\njitter = \"wobbly\"",
            "tasks.retry.jitter",
            "wobbly",
        );
    }

    #[test]
    fn retry_rejects_an_unknown_key_by_name() {
        let text = read("[tasks.retry]\nattempts = 3").unwrap_err().to_string();
        assert!(text.contains("attempts"), "{text}");
    }

    #[test]
    fn both_sections_load_side_by_side() {
        let toml = "[tasks.scheduling]\nmax-concurrent = 2\n[tasks.retry]\nmax-attempts = 5\n";
        let config = Config::from_str(toml).unwrap();
        assert_eq!(
            TasksSettings::from_config(&config).unwrap().max_concurrent,
            2
        );
        assert_eq!(RetryDefaults::from_config(&config).unwrap().max_attempts, 5);

        let typo =
            Config::from_str("[tasks.scheduling]\nmax-concurent = 2\n[tasks.retry]\n").unwrap();
        assert!(
            TasksSettings::from_config(&typo)
                .unwrap_err()
                .to_string()
                .contains("max-concurent")
        );
        assert!(
            RetryDefaults::from_config(&typo)
                .unwrap_err()
                .to_string()
                .contains("max-concurent")
        );

        let typo = Config::from_str("[tasks.scheduling]\n[tasks.retry]\nattempts = 3\n").unwrap();
        assert!(
            TasksSettings::from_config(&typo)
                .unwrap_err()
                .to_string()
                .contains("attempts")
        );
        assert!(
            RetryDefaults::from_config(&typo)
                .unwrap_err()
                .to_string()
                .contains("attempts")
        );

        let typo = Config::from_str("[tasks]\nretri = true\n").unwrap();
        assert!(
            TasksSettings::from_config(&typo)
                .unwrap_err()
                .to_string()
                .contains("retri")
        );
        assert!(
            RetryDefaults::from_config(&typo)
                .unwrap_err()
                .to_string()
                .contains("retri")
        );
    }

    #[test]
    fn a_bad_duration_does_not_echo_the_value() {
        let text = read("[tasks.retry]\ninitial = \"banana\"")
            .unwrap_err()
            .to_string();
        assert!(text.contains("tasks.retry.initial"), "{text}");
        assert!(!text.contains("banana"), "{text}");
    }

    #[test]
    fn initial_above_max_delay_names_the_key_the_user_set() {
        let text = read("[tasks.retry]\nmax-delay = \"50ms\"")
            .unwrap_err()
            .to_string();
        assert!(text.contains("tasks.retry.max-delay"), "{text}");
        assert!(!text.contains("tasks.retry.initial"), "{text}");

        let text = read("[tasks.retry]\ninitial = \"1h\"")
            .unwrap_err()
            .to_string();
        assert!(text.contains("tasks.retry.initial"), "{text}");
        assert!(!text.contains("tasks.retry.max-delay"), "{text}");
    }
}
