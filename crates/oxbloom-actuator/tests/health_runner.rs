use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oxbloom_actuator::{Health, HealthIndicator, HealthRunner, Status};

struct Fixed(&'static str, Status);

#[async_trait]
impl HealthIndicator for Fixed {
    fn name(&self) -> &str {
        self.0
    }
    async fn health(&self) -> Health {
        Health::new(self.1)
    }
}

struct Slow;

#[async_trait]
impl HealthIndicator for Slow {
    fn name(&self) -> &str {
        "slow"
    }
    async fn health(&self) -> Health {
        tokio::time::sleep(Duration::from_secs(1)).await;
        Health::up()
    }
}

struct Panics;

#[async_trait]
impl HealthIndicator for Panics {
    fn name(&self) -> &str {
        "boom"
    }
    async fn health(&self) -> Health {
        panic!("indicator failed on purpose");
    }
}

fn runner(indicators: Vec<Arc<dyn HealthIndicator>>) -> HealthRunner {
    HealthRunner::new(indicators, Duration::from_millis(50))
}

#[tokio::test]
async fn a_slow_indicator_is_down_with_timeout_error() {
    let (status, parts) = runner(vec![Arc::new(Slow)]).run(None).await;
    assert_eq!(status, Status::Down);
    assert_eq!(parts["slow"].status, Status::Down);
    assert_eq!(parts["slow"].details["error"], "timed out");
}

#[tokio::test]
async fn a_panicking_indicator_is_down_with_panic_error() {
    let (status, parts) = runner(vec![Arc::new(Panics), Arc::new(Fixed("ok", Status::Up))])
        .run(None)
        .await;
    assert_eq!(status, Status::Down);
    assert_eq!(parts["boom"].details["error"], "panicked");
    assert_eq!(parts["ok"].status, Status::Up);
}

#[tokio::test]
async fn worst_status_wins() {
    let (status, parts) = runner(vec![
        Arc::new(Fixed("a", Status::Up)),
        Arc::new(Fixed("b", Status::OutOfService)),
        Arc::new(Fixed("c", Status::Unknown)),
    ])
    .run(None)
    .await;
    assert_eq!(status, Status::OutOfService);
    assert_eq!(parts.len(), 3);
}

#[tokio::test]
async fn only_filters_by_name() {
    let only = vec!["a".to_string()];
    let (status, parts) = runner(vec![
        Arc::new(Fixed("a", Status::Up)),
        Arc::new(Fixed("b", Status::Down)),
    ])
    .run(Some(&only))
    .await;
    assert_eq!(status, Status::Up);
    assert_eq!(parts.keys().collect::<Vec<_>>(), ["a"]);
}

#[tokio::test]
async fn no_indicators_is_up() {
    let (status, parts) = runner(vec![]).run(None).await;
    assert_eq!(status, Status::Up);
    assert!(parts.is_empty());
}

#[tokio::test]
async fn duplicate_names_do_not_mask_a_down_indicator() {
    for order in [[Status::Down, Status::Up], [Status::Up, Status::Down]] {
        let (status, _) = runner(vec![
            Arc::new(Fixed("same", order[0])),
            Arc::new(Fixed("same", order[1])),
        ])
        .run(None)
        .await;
        assert_eq!(status, Status::Down, "{order:?}");
    }
}

#[tokio::test]
async fn duplicate_names_keep_both_entries() {
    let (_, parts) = runner(vec![
        Arc::new(Fixed("same", Status::Up)),
        Arc::new(Fixed("same", Status::Down)),
    ])
    .run(None)
    .await;
    assert_eq!(parts.keys().collect::<Vec<_>>(), ["same", "same#2"]);
    assert_eq!(parts["same"].status, Status::Up);
    assert_eq!(parts["same#2"].status, Status::Down);
}
