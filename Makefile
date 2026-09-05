# The four things a person types, and nothing else.
#
# Every target here is a `cargo` line someone would otherwise have to remember,
# and AGENTS.md stays the place that says *why* each of them is the command it
# is. If a target ever needs a paragraph, the paragraph belongs there and the
# target belongs here.
#
# The VS Code extension (editors/vscode) is TypeScript and is part of `install`
# and `build` when npm is on the PATH, and skipped with a line saying so when it
# is not — a Rust checkout that cannot build the editor surface is still a
# working checkout.

CARGO ?= cargo
EXTENSION := editors/vscode
BIND ?= 127.0.0.1:7878

.DEFAULT_GOAL := help
.PHONY: help install test build up fmt lint

help:
	@echo "make install   fetch dependencies, Rust and the VS Code extension's"
	@echo "make test      cargo test --workspace, and the probes that need no model"
	@echo "make build     release binary, and the extension if npm is here"
	@echo "make up        the debug UI and the agent protocol on $(BIND)"
	@echo ""
	@echo "make fmt       cargo fmt --all"
	@echo "make lint      what CI runs: fmt --check and clippy with -D warnings"

install:
	$(CARGO) fetch
	@if command -v npm >/dev/null 2>&1; then \
		echo "==> npm ci in $(EXTENSION)"; \
		cd $(EXTENSION) && npm ci; \
	else \
		echo "==> no npm on PATH: skipping $(EXTENSION)"; \
	fi

# The whole workspace, which includes the selection probe — 38 questions scored
# against scripts/tasks/map-order-probe.key with no model on the machine.
test:
	$(CARGO) test --workspace

build:
	$(CARGO) build --release --workspace
	@if command -v npm >/dev/null 2>&1; then \
		echo "==> tsc in $(EXTENSION)"; \
		cd $(EXTENSION) && npm run compile; \
	else \
		echo "==> no npm on PATH: skipping $(EXTENSION)"; \
	fi

# Loopback, and no token: `serve` refuses any other address without
# --auth-token-file, because /ws carries job approval. Sessions are cached in
# the state directory, so a restart does not lose the conversation.
up:
	$(CARGO) run --bin luu -- serve --bind $(BIND)

fmt:
	$(CARGO) fmt --all

lint:
	$(CARGO) fmt --all --check
	RUSTFLAGS="-D warnings" $(CARGO) clippy --workspace --all-targets
