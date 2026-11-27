# Verification and enforcement

Select commands from actual CI, README, Makefile, justfile, xtask and toolchain. Example commands for a conventional host workspace (not universally applicable):

```sh
cargo metadata --format-version 1 --no-deps
cargo fmt --all -- --check
cargo check -p <package> --all-targets
cargo test -p <package>
cargo clippy -p <package> --all-targets
cargo test --workspace
```

Run only supported commands and preserve package/target/feature settings. Where applicable check default, `--no-default-features`, explicit feature sets and `--all-features` only if supported. `--all-targets` does not mean all platform target triples. Some combinations need separate invocations because Cargo feature unification changes build configuration. Consider `cargo tree -e features` for dependency-feature inspection. For cross-compilation, use the documented target and appropriate runner; `cargo check` is not a substitute for target tests. Adopt `-D warnings`, coverage, nextest, cargo-hack, cargo-deny, cargo-semver-checks, Miri or fuzzing only when the repo uses them or the user approves a change. Do not assume network access, installed components or target support.

## Enforcement ladder

1. Visibility and private fields: compile-time access and construction rules.
2. Typed states and errors: static contracts, not all runtime invariants.
3. Cargo package graph: explicit inter-crate dependencies and cycle detection for normal dependencies. For prescribed direction, compare `cargo metadata` dependency edges against an approved policy; metadata `--no-deps` exposes direct dependency declarations but is not a full resolved graph.
4. Intra-crate module dependency policy: code review or purpose-built AST/name-resolution-aware checker. Do not claim a regex import check is comprehensive (aliases, reexports, macros, and cfg can hide edges).
5. Behavioral tests: success, negative cases and compatibility. Integration tests use public contracts. Compile-fail tests may be appropriate for key static invariants; doc tests can demonstrate public API behavior.
6. CI: use repository-defined gates, not invented green results.

## Skill audit

- Folder name equals frontmatter name; description is specific and valid.
- All relative reference links exist; generated template placeholders resolved.
- Repo paths, entry points, commands and targets confirmed or explicitly marked provisional.
- Distinguish documented policy from enforced policy and executed checks.
- Record command, scope, exit status and blockers. Review diff for accidental edits and untracked files.

Agent Skills format: https://agentskills.io/specification
