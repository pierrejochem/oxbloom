# Conditions and auto-configuration (slice C4a)

Date: 2026-10-25

Status: implemented. The text below includes the changes listed under Amendments at the end.

Roadmap item: C4 in `docs/ROADMAP-core.md`, split into C4a (this spec: the mechanism) and C4b (moving the existing wiring into auto-configurations, and property metadata generation).

## Purpose

Today an Oxbloom application registers every bean by hand. A framework crate cannot offer a sensible default (a web server, a security chain, a JWT decoder) that steps aside when the application provides its own, and a developer cannot ask why a bean is missing. Spring Boot solves this with conditions and auto-configuration. C4a adds the same mechanism in a way that suits Rust: no classpath scanning, no global state, explicit lists.

Success means that a crate can ship an auto-configuration whose beans are registered only when their conditions hold; that user-registered beans always win over auto-configured ones; that auto-configurations can be ordered relative to each other; and that a developer can print a report of every condition that was evaluated, with the reason it matched or not.

## Scope

In scope:

- Conditions in `oxbloom-di`: a generic `condition`, `on_bean`, `on_missing_bean`, `on_property`, `on_property_value`, as modifiers on any registration, and `group` for class-level conditions.
- The `Environment` trait in `oxbloom-di` and its implementation for `Config`.
- The condition report.
- In `oxbloom-boot`: the `AutoConfiguration` trait, `AutoConfigurations` lists, ordering with `after` and `before`, `exclude`, registration after the user's wiring, and the `oxbloom.main.conditions-report` setting.
- Macros: `#[auto_configuration(..)]` and the new `#[bean(..)]` condition options.

Out of scope, with the owner:

- Moving the web server, the security chain and the JWT decoder into auto-configurations, and property metadata generation: C4b.
- Link-time collection of auto-configurations: a later optional `registry` feature that does not change the explicit API.
- Exclusion through configuration keys.
- Condition options on `#[component]` (the builder modifiers cover it).
- Naming condition-skipped registrations inside the missing-bean failure report.
- `on_feature`: a cargo feature is a compile-time `#[cfg(feature = "..")]`, so there is no runtime condition for it. This is documented.

## Decisions taken in brainstorming

1. Scope split into C4a and C4b (user choice).
2. Discovery is explicit: each crate exposes `auto_configurations()`, applications add their own; a link-time registry may come later as an opt-in feature (user choice).
3. Conditions apply to any registration and are evaluated inside `oxbloom-di` in registration order, against the registrations kept so far (user choice).
4. Class-level conditions use groups evaluated once (design decision made while presenting section 2; approved).
5. Properties reach DI through an `Environment` trait so that `oxbloom-di` keeps no dependency on `oxbloom-config` (design decision; approved).

## Conditions in `oxbloom-di`

### Model

A condition is a modifier on the most recent registration, like `profile`, `lazy` or `primary`. `build()` evaluates conditions as its first filtering step after profile filtering and before the provider index is built. It walks the registrations in registration order, and for each one asks its conditions (first those of enclosing groups that are not decided yet, then its own) in the context of the registrations kept so far, in order, stopping at the first one that does not match. A registration is kept when all its conditions match; otherwise it is skipped. A skipped registration is invisible, exactly like one skipped by a profile: it is not a candidate, it cannot cause `Duplicate`, a dependency on it is `Missing`, and its provider, hooks and coercions never run.

Several conditions on one registration are combined with AND. This differs from profile conditions, which are combined with OR, and the documentation says so.

### API (all new public items have runnable doc examples)

