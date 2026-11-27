# Working on Rill

Build and test from the repository root on Linux or macOS. On Windows hosts, WSL2 is an option for running the Linux workflow. The binary is `rill`. The Python checks start real listeners and accept `RILL` as the path to the binary under test. Use an absolute path when invoking a check from another directory.

Start with the [feature guide](docs/features.md) for a change in observable behavior. It connects each feature to its operator documentation and executable acceptance checks. Keep examples, tests and implementation in the same change. For a bug fix, an existing feature entry and a regression case can be enough.

## Port an NGINX deployment

1. Capture stdout from `nginx -T` on the source deployment using its active configuration and prefix. Keep the dump local with restricted permissions. It may contain credentials.
2. Run `rill migrate nginx nginx.dump > migration.json`. Parse stdout as JSON even on exit 2. Exit 0 produces a translated candidate, exit 2 produces a candidate requiring review, and exit 1 reports an error. Check `schema_version` and use diagnostic codes, file names, line numbers and contexts to identify affected routes. See the [CLI contract](docs/migration.md) and [JSON Schema](docs/cli-report-v1.schema.json).
3. Write the reviewed `candidate.config` to a new file and pass `candidate.environment` as literal environment entries. Resolve reported differences before switching traffic, particularly authentication, body limits, upstream Host headers and prefix replacement. Point `RILL_CONFIG` at the reviewed file and run `rill config test` with the deployment's working directory, mounts and environment.
4. Compare NGINX and Rill on separate listeners with representative requests. Check redirects, unknown hosts, denied requests, static paths and what the backend receives for a proxied path with a query string. Retain the original configuration and traffic target for rollback.

## Deploy and verify Rill

- **Native Linux or macOS.** From the repo root, run `cargo build --release --locked`. Use `./target/release/rill config test` for preflight, then start the same binary with the same environment. Check a known URL for its expected status and body.
- **Containers.** Build the [Dockerfile](deploy/Dockerfile) from the repo root. Run the image with `config test` as its arguments and the same read-only mounts and environment intended for service. Site roots and key files must be readable by UID 10001. For Docker commands and volume layouts, use the [deployment guide](docs/deployment.md).
- **Kubernetes.** Check the intended context and namespace. Make the image available to the nodes, then apply the [manifest](deploy/kubernetes.yaml). Wait for `kubectl rollout status deployment/rill`, verify both replicas are ready, and test an HTTP response through the Service. The base manifest exposes HTTP on 8080. For HTTP/3, verify the TLS port over UDP as well as TCP.
- **Configuration updates.** On Unix, publish routes, site roots and certificates together using the [reload procedure](docs/reload.md). Check the acceptance or rejection message and exercise the affected host on a fresh connection. Roll out a new process for startup settings such as listeners and access logging.

## Repo skills

- [Rust feature development](.agents/skills/rust-feature-development/SKILL.md) for module ownership and verification.
- [Develop a Rill feature](.agents/skills/rill-feature/SKILL.md) for defining an operator-visible outcome, selecting the relevant entry point, and checking it end to end.
- [Maintain Rill transports](.agents/skills/rill-transport/SKILL.md) for work on upgrades, connection lifetime, buffering, sendfile, and the pinned Hyper fork.
- [Create a Rust repository skill](.agents/skills/rust-repo-skill-creator/SKILL.md) for documenting repeatable development workflows.

Run `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, and `cargo test --locked` for Rust changes. Build the binary before running a socket check. The [test guide](docs/testing.md) lists the commands, prerequisites and additional suites. Benchmark results belong with the workload, CPU placement, payload size and revision used to obtain them.
