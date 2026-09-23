# Petal — common developer commands.
# Run `make` or `make help` to see the available targets.

.DEFAULT_GOAL := help
.PHONY: help build test test-examples test-c-bridge clean

help: ## Show this help
	@echo "Petal — make targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "} {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

build: ## Build the Petal compiler (debug)
	cd rust && cargo build

test: build ## Run the full vitest suite (also runs every examples/console/*.ptl)
	cd ts && npx vitest run

test-examples: build ## Print each example program's output for manual inspection
	./ts/bin/test-examples.ts

test-c-bridge: ## Build petal-c-bridge (C/C++ embedding) with CMake + Ninja and run its tests
	cmake -S integrations/petal-c-bridge -B integrations/petal-c-bridge/build -G Ninja
	cmake --build integrations/petal-c-bridge/build
	ctest --test-dir integrations/petal-c-bridge/build --output-on-failure

clean: ## Remove Rust build artifacts
	cd rust && cargo clean