- `ContainerBuilder::condition(name: &'static str, f: impl Fn(&ConditionContext) -> ConditionOutcome + Send + Sync + 'static) -> Self`
- `ContainerBuilder::on_bean::<T: ?Sized + 'static>()` and `on_missing_bean::<T: ?Sized + 'static>()`: some earlier kept registration provides `T` (its concrete type, or `dyn Trait` through `provides`), or none does.
- `ContainerBuilder::on_property(key: &str)` (the key is present) and `on_property_value(key: &str, value: &str)` (the value equals `value` exactly, case-sensitive). Both treat an absent key as not matched. `on_property_value_or_missing` is the variant that matches when the key is absent. `on_property` asks `Environment::contains` (amendment 17); the value conditions are sugar over `ConditionContext::property`.
- `ContainerBuilder::environment(impl Environment + 'static) -> Self`: the property source. Without one, every property is absent.
- `ConditionOutcome`: `ConditionOutcome::matched()` and `ConditionOutcome::skipped(reason: impl Into<String>)`. Reasons must be fixed text built from type names and condition names; they must never contain configuration values.
- `ConditionContext`: `has_bean::<T: ?Sized + 'static>() -> bool` (the same check as `on_bean`), `property(key) -> Option<String>`, `contains(key) -> bool` (amendment 17), `active_profiles() -> &[String]`, `registration() -> &str` (the label of the registration being decided).
- `trait Environment: Send + Sync { fn property(&self, key: &str) -> Option<String>; fn contains(&self, key: &str) -> bool { .. } }`, where `contains` defaults to `property(key).is_some()` (amendment 17).
- `Conditions`: a small value with the same condition methods as the builder (`new()`, `condition`, `on_bean`, `on_missing_bean`, `on_property`, `on_property_value`, `on_property_value_or_missing`), used for groups.
- `ContainerBuilder::group(name: &'static str, conditions: Conditions, f: impl FnOnce(ContainerBuilder) -> ContainerBuilder) -> Self`: runs `f`, and treats every registration `f` made as one group. The group's conditions are evaluated once, at the position of the group's first registration that a profile keeps, against the registrations kept before it. If they do not match, every registration of the group is skipped. Groups may nest; all conditions along the path are combined with AND. A group with no registrations is a no-op, and a group whose registrations are all skipped by profiles is never decided and leaves no record. The start of a group is a modifier barrier: a modifier inside `f` before its first registration is `Invalid`.
- `ContainerBuilder::build_with_report(self) -> (Result<Container, DiError>, ConditionReport)`: like `build()`, but also returns the report when the build fails. `build()` is `build_with_report().0`, and `Container::condition_report() -> &ConditionReport` gives the report of a successful build.
- `ConditionReport`: `records() -> &[ConditionRecord]` in evaluation order, with `Display` rendering one line per record. `ConditionRecord`: `registration() -> &str`, `condition() -> &str`, `matched() -> bool`, `reason() -> &str`. For a group the registration is the group's name. A registration skipped by a profile has no condition record.

### Errors and edge cases

- A condition modifier before any registration is `Invalid` (the same rule as other modifiers).
- A condition closure that panics aborts `build()` like any panic in user code (documented).
- `on_bean` and `on_missing_bean` look only at registrations kept before the one being decided, including those inside enclosing groups that were kept. A registration never sees itself or later registrations.
- Stack use: conditions run once before `resolve::build`; the depth probes (debug minima 206 KB closures, 277 KB component, 230 KB `#[bean]`) must not change.
- Report text and `ConditionOutcome` reasons carry no configuration values: `on_property_value` records `property "<key>" does not have the expected value`, never the actual or expected value.

## `oxbloom-config`

Under the `di` feature, `Config` implements `oxbloom_di::Environment`: `property(key)` returns the leaf's scalar text if the key exists and is a scalar, otherwise `None` (a table or list is `None`). Its `contains(key)` is true for any existing key, a table or list included (amendment 17). Secrets are not special: an `on_property_value` match on a secret key compares the plain text, and the report never shows it.

## Macros (`oxbloom-macros`)

