# Petal — common developer commands.
# Run `make` or `make help` to see the available targets.

.DEFAULT_GOAL := help
.PHONY: help build test test-examples clean

help: ## Show this help
	@echo "Petal — make targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "} {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

build: ## Build the Petal compiler (debug)
	cd rust && cargo build

test: build ## Run the full vitest suite (also runs every examples/*.ptl)
	cd ts && npx vitest run

test-examples: build ## Print each example program's output for manual inspection
	./ts/bin/test-examples.ts

clean: ## Remove Rust build artifacts
	cd rust && cargo clean
