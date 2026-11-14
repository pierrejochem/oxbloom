# ADR 0023: Secret keys are refused from the command line

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md)

## Context

Command-line arguments are visible in process listings, so a secret passed as `--security.jwt.secret=...` leaks to every user on the machine.

## Decision

`ConfigBuilder::secret_key("path")` registers a key that must not come from the command line. A command-line argument for it, or for any key below it, is `ConfigError::Invalid` with a message pointing to environment variables or files; the same check applies to values moved there by a `renamed` rule. `oxbloom-security` registers `security.jwt.secret`, and as defense in depth `jwt_decoder_from_config` also rejects a secret whose origin is `Origin::Cli`, whether or not the caller registered the key.

## Consequences

Good:

- Secrets cannot leak through process listings by mistake.
- The check fails closed even when an application forgets to register the key.

Bad:

- Tests that set secrets must use files or environment maps instead of command-line style overrides (`ContextRunner::property` refuses them too).
