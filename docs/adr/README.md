# Architecture decision records

An architecture decision record (ADR) is a short document that captures one architectural decision: the
context that made it necessary, what was decided, and what follows from it. Oxbloom keeps its ADRs in
`docs/adr/`. The Superpowers design specs in `docs/superpowers/specs/` remain the design documents of each
slice and the history of how it was built; the ADRs are the long-term record of the decisions inside them.
Each ADR links to the spec it came from.

## When to write one

Write an ADR for every architectural decision: a choice between approaches, a dependency choice, a limit,
or a rejected alternative that a later maintainer could wonder about or want to reverse. Write it in the
same change that implements the decision. One decision per ADR; group small choices only when they were
made together and cannot be separated.

## Rules

- File names are `NNNN-short-kebab-title.md`: four digits, ascending in the order the decisions were made.
  Take the next free number.
- Never edit the decision of an accepted ADR. To change a decision, write a new ADR, say in its Context
  which ADR it replaces and why, and set the old one's status to `Superseded by ADR NNNN`. Fixing a typo
  or a broken link is allowed.
- Link the ADR to its source spec and, where useful, to the plan. Link new ADRs from the spec or plan
  that introduces them.
- Write in plain full sentences. Write "Oxbloom" in prose and `oxbloom` in code.

## Status values

| Status | Meaning |
| --- | --- |
| Proposed | Written down, not yet agreed or implemented. |
| Accepted | Agreed and in force. |
| Superseded by ADR NNNN | Replaced by a later decision; the record stays as history. |
| Deprecated | No longer relevant, without a replacement. |

## Template

Copy [`0000-template.md`](0000-template.md). The fields are `Status`, `Date` (the date of the decision,
`YYYY-MM-DD`; for decisions taken from a spec, the spec's date) and `Source`; the sections are `Context`,
`Decision`, `Consequences` (good and bad) and, when there were real alternatives, `Alternatives
considered`.

## Index

| Number | Title | Status | Date | Source spec |
| --- | --- | --- | --- | --- |

