# Run, build and test the examples under examples/.
#
#   make examples                         list the examples
#   make run-hello                        run examples/hello
#   make run EXAMPLE=hello ARGS="--server.port=0 --oxbloom.profiles.active=dev"
#   make test-hello                       test one example
#   make build-hello                      build one example
#   make check-examples                   build and test every example
#
# The cargo target directory comes from ~/.cargo/config.toml; do not override it here.

EXAMPLES := $(notdir $(patsubst %/,%,$(dir $(wildcard examples/*/Cargo.toml))))
EXAMPLE  ?=
ARGS     ?=

.PHONY: help examples run build test check-examples

help:
	@echo "targets: examples, run-<name>, build-<name>, test-<name>, check-examples"
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
