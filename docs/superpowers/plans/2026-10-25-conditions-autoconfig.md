# Conditions and auto-configuration (C4a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add conditions to `oxbloom-di` (on any registration and on groups), an `Environment` for property conditions, a condition report, and auto-configuration in `oxbloom-boot` (explicit lists, `after`/`before` ordering, `exclude`, back-off), with the macro options `#[auto_configuration]` and `#[bean(on_..)]`.

**Architecture:** `build()` gains a filtering step after profile filtering: it walks the registrations in order, tracks which types the kept ones provide, and evaluates each registration's own conditions and its enclosing groups' conditions once, recording a report. Boot appends the sorted auto-configurations as groups after the user's wiring so user beans always win. All of it runs before provider resolution, so the recursion path and its stack minima do not change.

**Tech Stack:** Rust 2024; `syn`/`quote` (already in `oxbloom-macros`); `trybuild` (already a dev-dependency). No new dependency.

**Spec:** `docs/superpowers/specs/2026-10-25-conditions-autoconfig-design.md`

## Spec amendments decided while planning

Task 8 writes these into the spec.

1. Conditions of one registration are evaluated in order and evaluation stops at the first condition that does not match (short-circuit), so a skipped registration has exactly one `matched = false` record for its own conditions; conditions after it are not evaluated and have no record. A group's conditions follow the same rule.
2. The macro `#[auto_configuration]` generates paths into `::oxbloom_boot` (the trait lives there); `oxbloom-boot` re-exports the macro so `use oxbloom_boot::auto_configuration;` works. The crate using it must be able to name `oxbloom_boot` and `oxbloom_di`, as the other macros require for `oxbloom_di`.
3. Boot registers `Config` as the container environment before the user's `configure` closures, so a user `environment(..)` call can replace it.
4. A cycle in auto-configuration order is reported through a crate-private error type that the C3b analyzer family matches with `find_cause`.

## Global Constraints

- Crates touched: `crates/oxbloom-di`, `crates/oxbloom-config` (feature `di`), `crates/oxbloom-macros`, `crates/oxbloom-boot`, `examples/hello`, docs. Workspace package fields via `.workspace = true`. No new dependency.
- Conditions: modifier on the last registration; AND semantics; evaluated once in `build()` after profile filtering and before provider resolution, in registration order, against the kept registrations before the one being decided (never itself, never later ones). A skipped registration is invisible (not a candidate, no `Duplicate`, dependants get `Missing`, provider and hooks never run). Profile-skipped registrations do not count for `on_bean`/`on_missing_bean`.
- Groups: `group(name, conditions, f)`; conditions evaluated once at the group's first registration against the registrations kept before it; a failing group skips all of its registrations (including nested groups); nested group conditions are ANDed; an empty group is a no-op.
- `ConditionOutcome` reasons and report text are fixed text built from type names, condition names and key names: never configuration values. `on_property_value` records `property "<key>" does not have the expected value` and never either value.
- `Environment::property(key)`: scalar leaf text, `None` for absent keys, tables and lists. Without an environment every property is absent.
- Report: `ConditionReport::records() -> &[ConditionRecord]` in evaluation order; `ConditionRecord { registration, condition, matched, reason }` with accessors `registration() -> &str`, `condition() -> &str`, `matched() -> bool`, `reason() -> &str`; group records use the group name as registration. Console format exactly as in the spec ("CONDITIONS EVALUATION REPORT", sections `Matched:` and `Skipped:`, lines `  <registration>: <condition> - <reason>`, `  (none)` for an empty section, blank line between sections, no trailing newline).
- Evaluation cost is linear: `has_bean` uses an index (a set of the type ids provided by kept registrations) updated while walking.
- Stack: the ignored depth probes in `crates/oxbloom-di/tests/depth.rs` stay at 206 KB (closures), 277 KB (`#[component]`), 230 KB (`#[bean]`), 194 KB (`.bean`) in debug; `DiError` stays at most 56 bytes (existing guard test). Re-run the probes in Task 1 and in the final task.
- Ordering of auto-configurations: `A::after() = [B]` and `B::before() = [A]` both mean B before A; stable topological sort, ties in list order; unknown references ignored; cycle is a `Container`-phase `StartupError` with fixed text naming the types; `exclude::<A>()` drops a configuration, no-op if absent and ordering stays valid.
- `oxbloom.main.conditions-report` is `off` (default) or `console`; anything else is a `Config`-phase `StartupError` without the value; in `console` mode the report is printed to the output target after the build, also when the build failed, before the runners or before the failure report.
- Every public item has a runnable doc example (`AGENT.md`). Brand: "Oxbloom" in prose, "oxbloom" in code. Persisted docs use plain full sentences.
- The repo is not a git repository: no commit steps. Never set `CARGO_TARGET_DIR` or `--target-dir`; `/tmp` is a RAM-backed tmpfs shared with the build target: no new target dirs. Delete any probe crate or file you create.
- Done gate: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, each also with `-p oxbloom-boot --features web`, `-p oxbloom-boot --features "web security"`, `-p oxbloom-web --features di`, `-p oxbloom-config --features di`, `-p oxbloom-security --all-features`; `cargo build -p hello`; `graphify update .`.