- `#[bean(on_bean(T), on_missing_bean, on_missing_bean(T), on_property("key"), on_property("key", "value"))]`: the options apply as the corresponding builder modifiers to the registration the method makes. `on_missing_bean` without an argument means the method's own return type, as in Spring. Several options are ANDed. Misuse (an empty key, a non-type argument) is a spanned compile error.
- `#[auto_configuration(after(A, B), before(C), on_bean(T), on_missing_bean(T), on_property("key"), on_property("key", "value"))]` on an inherent impl. It implies `#[configuration]` and generates `impl ::oxbloom_boot::AutoConfiguration` (class-level conditions through `Conditions`, ordering through `after()` and `before()`). `oxbloom-boot` re-exports the macro. The type must also implement `Component` (usually `#[component]` on the struct).

## Auto-configuration (`oxbloom-boot`)

### Types

```rust
pub trait AutoConfiguration: Configuration + Component {
    fn conditions() -> Conditions { Conditions::new() }
    fn after() -> Vec<ConfigRef> { Vec::new() }
    fn before() -> Vec<ConfigRef> { Vec::new() }
}
```

`ConfigRef` identifies a configuration by `TypeId` and type name; `ConfigRef::of::<T>()`. `AutoConfigurations` is an ordered list with `new()`, `add::<A: AutoConfiguration>()` and `extend(other)`. A crate that ships auto-configurations exposes `pub fn auto_configurations() -> AutoConfigurations`.

### Builder

`OxbloomApplicationBuilder` gains `auto_configuration::<A>()`, `auto_configurations(AutoConfigurations)` (lists combine in call order) and `exclude::<A: AutoConfiguration>()` (the configuration is dropped from the combined list; excluding something absent is a no-op).

### Run order

Boot first registers `Config` as the container `environment`, then the boot instances and `active_profiles`, then applies the user's `configure` closures, so a condition in a closure reads `Config` and a closure may replace the environment with its own `environment(..)` call. Boot then resolves the order of the auto-configurations and appends each one as a `group` (named by its type name, with its class-level conditions) containing `component::<A>()` followed by `A::register`. User registrations therefore come first, which gives back-off: an auto-configured bean with `on_missing_bean` skips itself when the user provided one.

### Ordering

`A::after() = [B]` and `B::before() = [A]` both mean B is processed before A. The order is a stable topological sort of the combined list: among the configurations whose predecessors are all placed, the earliest in the list goes next. A referenced configuration that is not in the list, and a reference of a configuration to itself, are ignored. A cycle is a `StartupError` in phase `Container` whose text is fixed and names the types on the cycle (type names only), `auto-configurations form an ordering cycle: A -> B -> A`, where each arrow points from a configuration to one it must come after. A separate built-in analyzer, placed after the C3b bean-wiring analyzer, gives it a description and an action.

### Report setting

The configuration key `oxbloom.main.conditions-report` is `off` (default) or `console`. Any other value is a `StartupError` in phase `Config` without the value in its text. Boot reads the setting before it prints the banner, so a rejected value prints nothing. In `console` mode boot uses `build_with_report` and writes the report to the output target after the build, whether or not the build succeeded, before the runners (success) or before the failure report (failure). Exact format, one block:

```
CONDITIONS EVALUATION REPORT

Matched:
  <registration>: <condition> - <reason>

Skipped:
  <registration>: <condition> - <reason>
```

A section with no records prints `  (none)`.

## Error handling and safety rules

- No message or report contains configuration values, argument values or secrets. Type names, configuration key names, condition names and fixed reasons are allowed.
- Condition evaluation never changes the order or the result of provider creation; it only decides which registrations exist.
- No global state: two applications in one process evaluate independently.
- The explicit list is the only discovery mechanism in this slice.

## Testing

