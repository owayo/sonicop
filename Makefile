.PHONY: build release install clean test test-ruby fmt check gem gem-platform version-sync version-check spec-fixtures cop-coverage help

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

# 本家の spec を入力に、本家 1.89.0 の実出力を期待値として録り直す。**本家を cop の数だけ
# 起動するので 1 時間前後かかる。**録った結果は tests/fixtures/ にコミットされ、`make test` は
# それを読むだけなので rubocop gem を要らない。本家を上げたときだけ回す。
SPEC_FIXTURE_GEN ?= $(HOME)/.claude/skills/migrate-rubocop/scripts/spec_fixture_gen.py

spec-fixtures: ## Re-record upstream spec expectations (needs the rubocop gem; ~1h)
	cd $(dir $(SPEC_FIXTURE_GEN)) && python3 $(notdir $(SPEC_FIXTURE_GEN)) \
		--all --out $(CURDIR)/tests/fixtures

# どの cop に回帰テストが無いかを数える。テスト本文を静的に読んでも分からない
# (cop 名を const 経由で渡す書き方が混ざる) ので、実行時に記録する。
cop-coverage: ## Count which cops the hand-written tests actually reach
	@rm -f target/cop-coverage.tsv
	@SONICOP_COP_COVERAGE=$(CURDIR)/target/cop-coverage.tsv \
		cargo test --test cops --locked >/dev/null
	@cut -f1 target/cop-coverage.tsv | sort -u | wc -l | xargs echo "検証に到達した cop:"
	@awk -F'\t' '$$2=="positive"' target/cop-coverage.tsv | cut -f1 | sort -u | wc -l \
		| xargs echo "  うち offense 検出を検証:"
	@awk -F'\t' '$$2=="correction"' target/cop-coverage.tsv | cut -f1 | sort -u | wc -l \
		| xargs echo "  うち autocorrect を検証:"

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
