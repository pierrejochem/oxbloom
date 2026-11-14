<p align="center"><img src="branding/logos/oxbloom-lockup.svg" alt="Oxbloom" width="360"></p>

# Oxbloom

**Enterprise Rust, in bloom.**

Modular, Spring-Boot-inspired MVC framework for Rust: dependency injection, typed configuration,
auto-configuration, a web server with filters and errors, security, logging and test support.

## Documentation

The full developer documentation is at **<https://pierrejochem.github.io/oxbloom/>**:

- [Guide](https://pierrejochem.github.io/oxbloom/guide/01-status.html): status, CLI, dependency injection, configuration, application lifecycle, conditions and auto-configuration, logging, security, web errors and filters, JSON, CORS, compression, static resources, the examples and the roadmap.
- [API reference](https://pierrejochem.github.io/oxbloom/API.html): one runnable example per public function.
- [Rustdoc](https://pierrejochem.github.io/oxbloom/api/oxbloom_boot/index.html): generated documentation of every crate.
- Roadmaps, design specs and implementation plans.

The pages are built from `docs/` by `.github/workflows/pages.yml`. To build them locally:

```bash
scripts/gen-book-summary.sh
mdbook build
```

## Repository

- `crates/`: `oxbloom-core`, `-macros`, `-web`, `-di`, `-config`, `-security`, `-boot`, `-logging`, `-test` and `-cli`.
- `examples/`: `hello` and `todos`; run them with `make run-hello` and `make run-todos`.
- `IDEA.md`: the overall project plan. `branding/`: brand assets and naming rules. `AGENT.md`: rules for contributors and coding agents.

## License

MIT OR Apache-2.0.