- DI conditions: back-off in both registration orders; AND of several conditions; a skipped registration is invisible (no `Duplicate`, dependants get `Missing`, provider never runs); profile-skipped registrations do not count for `on_bean`; a condition before any registration is `Invalid`; groups (a class-level `on_missing_bean::<Foo>` whose first bean provides `Foo` does not hide its siblings; nested groups AND; empty group is a no-op); a panicking condition aborts `build()`; the report holds one record per evaluated condition in order with value-free reasons; `build_with_report` returns the report on failure; stack probes unchanged.
- Environment and properties: `on_property`, `on_property_value`, absent key, `_or_missing` variant; `Config` as `Environment` (scalar, table gives `None`); a marker value never appears in any reason.
- Boot: ordering by `after` and `before`; ties keep list order; an absent reference is ignored; a cycle is a `Container`-phase error with the analyzer report; `exclude`; user beans win; several lists combine; `conditions-report` modes `off` and `console`, also on a failed build; an invalid mode value is a `Config`-phase error without the value.
- Macros: every option form of `#[auto_configuration]` and `#[bean]`; trybuild cases for unknown options, an empty `on_property`, a bad `after` argument; `on_missing_bean` without an argument uses the return type.
- Hygiene: a marker in a config value never appears in the report or in the cycle error.

## Review Focus

1. A condition that refers to a type registered later (`on_bean::<Later>()` placed before `Later`) is not matched, and the report says so; this is the documented order rule, tested.
2. An auto-configuration whose class-level condition is skipped never runs `register`'s providers, and its siblings in other groups are unaffected.
3. `exclude` of a configuration that other configurations list in `after` or `before` does not break ordering.
4. `on_property_value` on a key whose value contains the expected text as a substring does not match (exact comparison).
5. The report for 1000 registrations stays linear (no quadratic scan per condition: `has_bean` uses an index built incrementally while walking).

## Task outline

About eight tasks, each with its own review:

1. `oxbloom-di`: `Environment`, `ConditionContext`, `ConditionOutcome`, entry conditions and their evaluation, the report, `build_with_report`, depth-probe re-check.
2. `oxbloom-di`: `Conditions`, `group`, nesting, tests for class-level semantics.
3. `oxbloom-config`: `Environment for Config` under feature `di`.
4. `oxbloom-macros`: `#[bean(..)]` condition options.
5. `oxbloom-macros` and `oxbloom-boot`: `AutoConfiguration` trait and `#[auto_configuration]`.
6. `oxbloom-boot`: `AutoConfigurations`, ordering, `exclude`, registration after user wiring, cycle error and analyzer.
7. `oxbloom-boot`: the `conditions-report` setting and printing.
8. Documentation, `hello` with an auto-configuration and a user override, spec amendments, final gate.

## Amendments

These changes were decided while planning and implementing C4a. The text above has been corrected to match them where it said something different.

### Decided while planning

1. Short-circuit evaluation. The conditions of one registration are evaluated in order, and evaluation stops at the first condition that does not match. A skipped registration therefore has exactly one record with `matched = false` for its own conditions; the conditions after it are not evaluated and have no record. A group's conditions follow the same rule.
2. Macro paths. The macro `#[auto_configuration]` generates paths into `::oxbloom_boot`, where the `AutoConfiguration` trait lives, and `oxbloom-boot` re-exports the macro, so `use oxbloom_boot::auto_configuration;` works. The crate that uses it must be able to name both `oxbloom_boot` and `oxbloom_di`, as the other macros already require for `oxbloom_di`.
3. Environment before the closures. Boot registers `Config` as the container environment before the user's `configure` closures, so a user `environment(..)` call can replace it. This is a visible change: a property condition written in a `configure` closure reads `Config` without any extra call.
4. Cycle error type. A cycle in the auto-configuration order is reported through a crate-private error type, which the C3b analyzer family matches with `find_cause`. It has its own analyzer, separate from the bean-wiring analyzer, placed right after it in the list of built-in analyzers.

### Decided while implementing

