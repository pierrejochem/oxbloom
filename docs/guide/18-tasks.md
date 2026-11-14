# Task scheduling

Oxbloom runs methods of your beans on a schedule: at a fixed rate, with a fixed delay between runs, or at the times of a cron expression. The crate `oxbloom-tasks` holds the triggers, the `Scheduler` and the macros, and the feature `tasks` of `oxbloom-boot` wires the scheduler into the application lifecycle. In an application you add the feature, write an impl block with `#[scheduling]`, and register the bean.

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["tasks"] }
oxbloom-tasks = "0.1"
```

The design is in the C9a spec, the plan is the C9a plan (both are local files under `docs/superpowers`, so this page names them without a link), the decisions are in [ADR 0084](../adr/0084-task-scheduling.md), and every public item has an example in the [API reference](../API.md#413-oxbloom-tasks). The example `examples/todos` has a scheduled cleanup task.

## Declaring tasks

Put `#[scheduling]` on an inherent `impl` block and `#[scheduled(..)]` on each method that should run on a schedule. The macro generates `impl Scheduling for T`, whose `scheduled()` returns one `Task` per annotated method, in declaration order. The application registers the bean and exposes it as a `dyn Scheduling`, the same way a controller is exposed as a `dyn Controller`:

```rust
use std::sync::Arc;
use oxbloom_tasks::{Scheduling, scheduling};

pub struct Reports;

#[scheduling]
impl Reports {
    // Every night at 03:00 in Amsterdam.
    #[scheduled(cron = "0 0 3 * * *", zone = "Europe/Amsterdam")]
    async fn nightly(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    // Every 30 seconds, first after 5 seconds, under the task name `refresh`.
    #[scheduled(fixed_rate = "30s", initial_delay = "5s", name = "refresh")]
    fn refresh(&self) {}
}

// In the application builder:
//     .configure(|b| {
//         b.instance(Reports)
//             .provides(|r: Arc<Reports>| -> Arc<dyn Scheduling> { r })
//     })
```

The macro cannot register the bean itself, because an impl macro only sees the impl block. `#[controller]` works the same way. With the feature `tasks`, the `TasksAutoConfiguration` collects every `dyn Scheduling` bean, builds one `Scheduler` from all their tasks and runs it as a boot service.

The keys of `#[scheduled(..)]` are string literals:

| Key | Meaning |
|---|---|
| `cron = ".."` | A six-field cron expression, seconds first. |
| `fixed_rate = ".."` | A period, for example `30s`. |
| `fixed_delay = ".."` | A delay after the end of each run, for example `30s`. |
| `initial_delay = ".."` | Postpones the first run. |
| `zone = ".."` | The IANA zone of a `cron` trigger; allowed with `cron` only. |
| `name = ".."` | The task name in logs and in `Scheduler::tasks()`; the default is `Type::method`. |

A method has exactly one trigger. It takes only `&self` and returns `()` or `Result<(), E>` where `E: Display`; an `Err` is a failed run and is logged with the `to_string()` of the error. An `async fn` is awaited on the runtime. A plain `fn` runs in `tokio::task::spawn_blocking`, so blocking work does not stall the other tasks.

Mistakes in the attribute, such as two triggers, none, an unknown or repeated key, `zone` without `cron`, an empty `name`, a method with arguments, a generic method or a wrong return type, are compile errors that name the method. A literal that is well formed but not valid, such as a cron expression with five fields, an unknown zone or the duration `soon`, cannot be seen by the compiler, because the literals are parsed when `scheduled()` runs. The startup then fails with an error that names the task and the key (`cron`, `zone`, `fixed_rate`, `fixed_delay` or `initial_delay`) and never shows the value. A zero `fixed_rate` or `fixed_delay` is invalid too.

Task names must be unique across all `dyn Scheduling` beans. Two tasks with the same name fail the startup with an error that names the task and the key `name`. Use `name = ".."` when two impls would produce the same `Type::method`.

## The three triggers

A fixed rate runs every period, measured from the moment the trigger fired, not from the moment the run actually started (a run that waited for a concurrency permit still counts from the fire time). A fixed delay runs a delay after the previous run ended. A cron trigger runs at the instants that match the expression.

Without `initial_delay`, the first run of a fixed rate or fixed delay task is at the moment the scheduler starts. With an `initial_delay` the first run is the scheduler start plus the delay. For a cron task the delay postpones the first evaluation: the first run is the first matching time after the start plus the delay.

### Cron expressions

An expression has six fields: second, minute, hour, day of month, month and day of week. Five-field Unix expressions, seven-field expressions with a year and macros such as `@daily` are rejected.

