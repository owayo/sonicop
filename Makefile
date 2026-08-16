.PHONY: build release install clean test test-ruby fmt check gem gem-platform version-sync version-check help

.DEFAULT_GOAL := help

BINARY_NAME := sonicop
INSTALL_PATH ?= /usr/local/bin
RAKE ?= rake

# 入口はこの Makefile に一本化する。gem 配布とバージョン整合の実装は Rakefile 側にあり、
# ここからは rake タスクとして呼び出すだけにして、同じ処理を二重定義しない。

build: ## Build debug version
	cargo build --locked

release: ## Build optimized native executable
	cargo build --release --locked

install: release ## Install the executable
	install -m 755 target/release/$(BINARY_NAME) $(INSTALL_PATH)/$(BINARY_NAME)

# `--no-fail-fast` runs every test binary even after one of them fails. Without it cargo stops at
# the first binary that reports a failure, so a single failing unit test hides everything in
# `tests/cops.rs` -- and "1 test is failing" reads as "1 test is failing" when it means "at least 1".
# Measured on 2026-08-17: a run reported as 1 failure was 3 once the later binaries got to run.
test: test-ruby ## Run Rust and Ruby wrapper tests
	cargo test --all-targets --locked --no-fail-fast

test-ruby: ## Run the Ruby wrapper tests only
	$(RAKE) test:ruby

fmt: ## Format Rust sources
	cargo fmt --all

version-sync: ## Regenerate lib/sonicop/version.rb from Cargo.toml
	$(RAKE) version:sync

version-check: ## Fail when Cargo.toml and lib/sonicop/version.rb disagree
	$(RAKE) version:check

check: version-check ## Run all local quality gates
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features --locked -- -D warnings
	cargo test --all-targets --locked --no-fail-fast
	$(RAKE) test:ruby

gem: ## Build the source gem
	$(RAKE) gem

gem-platform: release ## Build a platform gem (set GEM_PLATFORM)
	test -n "$(GEM_PLATFORM)"
	$(RAKE) gem:platform BINARY="target/release/$(BINARY_NAME)" GEM_PLATFORM="$(GEM_PLATFORM)"

clean: ## Remove Cargo and gem build artifacts
	cargo clean
	rm -f libexec/$(BINARY_NAME) libexec/$(BINARY_NAME).exe
	rm -f $(BINARY_NAME)-*.gem

help: ## Show available targets
	@echo "$(BINARY_NAME) build commands"
	@echo ""
	@awk 'BEGIN {FS = ":.*?## "}; /^[a-zA-Z_-]+:.*?## / {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
