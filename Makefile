.PHONY: build release install install-skill test check fmt lint clean benchmark hooks

build:
	cargo build --workspace --locked

release:
	cargo build --workspace --release --locked

install: release
	mkdir -p ~/.cargo/bin
	install -m 755 target/release/ogrep ~/.cargo/bin/ogrep

# Claude Code skill: teaches Claude to search with ogrep in any project.
install-skill:
	mkdir -p ~/.claude/skills
	rm -rf ~/.claude/skills/ogrep
	cp -r .claude/skills/ogrep ~/.claude/skills/ogrep

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