## Review Focus

- `on_bean::<Later>()` placed before `Later` does not match and the report says so (Task 1).
- A class-level condition that fails never runs its siblings' providers, and other groups are unaffected (Task 2, Task 6).
- `exclude` of a configuration that others list in `after`/`before` keeps ordering valid (Task 6).
- `on_property_value` is an exact comparison: a value that contains the expected text as a substring does not match (Task 1, Task 3).
- The report and evaluation stay linear for 1000 registrations (Task 1 test with a generous time bound plus the index design).

---

### Task 1: `oxbloom-di`: entry conditions, `Environment`, report

**Files:**
- Create: `crates/oxbloom-di/src/condition.rs` (types, evaluation), `crates/oxbloom-di/tests/conditions.rs`
- Modify: `crates/oxbloom-di/src/entry.rs` (conditions on `Entry`), `crates/oxbloom-di/src/container.rs` (builder methods, `build`, `build_with_report`, `Container::condition_report`), `crates/oxbloom-di/src/lib.rs` (re-exports)

**Interfaces:**
- Produces: `pub trait Environment: Send + Sync { fn property(&self, key: &str) -> Option<String>; }`; `pub struct ConditionContext<'a>` with `has_bean::<T: ?Sized + 'static>() -> bool`, `property(&self, key: &str) -> Option<String>`, `active_profiles(&self) -> &[String]`, `registration(&self) -> &str`; `pub struct ConditionOutcome` with `matched()` and `skipped(reason: impl Into<String>)`; `ContainerBuilder::{condition(name: &'static str, f: impl Fn(&ConditionContext) -> ConditionOutcome + Send + Sync + 'static), on_bean::<T: ?Sized + 'static>(), on_missing_bean::<T: ?Sized + 'static>(), on_property(key: &str), on_property_value(key: &str, value: &str), on_property_value_or_missing(key: &str, value: &str), environment(impl Environment + 'static)}` (each modifier applies to the last registration and records `Invalid` when there is none); `ContainerBuilder::build_with_report(self) -> (Result<Container, DiError>, ConditionReport)`; `Container::condition_report(&self) -> &ConditionReport`; `ConditionReport` (`records()`, `Display`) and `ConditionRecord` as in Global Constraints. The conditions field of `Entry` is a `Vec` of boxed conditions with their names; the same condition representation is reused by `Conditions` in Task 2.

