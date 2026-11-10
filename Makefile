# Petal — common developer commands.
# Run `make` or `make help` to see the available targets.

.DEFAULT_GOAL := help

# The Rust test targets run at lower priority so a long build or test run
# leaves the machine responsive; `make NICE= test-core` runs them at normal
# priority. Build parallelism is capped in .cargo/config.toml and nextest's in
# core/.config/nextest.toml (see AGENTS.md, "Running the Rust tests").
NICE ?= nice -n 10
# cargo-nextest when it is installed (`cargo install cargo-nextest --locked`),
# else plain `cargo test`. `make NEXTEST= test-core` forces `cargo test`.
# nextest does not run doctests, so test-core runs those with `cargo test --doc`.
NEXTEST ?= $(shell cargo nextest --version >/dev/null 2>&1 && echo yes)
.PHONY: help build test test-core test-examples test-ui-corpus test-c-bridge expensive_lint_tests prove clean

help: ## Show this help
	@echo "Petal — make targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "} {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'

build: ## Build the petal binary (debug), cli/target/debug/petal
	cd cli && cargo build

test: build ## Run the full vitest suite (also runs every examples/console/*.ptl)
	npx vitest run

test-core: ## Run the petal crate's Rust tests twice: as embedders get it (no bundled libraries) and with every bundled library (--features libraries); uses cargo-nextest when installed
ifeq ($(NEXTEST),yes)
	cd core && $(NICE) cargo nextest run
	cd core && $(NICE) cargo nextest run --features libraries
	cd core && $(NICE) cargo test --doc
	cd core && $(NICE) cargo test --doc --features libraries
else
	cd core && $(NICE) cargo test
	cd core && $(NICE) cargo test --features libraries
endif

test-examples: build ## Print each example program's output for manual inspection
	./tools/test-examples.ts

test-ui-corpus: ## Run the core-feature tests over the panel corpus (frame gate, memo, static cache, effect audit, DM); see test/README.md
	cd test/ui-corpus && $(NICE) cargo test

test-c-bridge: ## Build petal-c-bridge (C/C++ embedding) with CMake + Ninja and run its tests
	cmake -S integrations/petal-c-bridge -B integrations/petal-c-bridge/build -G Ninja
	cmake --build integrations/petal-c-bridge/build
	ctest --test-dir integrations/petal-c-bridge/build --output-on-failure

expensive_lint_tests: ## Run the fmt/lint/CST sweeps over every .ptl in the repo (minutes of CPU; PETAL_TEST_JOBS=<n> caps the threads)
	cd core && $(NICE) cargo test --features libraries --lib over_repo_corpus -- --ignored

prove: ## Run the Kani proofs of the core numeric semantics (needs cargo-kani)
	cd core && cargo kani --solver cadical

clean: ## Remove Rust build artifacts
	cd core && cargo clean
	cd cli && cargo clean
	cd core-libs/petal-pkgman && cargo clean
	cd test/ui-corpus && cargo clean
