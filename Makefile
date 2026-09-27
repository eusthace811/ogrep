.PHONY: build release install package test check fmt lint clean benchmark hooks

build:
	cargo build --workspace --locked

release:
	cargo build --workspace --release --locked

package:
	python3 scripts/package.py

install: release
	mkdir -p ~/.cargo/bin
	install -m 755 target/release/ogrep ~/.cargo/bin/ogrep

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
	python3 scripts/evaluate.py $(BENCH_ARGS)

hooks:
	cp scripts/hooks/pre-commit .git/hooks/pre-commit
	chmod +x .git/hooks/pre-commit
	@echo "Git hooks installed"