- [ ] **Step 1: Write the failing tests** in `tests/conditions.rs`: `user_bean_first_makes_default_skip` (instance registered first, `.bean(default).on_missing_bean::<Server>()` skipped), `default_first_then_user_bean_is_unaffected` (condition sees only earlier registrations), `on_bean_requires_earlier_provider`, `on_bean_for_later_registration_does_not_match_and_report_says_so` (Review Focus 1), `conditions_and_together` (one failing of two skips), `short_circuit_records_only_up_to_first_failure`, `trait_provision_counts` (`provides` makes `on_missing_bean::<dyn T>()` fail), `skipped_entry_is_invisible` (no `Duplicate` when the duplicate is condition-skipped; dependant gets `Missing`; provider counter stays 0), `profile_skipped_entry_does_not_count_for_on_bean`, `condition_before_registration_is_invalid`, `on_property_present_and_absent`, `on_property_value_exact_match_and_substring_does_not_match` (Review Focus 4), `on_property_value_or_missing_matches_absent`, `without_environment_every_property_is_absent`, `custom_condition_gets_context` (profiles, registration label), `panicking_condition_aborts_build`, `report_records_in_order_with_value_free_reasons` (marker value in the environment absent from every reason and from `Display`), `build_with_report_returns_report_on_failure` (a `Missing` build error still has records), `container_condition_report_after_success`, `evaluation_is_linear` (1000 registrations each with `on_missing_bean` of a distinct type completes quickly: assert under 2 s on a debug build). Add a unit test in `condition.rs` for the exact `Display` format (sections, `(none)`).
- [ ] **Step 2: Run** `cargo test -p oxbloom-di --test conditions`. Expected: compile errors.
- [ ] **Step 3: Implement** the types and builder methods; evaluation in `build_with_report`: profile filtering first, then a walk that maintains a `HashSet<TypeId>` of provided keys (concrete key of each kept entry plus its `provided` keys) and evaluates entry conditions in order with short-circuit; skipped entries are dropped before `resolve::build`; the report is built during the walk. `build()` delegates to `build_with_report`. `Container` stores the report.
- [ ] **Step 4: Doc examples** for every public item (success and the skipped case). Re-run the ignored depth probes (`cargo test -p oxbloom-di --test depth -- --ignored` bisecting as in earlier slices) and confirm 206 / 277 / 230 / 194 KB debug; run `cargo test -p oxbloom-di` and `-p oxbloom-web --features di`, `-p oxbloom-config --features di`, `-p oxbloom-security --all-features`, `-p oxbloom-boot`. Expected: PASS.

### Task 2: `oxbloom-di`: `Conditions` and `group`

**Files:**
- Modify: `crates/oxbloom-di/src/condition.rs`, `crates/oxbloom-di/src/container.rs`, `crates/oxbloom-di/src/entry.rs`, `crates/oxbloom-di/src/lib.rs`
- Test: `crates/oxbloom-di/tests/groups.rs`

**Interfaces:**
- Consumes: Task 1 condition representation and evaluation walk.
- Produces: `#[derive(Default)] pub struct Conditions` with `new()`, `condition(name, f)`, `on_bean::<T>()`, `on_missing_bean::<T>()`, `on_property(key)`, `on_property_value(key, value)`, `on_property_value_or_missing(key, value)` (all consuming `self -> Self`); `ContainerBuilder::group(name: &'static str, conditions: Conditions, f: impl FnOnce(ContainerBuilder) -> ContainerBuilder) -> Self`. Each `Entry` records the id of its innermost group; groups form a tree stored on the builder; evaluation decides a group at the first kept-walk visit of one of its entries (parents first), memoizes the result and its report record (registration = group name).

- [ ] **Step 1: Write the failing tests** in `tests/groups.rs`: `class_level_on_missing_bean_does_not_hide_siblings` (group `on_missing_bean::<Foo>` whose first bean provides `Foo` and second needs it: both kept), `failing_group_skips_all_its_registrations_and_providers_never_run` (counters), `other_groups_unaffected`, `nested_groups_and_together`, `nested_failing_inner_keeps_outer_siblings`, `empty_group_is_noop` (no record), `group_conditions_see_only_registrations_before_the_group`, `entry_conditions_inside_group_are_anded_with_group`, `group_record_uses_group_name`, `group_with_profile_skipped_first_entry_still_decided_once` (profile-skipped first entry: the group is decided at its first kept entry), `modifier_after_group_applies_to_last_registration_of_group`.
- [ ] **Step 2: Run** `cargo test -p oxbloom-di --test groups`. Expected: compile errors.
- [ ] **Step 3: Implement** `Conditions`, `group` (a builder-level stack of group ids is pushed before running `f` and popped after, so nested calls and macros that call `group` work), evaluation of groups in the walk.
- [ ] **Step 4: Doc examples** for `Conditions` and `group`. Re-run the depth probes (expect unchanged) and the DI suites. Expected: PASS.

