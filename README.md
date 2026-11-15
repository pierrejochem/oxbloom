<p align="center"><img src="branding/logos/oxbloom-lockup.svg" alt="Oxbloom" width="360"></p>

<p align="center">
  <a href="https://github.com/pierrejochem/oxbloom/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/pierrejochem/oxbloom/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://pierrejochem.github.io/oxbloom/"><img alt="Documentation" src="https://img.shields.io/badge/docs-pierrejochem.github.io%2Foxbloom-b5462a"></a>
  <a href="#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-56722b"></a>
  <img alt="Rust 1.99+" src="https://img.shields.io/badge/rust-1.99%2B-5e1a1d?logo=rust">
  <img alt="Edition 2024" src="https://img.shields.io/badge/edition-2024-1c1714">
  <a href="https://github.com/pierrejochem/oxbloom/commits/main"><img alt="Last commit" src="https://img.shields.io/github/last-commit/pierrejochem/oxbloom?color=f4b39a&labelColor=1c1714"></a>
</p>

# Oxbloom

**Enterprise Rust, in bloom.**

Modular, Spring-Boot-inspired MVC framework for Rust: dependency injection, typed configuration,
auto-configuration, a web server with filters and errors, security, logging and test support.

## Documentation

The full developer documentation is at **<https://pierrejochem.github.io/oxbloom/>**:

- [Guide](https://pierrejochem.github.io/oxbloom/guide/01-status.html): status, CLI, dependency injection, configuration, application lifecycle, conditions and auto-configuration, logging, security, web errors and filters, JSON, CORS, compression, static resources, the examples, SSL bundles and HTTPS, the actuator (health, info and log level endpoints), task scheduling (cron, fixed rate and fixed delay), password login with HTTP Basic, sessions (server-side or an encrypted cookie), and the roadmap.
- [API reference](https://pierrejochem.github.io/oxbloom/API.html): one runnable example per public function.
- [Rustdoc](https://pierrejochem.github.io/oxbloom/api/oxbloom_boot/index.html): generated documentation of every crate.
- [Architecture decisions](https://pierrejochem.github.io/oxbloom/adr/): the architecture decision records, one per design decision, in `docs/adr/`.
- Roadmaps, design specs and implementation plans.

The pages are built from `docs/` by `.github/workflows/pages.yml`. To build them locally:

```bash
scripts/gen-book-summary.sh
mdbook build
```

## Repository

- `crates/`: `oxbloom-core`, `-macros`, `-web`, `-di`, `-config`, `-security`, `-boot`, `-logging`, `-ssl`, `-actuator`, `-tasks`, `-test` and `-cli`.
- `examples/`: `hello` and `todos`; run them with `make run-hello` and `make run-todos`.
- `IDEA.md`: the overall project plan. `branding/`: brand assets and naming rules. `AGENT.md`: rules for contributors and coding agents.

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Oxbloom by you, as defined in the Apache-2.0 license, is dual licensed as above, without any additional terms or conditions.
