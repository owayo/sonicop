# Development tasks for sonicop. Run `make` with no arguments to list the targets.
#
# Tool versions are pinned in mise.toml. When mise is available, every tool runs through
# `mise exec --`, so the pinned versions are used even when mise is not activated in the shell
# (for example when make is started from an IDE or a GUI). SYSTEM_TOOLS=1 uses the tools on PATH
# instead (the versions are then not guaranteed).
#
# Only GNU Make 3.81 features are used (the make that ships with macOS):
# no .ONESHELL, .SHELLFLAGS, $(file ...) or !=.
#
# This Makefile is the single entry point. The gem packaging and the version sync live in the
# Rakefile, and the targets here only call those rake tasks, so nothing is defined twice.

.DEFAULT_GOAL := help

BINARY_NAME := sonicop
INSTALL_PATH ?= /usr/local/bin
# Cargo.lock is committed, so resolve dependencies exactly as CI does
CARGO_FLAGS ?= --locked
RAKE ?= rake

# ---- Toolchain ------------------------------------------------------------------
# Look for mise on PATH, then in the usual install locations (make started from a GUI may not
# inherit the shell's PATH). Override with make MISE=/path/to/mise.
# To try the behavior without mise, empty the candidates with MISE_CANDIDATES=.
MISE_CANDIDATES ?= $(HOME)/.local/bin/mise /opt/homebrew/bin/mise /usr/local/bin/mise
ifeq ($(SYSTEM_TOOLS),1)
RUN :=
else
ifndef MISE
MISE := $(firstword $(shell command -v mise 2>/dev/null) $(wildcard $(MISE_CANDIDATES)))
endif
ifeq ($(MISE),)
ifneq ($(filter-out help,$(or $(MAKECMDGOALS),help)),)
$(error mise was not found. Install it from https://mise.jdx.dev, or add SYSTEM_TOOLS=1 to use the tools on PATH)
endif
endif
RUN := $(if $(MISE),$(MISE) exec --,)
endif

# ---- Upstream spec fixtures (maintainers only) ------------------------------------
# The input is collected in two steps. spec-capture *runs* upstream's own spec suite and records
# whatever reaches expect_offense; spec-fixtures then runs the upstream gem over that recording to
# take the expectations. Reading the specs as text cannot recover shared_examples, #{} or the
# cop_config a surrounding context builds, and 54 of the 609 cops were missed entirely that way.
#
# The upstream checkout, the capture hook and the generator are not part of this repository, so
# pass their locations, for example
#   make spec-capture UPSTREAM_SPEC_TREE=<rubocop v1.89.0 checkout> SPEC_CAPTURE_HOOK=<spec_capture.rb>
#   make spec-fixtures SPEC_FIXTURE_GEN=<spec_fixture_gen.py>
# Both run upstream's code, so they use the ruby and python3 on PATH (where upstream's development
# gems and the rubocop gem are installed) instead of going through mise exec.
UPSTREAM_SPEC_TREE ?=
SPEC_CAPTURE_HOOK ?=
SPEC_FIXTURE_GEN ?=
SPEC_CAPTURE_OUT ?= tests/fixtures/upstream_spec_capture.jsonl

.PHONY: help setup build release run test test-ruby lint fmt fmt-check check ci version-sync version-check gem gem-check gem-platform cop-coverage spec-capture spec-fixtures install uninstall clean

## Setup

setup: ## Install the toolchain (mise) and dependencies
	@if [ -n "$(MISE)" ]; then "$(MISE)" install; fi
	$(RUN) cargo fetch $(CARGO_FLAGS)

## Build

build: ## Build a debug binary
	$(RUN) cargo build $(CARGO_FLAGS)

release: ## Build a release binary
	$(RUN) cargo build --release $(CARGO_FLAGS)

run: ## Run the debug binary (arguments via ARGS="...")
	$(RUN) cargo run $(CARGO_FLAGS) -- $(ARGS)

## Checks

# --no-fail-fast keeps running the remaining test binaries after one fails. Without it cargo stops
# at the first failure and hides tests/cops.rs and the rest, so "1 failure" would really mean "at
# least 1 failure": on 2026-08-17 a run reported as 1 failure had 3 once the rest ran too.
test: test-ruby ## Run the tests (Rust and the Ruby wrapper)
	$(RUN) cargo test $(CARGO_FLAGS) --all-targets --no-fail-fast

test-ruby: ## Run only the Ruby wrapper tests
	$(RUN) $(RAKE) test:ruby

lint: ## Run clippy with warnings as errors
	$(RUN) cargo clippy $(CARGO_FLAGS) --all-targets --all-features -- -D warnings

fmt: ## Format the code (rewrites files)
	$(RUN) cargo fmt --all

fmt-check: ## Check the formatting (no changes)
	$(RUN) cargo fmt --all -- --check

check: fmt-check lint version-check ## Run fmt-check and lint (no changes), and check that the versions agree

ci: check test gem-check ## Run the same checks as CI (no changes)

## Version

version-sync: ## Regenerate lib/sonicop/version.rb from Cargo.toml
	$(RUN) $(RAKE) version:sync

# The binary's --version comes from CARGO_PKG_VERSION and the gem's version from version.rb, so
# besides the rake task, compare the version cargo itself reads with the one the gem reads
version-check: ## Fail when Cargo.toml and lib/sonicop/version.rb disagree
	$(RUN) $(RAKE) version:check
	@cargo_version=$$($(RUN) cargo metadata --no-deps --format-version 1 | $(RUN) ruby -rjson -e 'print JSON.parse($$stdin.read).fetch("packages").find { |p| p["name"] == "$(BINARY_NAME)" }&.fetch("version")'); \
	gem_version=$$($(RUN) ruby -Ilib -rsonicop/version -e 'print Sonicop::VERSION'); \
	echo "Cargo.toml=$$cargo_version lib/sonicop/version.rb=$$gem_version"; \
	test -n "$$cargo_version" && test "$$cargo_version" = "$$gem_version"

## Gem

gem: ## Build the source gem
	$(RUN) $(RAKE) gem

# The source gem compiles the executable with Cargo when it is installed, which is what a platform
# without a prebuilt gem gets. Install it into tmp/gems and run the installed command
gem-check: gem ## Build, install and run the source gem (a release build; no changes)
	@version=$$($(RUN) ruby -Ilib -rsonicop/version -e 'print Sonicop::VERSION'); \
	$(RUN) gem install "./$(BINARY_NAME)-$$version.gem" --install-dir tmp/gems --no-document && \
	GEM_HOME="$(CURDIR)/tmp/gems" GEM_PATH="$(CURDIR)/tmp/gems" $(RUN) tmp/gems/bin/$(BINARY_NAME) --version

gem-platform: release ## Build a platform gem (set GEM_PLATFORM, for example arm64-darwin)
	@test -n "$(GEM_PLATFORM)" || { echo "Set GEM_PLATFORM (for example make gem-platform GEM_PLATFORM=arm64-darwin)" >&2; exit 1; }
	$(RUN) $(RAKE) gem:platform BINARY="target/release/$(BINARY_NAME)" GEM_PLATFORM="$(GEM_PLATFORM)"

## Maintenance

# Which cops lack a regression test. Reading the test source cannot answer it (cop names are
# passed through consts), so the test run records every cop it reaches
cop-coverage: ## Count the cops the hand-written tests reach
	@rm -f target/cop-coverage.tsv
	@SONICOP_COP_COVERAGE=$(CURDIR)/target/cop-coverage.tsv \
		$(RUN) cargo test $(CARGO_FLAGS) --test cops >/dev/null
	@cut -f1 target/cop-coverage.tsv | sort -u | wc -l | xargs echo "Cops the tests reach:"
	@awk -F'\t' '$$2=="positive"' target/cop-coverage.tsv | cut -f1 | sort -u | wc -l \
		| xargs echo "  of which check an offense:"
	@awk -F'\t' '$$2=="correction"' target/cop-coverage.tsv | cut -f1 | sort -u | wc -l \
		| xargs echo "  of which check an autocorrect:"

spec-capture: ## Record the input upstream's specs pass to expect_offense (needs upstream's development gems)
	@test -n "$(UPSTREAM_SPEC_TREE)" && test -n "$(SPEC_CAPTURE_HOOK)" || { \
		echo "Set UPSTREAM_SPEC_TREE (a rubocop v1.89.0 checkout) and SPEC_CAPTURE_HOOK (spec_capture.rb)" >&2; exit 1; }
	cd "$(UPSTREAM_SPEC_TREE)" && CAPTURE_OUT="$(CURDIR)/$(SPEC_CAPTURE_OUT)" \
		ruby -e 'require "rspec/core"; exit RSpec::Core::Runner.run(ARGV)' -- \
		--require "$(abspath $(SPEC_CAPTURE_HOOK))" spec/rubocop/cop

# Re-record the expectations: upstream's own spec cases are the input, and what RuboCop 1.89.0
# really reports for them is the expectation. It starts upstream once per cop, so it takes one to
# two hours. The recording is committed under tests/fixtures/ and make test only reads it, so the
# tests need no rubocop gem. Run it only when upstream is upgraded
spec-fixtures: ## Regenerate the expectations of upstream's spec cases from the recording (needs the rubocop gem; about 2 hours)
	@test -n "$(SPEC_FIXTURE_GEN)" || { echo "Set SPEC_FIXTURE_GEN (spec_fixture_gen.py)" >&2; exit 1; }
	cd "$(dir $(SPEC_FIXTURE_GEN))" && python3 "$(notdir $(SPEC_FIXTURE_GEN))" \
		--from-capture "$(CURDIR)/$(SPEC_CAPTURE_OUT)" --all --out "$(CURDIR)/tests/fixtures"

## Install

# Replace the binary through a temporary file and a rename instead of copying over it. macOS
# caches the code signature check per inode, so a binary copied over one that is running (or ran
# a moment ago) is killed with SIGKILL right after it starts (exit 137). The temporary file sits
# in the same directory so that the rename swaps the inode.
install: release ## Install the release binary to INSTALL_PATH (default /usr/local/bin)
	@mkdir -p "$(INSTALL_PATH)"
	cp "target/release/$(BINARY_NAME)" "$(INSTALL_PATH)/$(BINARY_NAME).new"
	mv -f "$(INSTALL_PATH)/$(BINARY_NAME).new" "$(INSTALL_PATH)/$(BINARY_NAME)"

uninstall: ## Remove the binary from INSTALL_PATH
	rm -f "$(INSTALL_PATH)/$(BINARY_NAME)"

clean: ## Remove build artifacts (Cargo and the gems)
	$(RUN) cargo clean
	rm -f libexec/$(BINARY_NAME) libexec/$(BINARY_NAME).exe
	rm -f $(BINARY_NAME)-*.gem
	rm -rf tmp/gems

## Help

help: ## Show this help
	@echo "Development tasks for $(BINARY_NAME)"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'
	@echo ""
	@echo "Tool versions are pinned in mise.toml. Run make setup first."
	@echo "Release: GitHub Actions > Release > Run workflow"