### Task 3: `oxbloom-config`: `Environment for Config`

**Files:**
- Modify: `crates/oxbloom-config/src/di.rs`
- Test: `crates/oxbloom-config/tests/environment.rs` (gated on feature `di`)

**Interfaces:**
- Consumes: `oxbloom_di::Environment` (Task 1).
- Produces: `impl oxbloom_di::Environment for Config`: `property(key)` returns the scalar leaf's text (strings as is; integers, floats and booleans in their TOML text form) for existing scalar keys, `None` for absent keys, tables and lists; lookup uses the same canonical key rules as `Config::get`.

- [ ] **Step 1: Write the failing tests**: `string_integer_bool_float_scalars`, `absent_key_is_none`, `table_and_list_are_none`, `relaxed_key_forms` (`SERVER.port`, `server-port` canonicalization as `get` does), `works_with_on_property_value_in_a_build` (a real `ContainerBuilder` with `.environment(config)`), `secret_value_compares_plain_and_is_not_in_report` (marker in a `Secret`-like key: `on_property_value` match works and the marker is absent from the report `Display`).
- [ ] **Step 2: Run** `cargo test -p oxbloom-config --features di --test environment`. Expected: FAIL.
- [ ] **Step 3: Implement** the impl using the existing node accessors.
- [ ] **Step 4: Doc example** on the impl. Run `cargo test -p oxbloom-config --features di`. Expected: PASS.

### Task 4: `oxbloom-macros`: `#[bean(on_..)]` options

**Files:**
- Modify: `crates/oxbloom-macros/src/configuration.rs`, `crates/oxbloom-macros/src/options.rs`, `crates/oxbloom-macros/src/lib.rs` (docs)
- Test: `crates/oxbloom-di/tests/configuration.rs` (new tests), `crates/oxbloom-di/tests/ui/configuration/*.rs` and `.stderr` (new trybuild cases)

