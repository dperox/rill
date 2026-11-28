# Working on Japka

## Help an operator set up Japka

Find out whether this is a new installation or a change to a running service. Confirm where Japka will run, which hostnames and paths it should handle, which public directories or backends those paths should reach, and what a successful request should return. Ask the operator for any details you cannot infer.

- **New installation.** Follow the [deployment guide](docs/deployment.md) for Cargo, containers, or Kubernetes. Start with the simplest setup that meets the request. A single static site can use `DOCUMENT_ROOT`. If the setup needs a JSON file, draft it separately and point `JAPKA_CONFIG` at that file for preflight.
- **Existing service.** Identify the running version and its configuration inputs. Follow [Navigate configuration](#navigate-configuration) to inspect the baseline, preserve inherited settings, and prepare the proposed change. For an environment-only change, show the variable edits and the required process rollout.
- **Human review.** Show the proposed configuration, deployment commands, checks you ran, and a `curl` request with the expected status and body. Wait for approval before applying changes or starting the service. After deployment, run the request and compare the response with the expected result.

## Navigate configuration

For an operator request, start with the target binary’s `japka --help`. For a deployment plan, use the service’s binary, working directory, startup environment and mounts.

1. **Find the setting.** Run `japka config list --json` for startup variables, accepted values, defaults and aliases. Run `japka config schema` for the JSON accepted by `JAPKA_CONFIG`. For HTTP balancing and health checks, the catalog’s `file_field` points to the reloadable `proxy` field.
2. **Inspect the baseline.** Run `japka config show --json` with the service’s environment. If it uses `JAPKA_CONFIG`, keep that variable pointed at the active file. This resolves inputs from disk and the environment. Check `effective.routes_source` and `effective.routes` before editing a route. An omitted or null `routes` inherits `JAPKA_ROUTES` and `UPSTREAMS`. An explicit list replaces both, including when the list is empty.
3. **Draft and validate.** Copy the active JSON to a sibling candidate so relative site and certificate paths resolve the same way. If the service has no active file, draft a new candidate beside its intended deployment path. Preserve unrelated fields. To add a route to an inherited list, use the effective routes as the starting list. Run `JAPKA_CONFIG="$candidate" japka config test`, with `candidate` set to the candidate’s path. On exit 1, inspect `diagnostics[].message` and the optional `path`, fix the proposal, and rerun.
4. **Prepare the human review.** When an active file exists, run `japka config plan "$JAPKA_CONFIG" "$candidate" --json > plan.json`. For a first file, show the validated candidate and how it changes the effective settings. On a successful plan, show `jq -r '.review[]' plan.json` and save the candidate and plan for that revision. Call out removals, changes from inherited to file-defined settings, and the deployment action. File changes can reload on Unix. Startup environment edits and Windows file changes require a process rollout.
5. **Apply after approval.** Pass the exact reviewed artifacts and approved plan digest to the trusted deploy job. On native Unix, the helper checks the baseline, applies the file and probes the affected route. A failed probe triggers rollback. Regenerate the plan for review if the candidate or baseline changes. [Worked example and deploy procedure](docs/config-review.md)

### Where to look

| Question | Operator guide | Implementation |
| --- | --- | --- |
| Which environment names and defaults does this binary advertise? | [Settings discovery](docs/config-review.md#discover-what-this-binary-accepts) | [Catalog](src/config_review/catalog.rs) · [Legacy alias lookup](src/settings.rs) |
| Which JSON fields load together and what can reload? | [Reload behavior](docs/reload.md) | [FileConfig and generation loader](src/reload.rs) |
| Which host, prefix or backend policy will change? | [Routing and HTTP pool settings](docs/routing.md) | [Route selection](src/routing.rs) · [File and environment precedence for pools](src/routing/config.rs) |
| How are the review and rollout verified? | [Configuration review](docs/config-review.md) | [Plan and effective configuration](src/config_review.rs) · [Unix deploy helper](deploy/apply-config.py) · [Acceptance check](checks/config_review.py) |

## Migrate, deploy and verify

- For NGINX, follow the [migration contract](docs/migration.md). Parse the report on exit 2, resolve the diagnostics, and compare requests on separate listeners before switching traffic.
- For native, container and Kubernetes installations, use the [deployment guide](docs/deployment.md). Run `japka config test` with the service’s environment, working directory and mounts. After startup, check a known URL for its expected status and body.
- For Unix file changes, use the [review workflow](docs/config-review.md) or [manual reload procedure](docs/reload.md). Restart the process for startup environment changes and for file changes on Windows.

## Develop from a source checkout

Build and test from the repository root on Linux or macOS. On Windows hosts, WSL2 is an option for running the Linux workflow. The binary is `japka`. The Python checks start real listeners and accept `JAPKA` as the path to the binary under test. Use an absolute path when invoking a check from another directory.

Start with the [feature guide](docs/features.md) for a change in observable behavior. It connects each feature to its operator documentation and executable acceptance checks. Keep examples, tests and implementation in the same change. For a bug fix, an existing feature entry and a regression case can be enough.

## Repo skills

- [Rust feature development](.agents/skills/rust-feature-development/SKILL.md) for module ownership and verification.
- [Develop a Japka feature](.agents/skills/japka-feature/SKILL.md) for defining an operator-visible outcome, selecting the relevant entry point, and checking it end to end.
- [Maintain Japka transports](.agents/skills/japka-transport/SKILL.md) for work on upgrades, connection lifetime, buffering, sendfile, and the plaintext file-region writer.
- [Create a Rust repository skill](.agents/skills/rust-repo-skill-creator/SKILL.md) for documenting repeatable development workflows.

Run `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, and `cargo test --locked` for Rust changes. Build the binary before running a socket check. The [test guide](docs/testing.md) lists the commands, prerequisites and additional suites. Benchmark results belong with the workload, CPU placement, payload size and revision used to obtain them.
