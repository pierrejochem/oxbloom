# ADR 0085: Retry is an async-only attribute over a policy value, with process-wide defaults and its own auto-configuration

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** the C9b design spec `2026-11-14-retry-design.md` and the C9b plan `2026-11-14-retry.md`, which are local files under `docs/superpowers` and therefore named here without a link

## Context

A bean method that calls something unreliable should be able to retry itself, with a growing delay and some randomness, and an operator should be able to set the defaults in configuration. Several decisions shape that feature. Where does a procedural macro that rewrites one method find configuration values, when it has no field on the bean and cannot see the container? Which methods can be retried? What is the default jitter? Does the macro read the receiver, and what does it allow the body to do? Does the configuration belong to the scheduler's auto-configuration or stands alone? How does the macro parse durations?

The task machinery and its crate come from [ADR 0084](0084-task-scheduling.md), and the wiring through a boot feature and an auto-configuration from [ADR 0048](0048-contributor-beans.md).

## Decision

**A policy value and an attribute over it.** `oxbloom-tasks` holds `RetryPolicy`, `Backoff`, `Jitter` and `RetryDefaults`. `RetryPolicy::run(label, op, retry_if)` is the whole behavior and can be called without the macro. `#[retryable(..)]` in `oxbloom-macros` (no feature flag, like `#[scheduled]`) rewrites one method so that its original body becomes a closure that `run` calls. `RetryPolicy::delay(retry_number, unit_random)` is a pure function, and the randomness enters `run` through `RetryPolicy::run_with_random`, which is `#[doc(hidden)] pub` so that tests in other crates can replace it. Sleeping uses `tokio::time::sleep`, so tests with paused time work. Jitter draws from the small `fastrand` crate.

**Process-wide defaults in a `static RwLock<RetryDefaults>`.** A key that the attribute omits is read from `oxbloom_tasks::defaults()` at call time. The spec chose a `OnceLock`, which can be set once; this decision replaces it with a `RwLock`, because `set_defaults` must be able to run again (a second application context in the same process, a test), and `reset_defaults()` exists to restore the built-in values. A macro that rewrites a single method cannot add a field to the bean or reach the container, so a global is the only place the value can live. The rules that follow:

- each context's `RetryAutoConfiguration` overwrites the values;
- a context without that auto-configuration keeps what the process stored before (the built-in values if nothing was);
- a context whose retry keys are invalid fails to start and leaves the defaults as they were;
- the values stay after the run ends;
- `oxbloom-test` has no reset. The spec said it resets the defaults between contexts. That is dropped: the test crate cannot know which contexts a test starts, and a hidden reset would hide the global from the author. A test that sets retry keys calls `reset_defaults()` itself.

A key that the attribute writes is fixed at compile time and is not affected by the defaults.

**Async only.** The method must be `async fn` and return `Result<T, E>` with `E: Display`. Retry of a plain `fn` would sleep on the calling thread or need `spawn_blocking`, which the scheduler already offers for tasks but which is wrong for an arbitrary method, and retry on a result value instead of an `Err` is a different feature. The error text is logged with `Display`, so the bound is the same as for a scheduled task.

**The receiver rule.** The receiver is `&self` or absent. `&mut self`, `self` and `mut` arguments are compile errors. The body runs again for every attempt, so it may borrow the receiver and the arguments but not consume them; an argument that the body must own is cloned in the body. A free `async fn` is accepted too, because an attribute on a function cannot see whether the function sits in a context with beans, and rejecting it would only add an arbitrary rule. A `return` in the body ends that attempt and `?` works as usual.

**Jitter is `full` by default.** Spring's `@Retryable` has no jitter unless it is asked for. Many callers that fail together, such as a service whose dependency went down, otherwise retry together and at the same moments; full jitter (a random delay between zero and the computed delay) spreads them best. The cost is that a delay can be near zero, and that exact delays need `jitter = "none"`. This differs from Spring and is documented in the guide.

**A separate `RetryAutoConfiguration`.** It reads the `tasks.retry.*` keys, has no conditions and stores them with `set_defaults`. Folding it into `TasksAutoConfiguration` would make the retry keys depend on the scheduler: with no `dyn Scheduling` bean or `tasks.scheduling.enabled = false`, the group is skipped, and `#[retryable]` on an ordinary bean method would silently ignore the configuration. Separate groups also keep the unknown-key check of each section independent; both readers accept the keys of the other section, so a typo in either is rejected whichever group runs. The cost is one more registration and a metadata list of six keys beside the five of the scheduler, eleven `tasks.*` keys in all.

**The macro keeps its own copy of the duration grammar.** A procedural macro runs at compile time and cannot call `parse_duration` of `oxbloom-tasks`: `oxbloom-tasks` re-exports the macros, so a normal dependency of `oxbloom-macros` on `oxbloom-tasks` would be a cycle. The macro parses a whole number with `ms`, `s`, `m`, `h` or `d` itself, and a test in `oxbloom-macros` (which has `oxbloom-tasks` as a dev-dependency) checks the copy against `parse_duration`. An attribute duration, unlike a `#[scheduled]` literal, is checked at compile time, so a bad value is a compile error that names the method.

**Logging and edge rules.** Events use the target `oxbloom::tasks`: a `warn` event per retry and one `error` event when the attempts run out. With `max_attempts = 1` there is no retry, so the first failure logs the `error` event. An error that `retry_if` rejects returns at once and logs nothing, because it is the caller's decision, not a failure of the retry. A panic is never retried and is not caught.

**Combined with `#[scheduled]`.** Both attributes can sit on one method of a `#[scheduling]` impl, in either order. The scheduled run calls the retrying method, so all attempts happen inside one run, and the run counts as the same run for the overlap rule: a trigger that fires during the retries is skipped.

## Consequences

Good:

- One small policy type that works with or without the macro, with a pure delay function that is easy to test.
- The retry keys apply to every `#[retryable]` method, with or without a scheduler.
- Defaults can change at run time (a new context, a test) and be restored.
- Errors in the attribute surface at compile time and name the method.

Bad:

- The defaults are global: two contexts in one process share the values stored last, and they outlive the application. Tests that touch them must restore them.
- The macro duplicates the duration grammar, held together by a test.
- Only `async fn` with a `Result` can be retried, the body cannot consume its arguments, and `&mut self` is not supported.
- Full jitter as the default differs from Spring, and it surprises someone who expects exact delays.
- A scheduled task that exhausts its attempts logs twice, once for the retry and once for the failed run.

## Alternatives considered

- A `OnceLock` for the defaults, as in the spec: rejected, because it cannot be set twice, so a second context or a test could not change it.
- A defaults field on each bean, passed by the macro: impossible for an attribute on one method of an impl block that has no fields of its own to extend.
- Folding the configuration into `TasksAutoConfiguration`: rejected, see above.
- Retry of plain `fn` methods, `&mut self` receivers and bodies that consume their arguments: rejected for the reasons above; they stay out of scope.
- A `recover` fallback method like Spring's `@Recover`, a circuit breaker and rate limiting: out of scope.
- No jitter by default, as in Spring: rejected, see above.