**Interfaces:**
- Consumes: Task 1 builder modifiers.
- Produces: `#[bean(on_bean(T), on_missing_bean, on_missing_bean(T), on_property("key"), on_property("key", "value"))]` (several allowed, ANDed, any order relative to the other bean options). They emit `.on_bean::<T>()`, `.on_missing_bean::<T>()` (no argument: the method's return type; for `fallible` beans the `Ok` type), `.on_property(..)`, `.on_property_value(..)` after the registration, with the same hygiene as existing options (`__oxbloom_` generated names).

- [ ] **Step 1: Write the failing tests**: `bean_on_missing_bean_without_argument_uses_return_type`, `bean_on_missing_bean_with_explicit_type`, `bean_on_bean_requires_earlier_registration`, `bean_on_property_forms` (key only, key and value), `bean_conditions_and_with_each_other`, `bean_condition_with_lazy_primary_order_profile` (combination), `fallible_bean_on_missing_bean_uses_ok_type`; trybuild: `on_property_empty_key`, `on_property_three_args`, `on_bean_without_argument`, `on_missing_bean_non_type_argument`, `unknown_condition_option_lists_valid_options` (unknown-option message lists the new options).
- [ ] **Step 2: Run** `cargo test -p oxbloom-di --test configuration` and the ui test. Expected: FAIL.
- [ ] **Step 3: Implement** the option parsing and emission. Keep generated code lean (no stack impact: conditions are evaluated at build, not in the bean adapter).
- [ ] **Step 4: Docs** on the `#[bean]` macro (examples that compile). Re-run the depth probes for `#[bean]` (expect 230 / 321 KB wide) and `cargo test -p oxbloom-di -p oxbloom-macros`. Expected: PASS.

### Task 5: `AutoConfiguration` and `#[auto_configuration]`

**Files:**
- Create: `crates/oxbloom-boot/src/autoconfig.rs` (trait, `ConfigRef`, `AutoConfigurations` data type only; ordering and registration come in Task 6)
- Modify: `crates/oxbloom-macros/src/configuration.rs` (shared parsing) and `crates/oxbloom-macros/src/lib.rs` (new attribute), `crates/oxbloom-boot/src/lib.rs` (re-export macro and types)
- Test: `crates/oxbloom-boot/tests/autoconfig_macro.rs`, `crates/oxbloom-boot/tests/ui/autoconfig/*.rs` + `.stderr`

**Interfaces:**
- Produces: `pub trait AutoConfiguration: oxbloom_di::Configuration { fn conditions() -> Conditions { Conditions::new() } fn after() -> Vec<ConfigRef> { Vec::new() } fn before() -> Vec<ConfigRef> { Vec::new() } }`; `#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub struct ConfigRef` with `ConfigRef::of::<T: 'static>()`, `name() -> &'static str`; `AutoConfigurations` with `new()`, `add::<A: AutoConfiguration>()`, `extend(other)`, `len()`, `is_empty()` (descriptors keep `ConfigRef`, `A::after()`, `A::before()`, `A::conditions` and `A::register` as function pointers). `#[auto_configuration(after(A, B), before(C), on_bean(T), on_missing_bean(T), on_property("key"), on_property("key", "value"))]` on an inherent non-generic impl: implies `#[configuration]` (same bean parsing, including the new `#[bean(on_..)]` options) and generates `impl ::oxbloom_boot::AutoConfiguration for Type` (class-level conditions via `Conditions`, ordering via `ConfigRef::of`). `oxbloom_boot` re-exports `auto_configuration`, `Conditions` and the DI `Configuration` items needed by users of the macro.

- [ ] **Step 1: Write the failing tests**: `macro_generates_configuration_and_auto_configuration` (beans register through `AutoConfigurations`+manual `ContainerBuilder::group`, see Task 6 for the full flow; here call `A::register` and `A::conditions()` directly), `after_and_before_lists`, `class_conditions_all_forms`, `no_options_is_valid`, `bean_options_still_work_inside`; trybuild: `unknown_option`, `after_non_type`, `on_property_empty`, `generic_impl_rejected`, `trait_impl_rejected`, `missing_oxbloom_boot_path_hint` is documented not tested.
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --test autoconfig_macro`. Expected: FAIL.
- [ ] **Step 3: Implement** the trait, data types and the macro (reuse the existing `#[configuration]` expansion; add the trait impl; options go through the shared option parser from Task 4).
- [ ] **Step 4: Docs/examples** for every public item. Run `cargo test -p oxbloom-boot -p oxbloom-macros` (and `--features web`). Expected: PASS.

### Task 6: Boot: ordering, `exclude`, registration, cycle error and analyzer

**Files:**
- Modify: `crates/oxbloom-boot/src/autoconfig.rs` (ordering), `crates/oxbloom-boot/src/app.rs` (builder methods, run step), `crates/oxbloom-boot/src/analyzers/mod.rs`, create `crates/oxbloom-boot/src/analyzers/autoconfig.rs`
- Test: `crates/oxbloom-boot/tests/autoconfig.rs`

**Interfaces:**
- Consumes: Tasks 1, 2, 3, 5.
- Produces: `OxbloomApplicationBuilder::{auto_configuration::<A>(), auto_configurations(AutoConfigurations), exclude::<A>()}`; crate-private `AutoConfigurationCycle` error (Display: `auto-configurations form an ordering cycle: A -> B -> A` with type names; implements `Error`); crate-private `AutoConfigurationCycleAnalyzer` registered in `builtin()` after the bean-wiring analyzer, description "The auto-configurations form an ordering cycle: `A -> B -> A`." and action "Remove one `after` or `before` entry so that the order is no longer circular." In `execute`, after the user's `configure` closures: `builder.environment(config.clone())` placed BEFORE the closures (amendment 3), then the resolved auto-configurations appended as `group(type_name, A::conditions(), A::register)`; the resolution is a stable topological sort with ties in list order, unknown references ignored, `exclude` applied first; a cycle is a `Container`-phase `StartupError`.

- [ ] **Step 1: Write the failing tests** in `tests/autoconfig.rs`: `user_bean_wins_over_auto_configured_default` (on_missing_bean), `auto_configured_default_used_when_user_gave_none`, `order_by_after`, `order_by_before`, `after_and_before_agree_and_conflict_free`, `ties_keep_list_order`, `absent_reference_is_ignored`, `cycle_is_container_phase_with_fixed_text_and_report` (analyzer text exact), `exclude_removes_configuration`, `exclude_of_referenced_configuration_keeps_order_valid` (Review Focus 3), `exclude_absent_is_noop`, `several_lists_combine_in_call_order`, `class_condition_failing_skips_group_only` (Review Focus 2), `on_property_class_condition_reads_config`, `user_environment_replaces_config_environment`, `no_auto_configurations_changes_nothing`, `marker_value_never_in_cycle_error_or_report`.
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --test autoconfig`. Expected: FAIL.
- [ ] **Step 3: Implement** the builder methods, ordering, registration and cycle error/analyzer.
- [ ] **Step 4: Docs/examples** for the builder methods. Run `cargo test -p oxbloom-boot` (default, `--features web`, `--features "web security"`). Expected: PASS.

### Task 7: Boot: conditions report setting

**Files:**
- Modify: `crates/oxbloom-boot/src/app.rs`
- Test: `crates/oxbloom-boot/tests/conditions_report.rs`

**Interfaces:**
- Consumes: `ContainerBuilder::build_with_report` (Task 1).
- Produces: the key `oxbloom.main.conditions-report` read with `Config::get::<String>` like the banner mode (missing = `off`); `console` prints `ConditionReport` `Display` to the output target right after the build (before runners on success; before the failure report on a failed build); other values are a `Config`-phase `StartupError` (fixed text naming the key, never the value). The build always goes through `build_with_report`.

- [ ] **Step 1: Write the failing tests**: `off_prints_nothing`, `console_prints_report_after_build_before_runner` (log ordering), `console_prints_report_on_failed_build_before_failure_report`, `report_lists_matched_and_skipped_with_reasons`, `invalid_mode_is_config_phase_without_the_value`, `report_text_has_no_config_values` (marker), `exit_code_and_events_unchanged_by_report`.
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --test conditions_report`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p oxbloom-boot` (default, `web`, `web security`). Expected: PASS.

### Task 8: Documentation, `hello`, spec amendments, final gate

**Files:**
- Modify: `examples/hello/**` (add `src/auto.rs` with an `#[auto_configuration]` offering a default bean guarded by `on_missing_bean`, a user bean that overrides it behind a config flag, and a test showing both outcomes plus the `console` conditions report), `README.md` (section "Conditions and auto-configuration": model, API table, order rules, report, limits), `docs/API.md` (one entry with an example per new public item), `AGENT.md` (crate rows), `docs/ROADMAP-core.md` (C4a done; C4b remaining; follow-ups: link-time registry feature, config-based exclusion, missing-bean hint naming condition-skipped registrations, conditions on `#[component]`), `docs/superpowers/specs/2026-10-25-conditions-autoconfig-design.md` (append an "Amendments" section with the four amendments in plain prose).

- [ ] **Step 1: Example.** Keep every file of `examples/hello` under about 200 lines; comments say which feature each part shows; `cargo run -p hello -- --server.port=0` still works; the new test passes; run the example for real with and without the user override flag and with `--oxbloom.main.conditions-report=console`, report the real output.
- [ ] **Step 2: Docs** per the file list; state the limits plainly (order dependence, AND vs OR, `on_feature` is `#[cfg]`, explicit lists only, property values compared as text, a panicking condition aborts the build).
- [ ] **Step 3: Verify snippets compile** (temporary test file, deleted afterwards) and the depth probes: closures 206, component 277, `#[bean]` 230, `.bean` 194 KB debug.
- [ ] **Step 4: Final gate** per Global Constraints, plus `tests/signal.rs` and `tests/web.rs` 5 runs each, then `graphify update .`. Report real counts.
