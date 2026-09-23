.PHONY: build release install test check fmt lint clean benchmark hooks

build:
	cargo build --workspace

release:
	cargo build --workspace --release

install: release
	cp target/release/zgrep ~/.cargo/bin/zgrep

test:
	cargo test --workspace

check:
	cargo fmt --all -- --check
	cargo clippy --workspace -- -D warnings

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace -- -D warnings

clean:
	cargo clean
	rm -rf .zgrep/

benchmark:
	./scripts/benchmark.sh --skip-build $(BENCH_ARGS)

hooks:
	cp scripts/hooks/pre-commit .git/hooks/pre-commit
	chmod +x .git/hooks/pre-commit
	@echo "Git hooks installed"
