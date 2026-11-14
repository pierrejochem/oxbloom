# ADR 0084: Task scheduling lives in its own crate, runs one clock-driven loop per task, and starts after the web server

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** the C9a design spec `2026-11-14-tasks-scheduling-design.md` and the C9a plan `2026-11-14-tasks-scheduling.md`, which are local files under `docs/superpowers` and therefore named here without a link

## Context

An application needs work that runs on a schedule: a cleanup every minute, a report every night. Several decisions shape that feature. Where does the code live, given that C9b (retry, backoff and jitter) will need the same task machinery? How does a method become a task, when an attribute macro on an `impl` block cannot register beans? How does the scheduler measure time, so that tests and clock jumps behave? What happens when a run is still active at the next trigger? When does the scheduler start and stop relative to the web server and the availability state? Which cron implementation, and how are zones and daylight saving time handled?

The crate dependency direction is fixed by [ADR 0033](0033-boot-entry-point-and-dependency-direction.md): `oxbloom-boot` sits above the other crates and wires them through a cargo feature and an auto-configuration ([ADR 0048](0048-contributor-beans.md)). The time source is the `Clock` trait of [ADR 0063](0063-clock-abstraction.md).

## Decision

**Own crate.** The scheduler is the new crate `oxbloom-tasks`, with the triggers, `Task`, the `Scheduling` trait, `Scheduler` and `TasksSettings`. Boot enables it with the feature `tasks`. C9b will reuse the crate for retry, so the task machinery does not belong to boot, and the crate does not depend on boot.

**The `Scheduling` trait is the macro target.** The attribute macro `#[scheduling]` on an inherent `impl` block generates `impl Scheduling for T`, whose `scheduled(self: Arc<Self>) -> Vec<Task>` returns one task per `#[scheduled]` method. An impl macro cannot register beans, because it only sees the block, so the application registers the bean with `.provides(.. -> Arc<dyn Scheduling>)`. `#[controller]` works in the same way: it generates `routes()` and the application registers a `dyn Controller`. The spec first said that the macro registers one `dyn ScheduledTask` bean per method. The user-visible behavior is the same, one task per method, and the plan recorded the change before the macro was written.

**Macros without a feature flag.** The spec said that the macros sit behind a feature. They live in `oxbloom-macros`, which has no feature flags, and `oxbloom-tasks` re-exports them as `scheduling` and `scheduled`, as `#[controller]` and the route macros are available without a flag. This decision amends the spec. The generated code names the crate `oxbloom_tasks`, so the dependency must not be renamed. The literals of the attribute are parsed when `scheduled()` runs, not at compile time, and an invalid one becomes `Task::invalid`, so the startup fails with an error that names the task and the key and never the value.

**One loop per task, driven by the injected clock.** The scheduler spawns one tokio task per scheduled task. Each loop reads the `Clock`, computes the next fire time from the `Trigger`, and sleeps in chunks of at most one second, reading the clock after each chunk. The clock is the only time source, so tests run the scheduler with a controllable clock, and a clock that jumps is noticed within a second. A jump forward gives one run, because the next fire time is always computed from the current time and nothing is caught up. For a fixed rate or fixed delay, a wait is capped at the planned wait and at one period, so a jump backward cannot stall the task for longer than a period. A tokio timer for the whole wait would not see the clock change.

**Skip on overlap.** A task never runs concurrently with itself. A trigger that finds the previous run active, running or waiting for a permit, is skipped, counted in `TaskStatus::skipped` and logged at `warn`. The alternatives, queueing the late trigger or running two copies, let a slow task build a backlog or exhaust the pool, and a missed trigger of a periodic job is rarely worth replaying. A semaphore bounds the runs that execute at once across all tasks (`tasks.scheduling.max-concurrent`). An `Err` or a panic is logged at `error` and the schedule goes on.

**Late phase with a DI order.** The `SchedulerService` has the phase `i32::MAX`, the phase of the web server, which is the highest there is, and its registration has the DI order `i32::MAX`. Boot starts services by phase with a stable sort and stops them in reverse, and the container keeps the registration order, so the scheduler starts after the server and stops before it. Boot fires `ready` after all services started, so the first runs can begin before readiness flips. A task that needs a warm application uses `initial_delay`. Another service with the phase and the order `i32::MAX` would tie with the scheduler, because no higher phase exists.

