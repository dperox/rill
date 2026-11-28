# Feature lifecycle

## Intent and design

For nontrivial changes record the outcome, owning slice, examples/acceptance criteria, negative cases, compatibility, dependencies and checks. Scale documentation to risk; a one-line fix does not need a new feature specification. For cross-feature work identify the orchestrator and preserve ownership. For libraries define the observable public contract; for systems code define protocol, safety, timing or resource expectations.

Prefer a small behavior test first when practical. Alternatively begin with a type/API design, compiler-guided changes, or a characterization test for existing behavior. Keep changes reviewable and runnable. Do not intentionally leave compilation broken at handoff. Treat the compiler as a design aid, not a substitute for behavior verification. Do not introduce `todo!()` or fake implementations to satisfy acceptance checks.

## Implementation order

1. Locate the feature facade and adjacent tests; inspect callers and existing conventions.
2. Model relevant input, output, states, invariant and failures. Choose where validation and external effects occur.
3. Implement a single end-to-end use case or coherent capability, with tests for success and failure. Use private helpers rather than prematurely introducing layers.
4. Wire through existing adapters and composition root. Preserve backward compatibility or explicitly document approved breaking changes and migration.
5. Run targeted tests, then broader checks based on impact. Review public API exposure, dependency direction, flag matrix, performance and security where relevant.
6. Summarize changes by feature, tests run, remaining risks and any deliberate exceptions.

## AI-specific guardrails

- Read local instructions and code before writing; no speculative repo-wide refactor.
- Do not duplicate a feature because search was incomplete. Prefer `rg` or available repository search.
- Never broaden visibility, weaken types, add global state, suppress lints or change test expectations merely to fix a compiler error.
- Do not introduce dependencies, network side effects, unsafe code or migrations without appropriate scope and rationale.
- Keep unrelated changes out of the diff; preserve existing uncommitted work.
- For new `unsafe`, document the safety invariant and audit call sites. Add concurrency, cancellation, FFI, fuzzing or performance checks when relevant rather than universally.
- When architecture conflicts with an existing repo, report and propose incremental migration rather than inventing a new standard silently.

## Feature brief

```markdown
# Feature: <outcome>
Owner / public API:
User or caller value:
Acceptance examples (including failures):
Domain states and invariants:
Affected paths / dependencies:
Compatibility, migration and rollout:
Verification commands:
Out of scope:
```
