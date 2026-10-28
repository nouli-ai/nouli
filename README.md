# nouli

A minimal Rust harness for decision models.

Development requires Rust 1.99.1 with Cargo, rustfmt, and Clippy. Rustup reads
the pinned toolchain from `rust-toolchain.toml`. No credentials or external
services are needed for the workspace checks:

```sh
cargo build --workspace --locked
cargo fmt --all
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run -p nouli-cli --locked -- --help
```

`nouli-core` contains reusable engine behavior; `nouli-cli` builds the `nouli`
executable and owns input, output, and process exits. The initial workspace
supports help and version reporting; decision execution is under development.

