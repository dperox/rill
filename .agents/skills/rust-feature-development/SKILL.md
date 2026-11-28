---
name: rust-feature-development
description: Use when implementing or reviewing Rust changes in Japka, including request dispatch, static file responses, upstream routing, and transport behavior.
---

Japka is a Rust 2024 binary running on Tokio. Cargo.toml declares no minimum Rust version or package feature flags. CI exercises stable Rust on Linux and macOS.

1. Identify the operator-visible success and failure cases in [the feature guide](../../../docs/features.md). Use the [acceptance workflow](../japka-feature/SKILL.md) to select a live check.
2. Trace the request from src/main.rs. Keep listener and router wiring there. Put file response and validator semantics in src/static_io/response.rs, blocking file work in src/static_io.rs, and asset caching in src/static_io/cache.rs. Configuration discovery and review belong in src/config_review.rs and its catalog. Use checks/config_review.py for the live approval and rollback workflow. Route selection belongs in src/routing.rs and backend connections in src/upstream.rs. Consult the [transport skill](../japka-transport/SKILL.md) for upgrades, direct writes or TLS.
3. Keep cache and file reads behind the static_io module. Share response conditions through StaticConditions so cached and streamed responses evaluate the same validators. Rust checks visibility. Review dependency direction when adding imports.
4. Preserve request and error behavior at the dispatch boundary. Test invalid inputs as well as successes. Settings read from the environment configure the running binary.
5. Run cargo fmt --check, cargo clippy --all-targets --locked -- -D warnings, and cargo test --locked. Build with cargo build --locked before socket checks. For static responses run python3 checks/static_sizes.py and python3 checks/static_updates.py. See [the test guide](../../../docs/testing.md) for cache, transport and HTTP replay coverage.
