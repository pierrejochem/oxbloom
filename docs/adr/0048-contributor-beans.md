# ADR 0048: Auto-configured web and security are extended through contributor beans

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [C4b: Auto-configure the web server, security chain and JWT decoder](../superpowers/specs/2026-10-25-web-security-autoconfig-design.md)

## Context

With conditions in place (C4a), the web server, security chain and JWT decoder could become auto-configurations. Applications still need to contribute routes, security rules and filters, and to replace a default entirely. Until then the application built the `App` and registered the server itself ([ADR 0036](0036-application-registers-web-server.md)).

## Decision

Contributor beans are the extension model. Routes come from beans registered as `dyn Controller` (`Controller` gained `Send + Sync` supertraits for this). Security rules come from `SecurityCustomizer` beans, applied in DI order to `HttpSecurity`. Filters come from `dyn Filter` beans. `JwtDecoderAutoConfiguration`, `SecurityAutoConfiguration` and `WebServerAutoConfiguration` live in `oxbloom-boot` behind its `web` and `security` features, because boot depends on both crates and owns the `AutoConfiguration` trait. A user bean of the same type (`SecurityFilterChain`, `WebServer`, `dyn JwtDecoder`) replaces the default. Without any customizer the default chain is `any_request().authenticated()`, so the application is closed unless it opens routes; without any controller no server is started. The web server remains a `Service` bean ([ADR 0035](0035-web-server-as-service-bean.md)). This supersedes [ADR 0036](0036-application-registers-web-server.md).

## Consequences

Good:

- Applications write no factory code for the server, chain or decoder.
- Secure by default: an auto-configured application is closed until it opens routes.

Bad:

- Behavior depends on which beans exist, which the conditions report has to explain.
- An application with no controller silently has no web server.

## Alternatives considered

- Replace-only defaults: rejected.
- Automatic collection of controllers: rejected.