5. Component requirement. `AutoConfiguration` requires `Configuration + Component`. The `#[bean]` methods of an auto-configuration run on `Arc<Self>`, so the configuration's instance must be in the container. The descriptor's register function is `component::<A>()` followed by `A::register`, so the instance is registered inside the group, as its first registration, and is skipped together with it. Users write `#[component]` on the struct (a unit struct works) and `#[auto_configuration]` on its `impl` block.
6. When a group is decided. A group is decided at its first registration that a profile keeps, against the registrations kept before it. A group whose registrations are all skipped by profiles is never decided: its conditions do not run and it leaves no record.
7. Modifier barrier. The start of a group is a barrier for modifiers. A modifier written inside the group's `f` before its first registration finds no registration and makes `build()` return `Invalid`; it never changes the registration before the group. A modifier written after `group(..)` applies to the last registration made inside it.
8. Replacement builder. If a group's `f` returns a different builder instead of extending the one it was given, that builder is used as it is, with its own groups, and the group has no effect. `group` does not panic in this case.
9. Report sections. `Display` of `ConditionReport` sorts the records into `Matched:` and `Skipped:` by each record's own flag. A registration whose first condition matched and whose second did not is skipped, yet its first record is listed under `Matched:`; the same holds for the record of a group that matched when a registration inside it is then skipped by its own condition. The sections list conditions, not kept registrations.
10. Tie rule. The topological sort is Kahn's algorithm that always takes the ready configuration earliest in the list. For the list `[P, Q, R, S]` with S before Q, the order is P, R, S, Q. Spring Boot's sorter would give P, S, Q, R, because it moves S directly in front of Q; Oxbloom documents the difference. A configuration that names itself in `after` or `before` is ignored, not reported as a cycle.
11. Cycle text. The cycle text is `auto-configurations form an ordering cycle: A -> B -> A`, and each arrow points from a configuration to one it must come after. The analyzer's description is ``The auto-configurations form an ordering cycle: `A -> B -> A`.`` with the action "Remove one `after` or `before` entry so that the order is no longer circular." A cycle fails before the container is built, so no conditions report is printed for it.
12. Run order. The run order paragraph was rewritten: boot sets `Config` as the environment first, then registers its instances and the active profiles, then applies the `configure` closures, then appends the auto-configurations as groups.
13. Bound of `exclude`. `exclude::<A>()` requires `A: AutoConfiguration`, so a typo or a type that is not an auto-configuration is a compile error instead of a silent no-op.
14. Reading the setting. Boot reads `oxbloom.main.conditions-report` before it prints the banner, so a rejected value prints nothing. In `console` mode the report is printed right after the build, before `context_ready` on success and before `failed` and the failure report on failure. The conditions report and the failure report follow each other without a blank line.
15. Empty property keys. On the builder, `on_property("")`, `on_property_value("", ..)` and `on_property_value_or_missing("", ..)` are misuse, like the macro's compile error for an empty key: `build()` returns `Invalid` with a value-free text such as "`on_property` needs a non-empty key". The same calls on `Conditions` record the misuse in the value, and the `group` call that receives it records it on the builder.
16. Documented pitfalls. Bare `#[bean(on_missing_bean)]` guards the concrete return type, also next to `provides(dyn Trait)`, so it does not back off for the trait; `on_missing_bean(dyn Trait)` does. `on_property("key")` matches whenever the key is present, also with the value `false`, unlike Spring; `on_property_value` compares case-sensitively, unlike Spring's `havingValue`. An override placed in an auto-configuration that comes after the starter's keeps both beans, so overrides belong in `configure`.
17. Presence of a section (C4b). `on_property(key)` is no longer sugar over `ConditionContext::property`. It asks the new `Environment::contains(key)`, whose default is `property(key).is_some()`, so custom environments behave as before. `Config` overrides `contains`, so a table or a list counts as present and `on_property("security.jwt")` matches a `[security.jwt]` section. `property` still returns `None` for tables and lists, and the value conditions still compare scalar text. `ConditionContext::contains(key)` exposes the same check to custom conditions.
