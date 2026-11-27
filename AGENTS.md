# Working on Rill

Build and test from the Linux checkout. The binary is `rill`. The Python checks start real listeners and accept `RILL` as the path to the binary under test. Use an absolute path when invoking a check from another directory.

Start with the [feature guide](docs/features.md) for a change in observable behavior. It connects each feature to its operator documentation and executable acceptance checks. Keep examples, tests and implementation in the same change. For a bug fix, an existing feature entry and a regression case can be enough.

## Repo skills

- [Rust feature development](.agents/skills/rust-feature-development/SKILL.md) for module ownership and verification.
- [Develop a Rill feature](.agents/skills/rill-feature/SKILL.md) for defining an operator-visible outcome, selecting the relevant entry point, and checking it end to end.
- [Maintain Rill transports](.agents/skills/rill-transport/SKILL.md) for work on upgrades, connection lifetime, buffering, sendfile, and the pinned Hyper fork.
- [Create a Rust repository skill](.agents/skills/rust-repo-skill-creator/SKILL.md) for documenting repeatable development workflows.

Run `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, and `cargo test --locked` for Rust changes. Build the binary before running a socket check. The [test guide](docs/testing.md) lists the commands, prerequisites and additional suites. Benchmark results belong with the workload, CPU placement, payload size and revision used to obtain them.
