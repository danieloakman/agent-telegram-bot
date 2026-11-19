default:
    @just --list

build:
    cargo build

check:
    cargo check

test:
    cargo test

fmt:
    cargo fmt

fmt-check:
    cargo fmt -- --check

clippy:
    cargo clippy --all-targets -- -D warnings

run *args:
    cargo run -- {{args}}
