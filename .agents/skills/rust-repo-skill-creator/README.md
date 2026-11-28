# Rust repo skill creator

A reusable meta-skill for creating a project-specific Rust feature-development skill. Compatible in format with the Agent Skills specification. Installation/discovery locations depend on the coding agent.

For Claude Code, copy this entire `rust-repo-skill-creator` directory under `~/.claude/skills/` for personal use (or `.claude/skills/` inside a project). For other clients use their documented skill directory.

Open an existing Rust repo and ask:

> Use rust-repo-skill-creator in discover mode. Inspect this repository, generate its rust-feature-development skill, and suggest minimal changes to AGENTS.md. Do not restructure application code. Report observed boundaries, assumptions and verification status.

For a greenfield repo:

> Use rust-repo-skill-creator in bootstrap mode. Design the smallest idiomatic Rust architecture for this project, generate a repo-specific feature-development skill, and propose the layout before implementing code.

The package contains instructions and a template, not an automated source rewriter or an architectural linter. It has not been run against a specific Rust repo. A coding agent must inspect the repository, fill the template, and execute the appropriate checks. No application code, dependency or CI changes are required merely to install it.