```text
0 0 9 * * 1-5        09:00:00 on Monday to Friday
0 */15 * * * *       every 15 minutes, on the minute
30 0 0 1 * *         00:00:30 on the first day of every month
```

Days of the week follow Spring: `1` is Monday through `6` for Saturday, and both `0` and `7` are Sunday. The names `SUN` to `SAT` work in any case. Numbers are translated before the expression reaches the `cron` crate, which counts Sunday as 1, so `1-5` means Monday to Friday, `5-7` means Friday to Sunday, and a step such as `*/2` counts from Monday. A number of 8 or more is an error. Names are passed through untouched.

A single range that mixes a name and a number, such as `MON-3` or `1-FRI`, is not translated, and the `cron` crate may read the digit in its own numbering. Use names or numbers in a range, never both.

The `cron` crate evaluates dates up to the year 2100. After that, a cron task has no next fire time, which is logged at `info` and ends that task's loop.

### Zones and daylight saving time

A cron expression is evaluated in a zone. The key `tasks.scheduling.zone` (default `UTC`) applies to every cron task without a `zone` of its own, and `zone = "Europe/Amsterdam"` on the attribute sets one for that task. Zone names are `UTC` or IANA names such as `America/New_York`. The two forms are not equal under `PartialEq` on `Trigger`: `Trigger::cron_in_default_zone("0 0 3 * * *")` leaves the zone open for the scheduler to fill in, while `Trigger::cron("0 0 3 * * *", Zone::Utc)` fixes it, even if the configured default zone is UTC as well.

Daylight saving time follows these rules:

- A local time that does not exist, because the clocks jumped forward over it, does not fire. A task at 02:30 skips the day the clock goes from 02:00 to 03:00.
- A local time that happens twice, because the clocks went back, fires once, at its first occurrence.
- An expression that matches every few minutes or seconds has a gap of about 75 minutes on a fall-back day. This comes from two things together: the `cron` crate looks for the next match on the local clock, and Oxbloom skips the second occurrence of a repeated local time so that it fires only once.

### Durations

The attribute literals and `parse_duration` accept a whole number with one unit: `ms`, `s`, `m`, `h` or `d`, for example `250ms`, `30s` or `2h`. A fraction, a sign, a missing unit or an unknown unit is an error. Zero is a valid initial delay.

The configuration binder reads durations too, for `tasks.scheduling.shutdown-timeout` and `tasks.scheduling.initial-delay`, and it accepts more forms, such as `1.5s`. The two grammars differ, so a value that works in `application.toml` may not work in an attribute. A single configuration grammar is a follow-up.

## Overlap and failures

A task never runs concurrently with itself. When a trigger fires while the previous run of the same task is still active, either running or waiting for a concurrency permit, no new run starts. The skip is logged at `warn`, counted in `TaskStatus::skipped`, and the next trigger is planned as usual. Skipped triggers are not made up later.

A run that returns `Err` is logged at `error` with the task name and the message, and the schedule continues. A run that panics is logged at `error` too, with the panic message when it is a string, and the schedule continues. A panic inside a plain `fn` is carried out of `spawn_blocking` and recorded as a panic of the run.

## Properties

| Key | Default | Meaning |
|---|---|---|
| `tasks.scheduling.enabled` | `true` | Only `true` (or a missing key) switches the group on. |
| `tasks.scheduling.max-concurrent` | `8` | How many runs execute at the same time across all tasks; at least 1. |
| `tasks.scheduling.shutdown-timeout` | `10s` | How long shutdown waits for runs in flight before it aborts them. |
| `tasks.scheduling.zone` | `UTC` | The zone of cron tasks without a `zone` of their own. |
| `tasks.scheduling.initial-delay` | `0s` | The initial delay of tasks without an `initial_delay` of their own. |

```toml
[tasks.scheduling]
max-concurrent   = 4
shutdown-timeout = "30s"
zone             = "Europe/Amsterdam"
initial-delay    = "10s"
```

An unknown key directly under `tasks` or under `tasks.scheduling`, a value of the wrong type, a bad duration, a `max-concurrent` of 0 and an unknown zone fail the startup with an error that names the key and never shows the value. This check runs only when the auto-configuration runs. With `tasks.scheduling.enabled = false`, or with no `dyn Scheduling` bean, the group is skipped and a misspelled key such as `tasks.typo` is accepted silently.

Any value of `tasks.scheduling.enabled` other than `true` switches the group off without an error, including a typo such as `ture`. The conditions report names the reason for the skip.

## Lifecycle

