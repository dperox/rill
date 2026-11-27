# Architecture decision guide

## Profiles

| Profile | Start with | Promote when |
|---|---|---|
| Small CLI/service | One package with `lib.rs` and thin `main.rs`; capability modules | Distinct dependencies, targets, reuse, or ownership justify crates |
| Large service | Feature modules; explicit composition root; transport adapters | Separate deployable packages or hard dependency boundaries are useful |
| Library/SDK | Public capability modules and private implementation; API contract | Independent publishability, optional heavy dependency or separate target |
| Systems/embedded/WASM | Subsystem or protocol modules; explicit platform/unsafe boundaries | Host/target, `no_std`, FFI or dependency constraints require isolation |
| Existing workspace | Respect established member graph; slice within each appropriate member | Evidence shows current boundaries cause meaningful coupling or build issues |

## Example: application feature module

```text
src/
  main.rs
  lib.rs
  orders.rs                # facade; could be orders/mod.rs instead
  orders/
    model.rs
    place.rs
    cancel.rs
    persistence.rs          # only if justified
  transport/
    http.rs
  bootstrap.rs
 tests/
  orders.rs
```

Use `orders.rs` to declare private children and re-export only the supported API. Within a feature, `pub(super)` may permit sibling access via the parent; `pub(crate)` is crate-wide and is not feature-private. A `pub mod` chain can expose internals; a narrow `pub use` keeps implementation paths private. A facade may itself be `pub(crate)` for an internal binary, or `pub` when accessed by the separate binary target or external users. `main.rs` and `lib.rs` are separate crates even in the same package. Integration tests outside the library use its public API. Use `pub(in crate::...)` only for an ancestor module scope. Rust module privacy does not automatically enforce the direction of all intra-crate dependencies; add a real check or review if needed.

## Crate boundaries

```text
Cargo.toml
crates/
  runtime/                  # core runtime and feature slices
  protocol/                 # stable wire contracts, if independently useful
  adapter-http/             # if dependencies or deployment justify it
  server/                   # binary/composition root
```

A sample allowed direction: `server -> adapter-http -> runtime -> protocol`, and `server -> runtime`; actual direction depends on ownership of ports and contracts. The core must not import the outer adapter. Cargo rejects cycles in normal crate dependencies, but doesn't enforce your desired layering by itself. A workspace is organizational, not a requirement for FDD. More crates may improve incremental rebuild isolation or parallelism, but can also add build overhead and API friction; measure before splitting.

## Type design

- Private-field newtypes with validated constructors for domain invariants; consider deserialization and other construction paths too.
- Enums for mutually exclusive states; exhaustive matches where future variants should force review; use non-exhaustive public enums when API evolution requires it.
- `Result<T, E>` for recoverable errors; typed domain errors when callers act differently. Map to HTTP/CLI errors at the boundary. Do not hide failure with defaults or unchecked unwraps.
- Typestate when a protocol has materially different legal operations per state; avoid gratuitous generic state machines.
- Ownership and borrowing should express real lifetimes, not optimize prematurely. Clone, `Arc`, locks, `unsafe`, `Send`/`Sync` and async runtime choices require reasons, not blanket bans.
- Prefer concrete types and functions; introduce traits for multiple implementations, external effects or a justified stable seam. Avoid one trait per struct.
- Keep transport DTOs and persistence representations distinct from domain types only where validation, compatibility or coupling merits it.

## Three meanings of feature

| Concept | Mechanism | Example |
|---|---|---|
| User/business capability | Module, crate, use case | Place order |
| Build-time capability | Cargo `[features]`, `cfg` | Optional sqlite backend |
| Runtime rollout | Config/toggle system | Enable new ordering UI for 10% of users |

Cargo flags generally unify across dependency paths. Make them additive; prefer separate types or runtime selection to mutually exclusive Cargo features when practical. Document exceptions and test supported combinations. Never treat a feature flag as authorization.

## Sources

- Rust module privacy: https://doc.rust-lang.org/reference/visibility-and-privacy.html
- Cargo workspaces: https://doc.rust-lang.org/cargo/reference/workspaces.html
- Cargo features: https://doc.rust-lang.org/cargo/reference/features.html
- Rust API guidelines: https://rust-lang.github.io/api-guidelines/type-safety.html
- Vertical slice architecture: https://www.jimmybogard.com/vertical-slice-architecture/
