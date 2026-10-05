.PHONY: build release install install-skill test check fmt lint clean benchmark hooks

build:
	cargo build --workspace --locked

release:
	cargo build --workspace --release --locked

install: release
	mkdir -p ~/.cargo/bin
	install -m 755 target/release/ogrep ~/.cargo/bin/ogrep
	install -m 755 target/release/ogrep-lab ~/.cargo/bin/ogrep-lab

# Agent skill: teaches coding agents to search with ogrep in any project.
SKILLS_DIR ?= ~/.agents/skills
install-skill:
	mkdir -p $(SKILLS_DIR)
	rm -rf $(SKILLS_DIR)/ogrep
	cp -r .claude/skills/ogrep $(SKILLS_DIR)/ogrep

test:
	cargo test --workspace --locked

check:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --locked -- -D warnings

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets --locked -- -D warnings

clean:
	cargo clean
	rm -rf .ogrep/

benchmark:
	./scripts/benchmark.sh $(BENCH_ARGS)

hooks:
	cp scripts/hooks/pre-commit .git/hooks/pre-commit
	chmod +x .git/hooks/pre-commit
	@echo "Git hooks installed"
