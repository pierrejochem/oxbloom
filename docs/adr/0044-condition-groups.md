# ADR 0044: Class-level conditions are decided once per group

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Conditions and auto-configuration (slice C4a)](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md)

## Context

An auto-configuration has conditions for the whole configuration (Spring's class-level conditions) as well as per bean. Copying the class-level condition onto every bean would re-evaluate it after earlier beans of the same configuration were kept, which changes its meaning (`on_missing_bean::<Foo>` would see the configuration's own `Foo`).

## Decision

`ContainerBuilder::group(name, conditions, f)` treats every registration made by `f` as one group. The group's conditions are evaluated once, at the position of its first registration that a profile keeps, against the registrations kept before it; if they do not match, every registration of the group is skipped. Groups nest and their conditions combine with AND. A group whose registrations are all profile-skipped is never decided and leaves no record. The start of a group is a modifier barrier.

## Consequences

Good:

- Class-level conditions keep their intended meaning.
- Auto-configurations map onto groups one to one.

Bad:

- Groups add rules (decision point, barrier, replacement builder) that need documentation.
