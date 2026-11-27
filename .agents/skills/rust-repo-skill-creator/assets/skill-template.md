---
name: rust-feature-development
description: Develop, extend, debug and review Rust features in <repo-name> using its actual module and crate boundaries, typed contracts, tests and compiler-guided iteration. Use for new features, behavior changes, API changes, refactors and Rust architecture reviews in this repository.
---

# Rust feature development in <repo-name>

## Repository map

- Project profile / edition / MSRV: <observed profile>
- Entry points and composition root: <actual paths>
- Feature ownership and public facades: <actual modules/crates and paths>
- Allowed dependency direction: <actual boundary policy>
- Existing flags, targets and test tools: <observed settings>

## Non-negotiable rules

- Locate the owning feature/capability first. Put implementation and tests near the code they exercise; use existing integration-test layout.
- Depend on another feature's designated public API, not its internals. Use the narrowest sufficient visibility. Preserve documented dependency direction.
- Keep entry points and adapters focused on translation/wiring; keep behavior in the owning capability when appropriate.
- Encode meaningful domain invariants through types and validated construction. Prefer concrete functions and types unless abstraction has a demonstrated purpose.
- Avoid unapproved new crates, dependencies, shared layers, global state, broad visibility and unrelated refactors.
- Cargo build features are not business features or runtime rollout toggles. Respect supported feature combinations.

## Feature workflow

1. Read relevant repo instructions, owning module, callers and tests; inspect current diff.
2. State outcome, constraints and acceptance cases. For complex work identify affected public contracts and dependencies.
3. Choose the smallest idiomatic boundary. Model states and errors; add a behavior test or another explicit verification path.
4. Implement the smallest complete feature, resolving compiler feedback without weakening invariants.
5. Wire through existing entry points. Verify failure cases and compatibility.
6. Run the applicable checks below; report exact results, blockers and untested configurations.

## Verification

<Replace with actual repo commands for targeted and full checks, feature matrix, target runners and CI requirements.>

## Architecture enforcement

<Identify compiler-enforced boundaries, real CI checks, and review-only policies separately.>

## References

<Link only existing or generated repo-specific references. Remove this section if none.>