**The `cron` crate, with two fixes of our own.** The cron implementation is the `cron` crate 0.17, with `chrono-tz` for IANA zones. The `croner` crate was the considered alternative, with the plan to switch if `cron` could not pass the daylight saving and leap-day tests. It did pass, with two additions in `oxbloom-tasks`. First, the `cron` crate looks for the next match on the local clock, so a local time that happens twice would fire twice; Oxbloom skips the second occurrence of a repeated local time. The cost is a gap of about 75 minutes on a fall-back day for expressions that fire every few minutes or seconds. A local time that does not exist does not fire. Second, the crate counts Sunday as day 1, while Spring and Unix cron count Monday as 1 and accept 0 and 7 for Sunday, so Oxbloom translates every day-of-week element into an explicit list of day names before it hands the expression to the crate. Day names are first read as Spring numbers (`MON` is 1, `SUN` is 7, or 0 at the start of a range), so names and numbers can be mixed within a range (`MON-3`) and a named range may end at Sunday (`SAT-SUN`), which the crate's own grammar rejects. Expressions have six fields with seconds first, and the crate evaluates up to the year 2100.

**No backward clamp for cron.** A fixed rate or fixed delay trigger waits for a planned span, so the scheduler can cap the real time it sleeps. A cron trigger waits for a wall-clock instant, and there is no planned span to compare with: if the clock is stepped back, the next matching instant really is later. Capping the sleep of a cron task would make it fire at a time that does not match its expression, and the scheduler has no planned span to cap it with. So a cron task stalls for the span of a backward step. This is documented, and a clamp is a follow-up.

**Further rulings of the slice.**

- `tasks.scheduling.zone` applies to cron tasks without a `zone` of their own. A cron trigger keeps an optional zone and `Scheduler::try_new` fills the configured one, so `Trigger::cron_in_default_zone(x)` and `Trigger::cron(x, Zone::Utc)` are not equal under `PartialEq`.
- `Scheduler::try_new` rejects duplicate task names, because names identify tasks in logs and in `tasks()`.
- `stop` waits for runs up to `tasks.scheduling.shutdown-timeout` and aborts the rest. A dropped `stop` aborts the runs too. A thread blocked in `spawn_blocking` cannot be aborted, so `stop` returns at the deadline and leaks the thread until the work finishes.
- Unknown keys under `tasks` are rejected when the auto-configuration runs; any `tasks.scheduling.enabled` other than `true` switches it off silently, as for the actuator ([ADR 0083](0083-actuator.md)).

## Consequences

Good:

- The task machinery is a separate crate that retry in C9b can build on.
- The same macro shape as `#[controller]`: one impl block, one `provides` line.
- Tests control time through the `Clock`, and clock jumps never cause bursts.
- A slow task cannot build a backlog or run twice at once.
- The first run happens after the server is up, and shutdown stops new work before the server stops.
- Startup errors name the task and the key and never show a value.

Bad:

- A trigger skipped because of an overlap is lost.
- A cron task has no protection against a clock step backward, and a fall-back day has a gap of about 75 minutes for frequent expressions.
- The scheduler can run its first tasks before readiness flips, and ties with another service in the phase `i32::MAX`.
- A blocking run cannot be aborted, and the tokio runtime shutdown still waits for it.
- The generated call syntax on `Arc<Self>` can pick up a method named `clone`, `to_string` or `scheduled`.
- The duration grammar of the attribute differs from the one of the configuration binder.

## Alternatives considered

- Registering one bean per method from the macro: impossible for an impl macro, see above.
- A cargo feature for the macros: rejected, because the other macros have none.
- Tasks inside `oxbloom-boot`: rejected, because C9b needs them without the boot dependency.
- The `croner` crate: kept as the fallback; `cron` passed the tests with the two additions above.
- A tokio interval or one long sleep per trigger: rejected, because it ignores the injected clock and cannot notice a jump.
- Queueing or running overlapping triggers: rejected, see skip on overlap.
- Catching up missed runs after a jump: rejected, a jump gives one run.
