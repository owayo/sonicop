.PHONY: build release install clean test test-ruby fmt check gem gem-platform version-sync version-check spec-fixtures cop-coverage help

.DEFAULT_GOAL := help

BINARY_NAME := sonicop
INSTALL_PATH ?= /usr/local/bin
RAKE ?= rake

# 入口はこの Makefile に一本化する。gem 配布とバージョン整合の実装は Rakefile 側にあり、
# ここからは rake タスクとして呼び出すだけにして、同じ処理を二重定義しない。

build: ## デバッグ版をビルド
	cargo build --locked

release: ## 最適化済みネイティブ実行ファイルをビルド
	cargo build --release --locked

install: release ## 実行ファイルをインストール
	install -m 755 target/release/$(BINARY_NAME) $(INSTALL_PATH)/$(BINARY_NAME)

# `--no-fail-fast` は途中で失敗してもすべてのテストバイナリを実行する。指定しない場合、
# cargo は最初の失敗で停止して `tests/cops.rs` 以降を隠すため、「1 件失敗」が実際には
# 「少なくとも 1 件失敗」を意味してしまう。2026-08-17 の実測では、後続も走らせると
# 1 件と報告された失敗が 3 件になった。
test: test-ruby ## Rust と Ruby ラッパーのテストを実行
	cargo test --all-targets --locked --no-fail-fast

test-ruby: ## Ruby ラッパーのテストだけを実行
	$(RAKE) test:ruby

fmt: ## Rust ソースを整形
	cargo fmt --all

version-sync: ## Cargo.toml から lib/sonicop/version.rb を再生成
	$(RAKE) version:sync

version-check: ## Cargo.toml と lib/sonicop/version.rb の不一致を検出
	$(RAKE) version:check

check: version-check ## ローカルの品質ゲートをすべて実行
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features --locked -- -D warnings
	cargo test --all-targets --locked --no-fail-fast
	$(RAKE) test:ruby

# 本家の spec を入力に、本家 1.89.0 の実出力を期待値として録り直す。**本家を cop の数だけ
# 起動するので 1 時間前後かかる。**録った結果は tests/fixtures/ にコミットされ、`make test` は
# それを読むだけなので rubocop gem を要らない。本家を上げたときだけ回す。
SPEC_FIXTURE_GEN ?= $(HOME)/.claude/skills/migrate-rubocop/scripts/spec_fixture_gen.py
# 入力の集め方は 2 段になっている。まず本家の spec を**走らせて** `expect_offense` に渡った
# ものを録り (`spec-capture`)、その記録に対して本家 gem を回して期待値を採る
# (`spec-fixtures`)。spec を字面で読むだけでは `shared_examples` も `#{}` も `context` の
# `cop_config` も復元できず、609 cop のうち 54 cop が丸ごと届かなかった。
SPEC_CAPTURE_HOOK ?= $(HOME)/.claude/skills/migrate-rubocop/scripts/spec_capture.rb
UPSTREAM_SPEC_TREE ?= $(HOME)/tmp/rubocop-v1.89.0/rubocop_rubocop
SPEC_CAPTURE_OUT ?= tests/fixtures/upstream_spec_capture.jsonl

spec-capture: ## 本家 spec が `expect_offense` に渡す入力を記録（開発用 bundle が必要）
	cd $(UPSTREAM_SPEC_TREE) && CAPTURE_OUT=$(CURDIR)/$(SPEC_CAPTURE_OUT) \
		ruby -e 'require "rspec/core"; exit RSpec::Core::Runner.run(ARGV)' -- \
		--require $(SPEC_CAPTURE_HOOK) spec/rubocop/cop

spec-fixtures: ## 記録から本家 spec の期待値を再生成（rubocop gem が必要、約 2 時間）
	cd $(dir $(SPEC_FIXTURE_GEN)) && python3 $(notdir $(SPEC_FIXTURE_GEN)) \
		--from-capture $(CURDIR)/$(SPEC_CAPTURE_OUT) --all --out $(CURDIR)/tests/fixtures

# どの cop に回帰テストが無いかを数える。テスト本文を静的に読んでも分からない
# (cop 名を const 経由で渡す書き方が混ざる) ので、実行時に記録する。
cop-coverage: ## 手書きテストが到達する Cop を集計
	@rm -f target/cop-coverage.tsv
	@SONICOP_COP_COVERAGE=$(CURDIR)/target/cop-coverage.tsv \
		cargo test --test cops --locked >/dev/null
	@cut -f1 target/cop-coverage.tsv | sort -u | wc -l | xargs echo "検証に到達した cop:"
	@awk -F'\t' '$$2=="positive"' target/cop-coverage.tsv | cut -f1 | sort -u | wc -l \
		| xargs echo "  うち offense 検出を検証:"
	@awk -F'\t' '$$2=="correction"' target/cop-coverage.tsv | cut -f1 | sort -u | wc -l \
		| xargs echo "  うち autocorrect を検証:"

gem: ## source gem をビルド
	$(RAKE) gem

gem-platform: release ## platform gem をビルド（GEM_PLATFORM を指定）
	test -n "$(GEM_PLATFORM)"
	$(RAKE) gem:platform BINARY="target/release/$(BINARY_NAME)" GEM_PLATFORM="$(GEM_PLATFORM)"

clean: ## Cargo と gem のビルド成果物を削除
	cargo clean
	rm -f libexec/$(BINARY_NAME) libexec/$(BINARY_NAME).exe
	rm -f $(BINARY_NAME)-*.gem

help: ## 利用可能なターゲットを表示
	@echo "$(BINARY_NAME) build commands"
	@echo ""
	@awk 'BEGIN {FS = ":.*?## "}; /^[a-zA-Z_-]+:.*?## / {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
