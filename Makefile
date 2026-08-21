.PHONY: install dev check test build clean

install:
	npm install

dev:
	npm run tauri dev

check:
	npm run check
	cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check

test:
	cargo test --manifest-path src-tauri/Cargo.toml --all-targets

build:
	npm run tauri build

clean:
	rm -rf dist node_modules src-tauri/target
