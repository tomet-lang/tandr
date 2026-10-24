# tandr development tasks

default:
    @just --list

build:
    cargo build --workspace

test:
    cargo test --workspace

check:
    cargo check --workspace

run-example:
    cargo run -p tandr-cli -- check-rules examples/rules.tandr

run-example-diff:
    cargo run -p tandr-cli -- run -r examples/rules.tandr examples/sample.tmt --diff --force
