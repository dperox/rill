---
name: rill-feature
description: Develop an operator-visible Rill feature from a concrete scenario through a live acceptance check and operator documentation.
---

Read the root AGENTS.md and docs/features.md. Identify who performs the action, how they invoke it in a real deployment, and what the client or operator observes. For an existing feature, extend its contract and existing check where possible.

Choose the acceptance boundary from the behavior. Use Rust unit tests for isolated state transitions. Use a loopback process in checks/ for configuration, routing, or client-visible behavior. Use checks/compare.py for the recorded HTTP contract. Edit a recorded expectation only after explaining the intended change in behavior.

Write down the success case and the most relevant failure before changing the implementation. For a reload, keep traffic active across publication and submit an invalid candidate afterwards. For a proxy feature, check what both client and upstream see. Prefer temporary fixtures, bounded readiness polling and dynamically allocated ports.

Run the new check against the built Rill binary. Update the operator guide with a runnable invocation, supported scope and failure behavior. Link a new feature’s contract to its test from docs/features.md.
