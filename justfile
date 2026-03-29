# cargo-read development commands

# Run all checks: format, lint, test
check: fmt clippy test

# Format code
fmt:
    cargo fmt

# Run clippy
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Run tests (unit only, no network)
test:
    cargo test

# Run all tests including network integration tests
test-all:
    cargo test -- --include-ignored

# Local CI sanity check
ci: fmt clippy test

# Check for outdated dependencies
outdated:
    cargo outdated

# Build release binary
build:
    cargo build --release

# Smoke test: download a crate and show output
smoke:
    cargo run -- serde