`TasksAutoConfiguration` registers the `TasksSettings`, the `Scheduler` (unless the application registered one) and a `SchedulerService`. The scheduler reads time only from the `dyn Clock` bean, or the system clock when there is none, so a test can drive it with a controllable clock.

The service is a boot `Service` in the same phase as the web server, `i32::MAX`, and its registration has the DI order `i32::MAX`, so among the services of that phase it comes last. It starts after the web server and every other service, and it stops first, before the server. Boot fires the `ready` event after all services started, so the first runs can happen before the availability state flips to accepting traffic. A task that needs the application to be warm should use `initial_delay`, or the key `tasks.scheduling.initial-delay` for all tasks.

The auto-configuration runs only with at least one `dyn Scheduling` bean. Without one there is no scheduler and no service.

### Shutdown

When boot stops the scheduler, no trigger fires any more. Runs that are in flight get up to `tasks.scheduling.shutdown-timeout` to finish. Runs still going at the deadline are aborted, and each of them is named in a `warn` event. Runs that wait for a permit give up instead of starting. If boot abandons the `stop` at its own deadline first, or the `stop` future is dropped, the runs in flight are aborted as well.

An abort can only cancel a future at an await point. A plain `fn` that is blocked in `spawn_blocking`, and an `async fn` that blocks its thread, cannot be aborted. `stop` still returns at the deadline and does not wait for it, but the thread is leaked until the work finishes, and the shutdown of the tokio runtime itself still waits for blocking threads. Keep blocking work short, or make it check for cancellation itself.

## Clock behavior

The scheduler sleeps in chunks of at most one second and reads the clock again after each chunk, so a clock that jumps is noticed within a second.

- A jump forward gives one run, never a burst. After a run the next fire time is computed from the current time, and missed fire times are not caught up.
- A jump backward delays fixed-rate and fixed-delay tasks, but a wait is never longer than the period, so the delay is clamped to one period.
- Cron tasks have no such clamp. If the clock is stepped back, for example by NTP, a cron task that fires every second stalls for that span of time, because the next matching instant is in the future of the new time.

## Logging and inspection

All events use the target `oxbloom::tasks`:

| Level | Event |
|---|---|
| `info` | `task scheduler started` with the number of tasks, and `task scheduler stopped`. |
| `info` | `scheduled task has no next fire time`, for a cron task past the year 2100. |
| `warn` | `scheduled task skipped: the previous run is still active`, with the task name. |
| `warn` | `scheduled task run abandoned at shutdown`, with the task name. |
| `error` | `scheduled task failed` or `scheduled task panicked`, with the task name and the `error` field. |
| `debug` | `scheduled task ran`, with the task name, the duration in milliseconds and the outcome. |

`Scheduler::tasks()` returns a snapshot of every task, in the order the tasks were given. Each `TaskStatus` has the name, the trigger as a string such as `fixed rate 5s` or `cron 0 0 3 * * * (UTC)`, the last start and end time, the last `Outcome` (`Ok`, `Failed`, `Panicked` or `Skipped`) and the number of skipped triggers. `Scheduler::is_running()` is true between `start` and `stop`.

## The todos example

`examples/todos` has a `TodoCleanup` bean in `cleanup.rs`. Its method `cleanup` is `#[scheduled(fixed_delay = "1m")]` and removes todos that have been done for longer than `todos.cleanup.max-age` (default `24h`). The bean is registered with `.provides(.. -> Arc<dyn Scheduling>)`, the application enables the feature `tasks`, and the task is named `TodoCleanup::cleanup`. Because a fixed delay without a history runs at the start, the first cleanup happens when the scheduler starts, and then one minute after the end of each run.

## Limits and deferred work

- A method named `clone`, `to_string` or `scheduled` can resolve to a method of `Arc` or of a trait instead of your method, because the generated call uses method syntax on an `Arc<Self>`.
- A `#[cfg]`-gated `#[scheduled]` method and an `unsafe fn` are not supported.
- The return type is checked by shape: `()` or a path that ends in `Result<..>`. The compiler then checks `Display` of the error.
- When one method of an impl has an error, the whole impl is left out of the output, so the compiler can report extra "no method found" errors next to the real one.
- A task that panics or returns an error does not stop its schedule, and there is no retry or backoff. Retry is the next slice, C9b.
- Missed runs are not caught up, there is no persistence, and several application instances all run every task; there is no distributed lock.
- Tasks cannot be added or removed at run time, and the concurrency limit is global, not per task.
- There is no actuator endpoint for the tasks yet; use `Scheduler::tasks()`.
- The macros live in `oxbloom-macros` and are re-exported by `oxbloom-tasks` without a cargo feature.
