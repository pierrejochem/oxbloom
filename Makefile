# Run, build and test the examples under examples/.
#
#   make examples                         list the examples
#   make run-hello                        run examples/hello
#   make run EXAMPLE=hello ARGS="--server.port=0 --oxbloom.profiles.active=dev"
#   make test-hello                       test one example
#   make build-hello                      build one example
#   make check-examples                   build and test every example
#   make test-db                          run the database and migration conformance tests against
#                                         Postgres and MySQL in containers (docker compose, docker-compose
#                                         or podman compose)
#
# The cargo target directory comes from ~/.cargo/config.toml; do not override it here.

EXAMPLES := $(notdir $(patsubst %/,%,$(dir $(wildcard examples/*/Cargo.toml))))
EXAMPLE  ?=
ARGS     ?=

.PHONY: help examples run build test check-examples test-db

help:
	@echo "targets: examples, run-<name>, build-<name>, test-<name>, check-examples, test-db"
	@echo "or: make run EXAMPLE=<name> ARGS=\"--key=value\""
	@echo "examples: $(EXAMPLES)"

examples:
	@printf '%s\n' $(EXAMPLES)

run:
	@test -n "$(EXAMPLE)" || { echo "set EXAMPLE=<name>; one of: $(EXAMPLES)"; exit 2; }
	@echo "$(EXAMPLES)" | tr ' ' '\n' | grep -qx -- "$(EXAMPLE)" || { echo "unknown example '$(EXAMPLE)'; one of: $(EXAMPLES)"; exit 2; }
	cargo run -p $(EXAMPLE) -- $(ARGS)

build:
	@test -n "$(EXAMPLE)" || { echo "set EXAMPLE=<name>; one of: $(EXAMPLES)"; exit 2; }
	cargo build -p $(EXAMPLE)

test:
	@test -n "$(EXAMPLE)" || { echo "set EXAMPLE=<name>; one of: $(EXAMPLES)"; exit 2; }
	cargo test -p $(EXAMPLE)

run-%:
	@$(MAKE) --no-print-directory run EXAMPLE=$* ARGS="$(ARGS)"

build-%:
	@$(MAKE) --no-print-directory build EXAMPLE=$*

test-%:
	@$(MAKE) --no-print-directory test EXAMPLE=$*

check-examples:
	@for e in $(EXAMPLES); do cargo build -p $$e && cargo test -p $$e || exit 1; done

# The database conformance tests need real servers. The compose tool is whichever of
# `docker compose`, `docker-compose` and `podman compose` exists. The services always stop
# again, also when the tests fail, and the exit code is the one of the tests.
COMPOSE_FILE := compose.test.yaml

test-db:
	@if docker compose version >/dev/null 2>&1; then compose="docker compose"; \
	elif command -v docker-compose >/dev/null 2>&1; then compose="docker-compose"; \
	elif podman compose version >/dev/null 2>&1; then compose="podman compose"; \
	else echo "no compose tool found: install docker compose, docker-compose or podman compose"; exit 2; fi; \
	trap '$$compose -f $(COMPOSE_FILE) down -v' EXIT INT TERM; \
	if $$compose -f $(COMPOSE_FILE) up --help 2>&1 | grep -q -- --wait; then \
		$$compose -f $(COMPOSE_FILE) up -d --wait || exit 1; \
	else \
		$$compose -f $(COMPOSE_FILE) up -d || exit 1; \
		for i in $$(seq 1 90); do \
			if $$compose -f $(COMPOSE_FILE) ps 2>/dev/null | grep -q starting; then sleep 2; else break; fi; \
		done; \
		if $$compose -f $(COMPOSE_FILE) ps | grep -q unhealthy; then echo "a test database did not become healthy"; exit 1; fi; \
	fi; \
	OXBLOOM_TEST_POSTGRES_URL=postgres://oxbloom:oxbloom@127.0.0.1:55432/postgres \
	OXBLOOM_TEST_MYSQL_URL=mysql://root:oxbloom@127.0.0.1:53306/mysql \
	cargo test --workspace --all-features --no-fail-fast -- --ignored data_conformance data_migrate_conformance; \
	status=$$?; exit $$status
