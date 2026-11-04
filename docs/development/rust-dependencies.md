# Rust dependency updates

The Rust workspace uses Rust 1.95.0, selected by `rust-toolchain.toml`. The
resolved dependency graph is committed in `Cargo.lock`; CI and container builds
use `--locked` so they fail rather than resolving a different graph.

To update Rust dependencies intentionally:

1. Use the toolchain selected by `rust-toolchain.toml`.
2. Run `cargo update -p <crate> --precise <version>` for a targeted update, or
   `cargo generate-lockfile` only when deliberately refreshing the complete graph.
3. Review `Cargo.lock`, including transitive version changes.
4. Run `cargo build --workspace --locked`, `cargo fmt --check`,
   `cargo clippy --workspace --all-targets --locked -- -D warnings`, and
   `cargo test --workspace --locked`.
5. Commit manifest and lockfile changes together when a manifest changed.

Do not hand-edit `Cargo.lock` or use an unlocked CI/container build as a way to
refresh it.
