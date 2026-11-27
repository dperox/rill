# Migrating an NGINX configuration

Generate a dump on the NGINX host using the same binary, prefix and configuration file as the running deployment. Capture stdout separately from NGINX diagnostics. The dump may contain credentials or private header values; keep it local.

```sh
umask 077
nginx -T > nginx.dump && rill migrate nginx nginx.dump > migration.json
```

Rill also accepts the dump on stdin. It reads included content from the dump itself. It writes one JSON object to stdout and makes no configuration changes. The report is a draft for the operator. In particular, a proxy route in the candidate may still have an unsupported authentication or body-size directive in the source.

## Contract for automation

| Exit | Command and status | Action |
| --- | --- | --- |
| 0 | `migrate nginx` reports `translated` | Inspect the candidate and run preflight (only emitted when no `server` blocks require review). |
| 2 | `migrate nginx` reports `review_required` | Retain the report. Resolve every behavioral difference and unsupported directive before cutover (expected for any dump containing `server` blocks). |
| 1 | `migrate nginx` reports `error` | Correct the input. No candidate was produced. |
| 0 / 1 | `config test` reports `valid` / `invalid` | Read diagnostics on failure. |
| 1 | `cli` reports `error` (`invalid_arguments`) | Check CLI syntax with `rill --help` or version with `rill --version`. |

The [version 1 JSON Schema](cli-report-v1.schema.json) describes the report for machine validation. All JSON-emitting CLI commands (`migrate nginx`, `config test`, and invalid CLI invocations reporting `command: "cli"`) emit `schema_version: 1`, `command`, `status`, and a `diagnostics` array. Branch on the exit code, then check the schema version and diagnostic `code`. Messages are for people. Migration records for directives include the source `file`, one-based `line`, enclosing `context`, directive name and classification. For example, an auth subrequest produces `authentication_required`; a URI on `proxy_pass` produces `proxy_uri_replacement`. Every translated virtual server also has a review record for NGINX defaults and listener scope, so real server migrations exit 2 (`review_required`). Use `rill --help` (`-h`) for usage and `rill --version` (`-V`) for the binary version.

`candidate.config` is the Rill JSON object. `candidate.environment` contains literal string settings to pass through the process environment. For example, after review:

```python
import json
import os
import subprocess
from pathlib import Path

report = json.loads(Path("migration.json").read_text())
assert report["schema_version"] == 1
candidate = report["candidate"]
path = Path("rill.candidate.json").resolve()
path.write_text(json.dumps(candidate["config"], indent=2))
env = {**os.environ, **candidate["environment"], "RILL_CONFIG": str(path)}
subprocess.run(["rill", "config", "test"], env=env, check=True)
```

Run this with the deployment's working directory, default `DOCUMENT_ROOT`, readable certificate files and intended environment. Preflight uses the server's configuration loaders, checks site directories, constructs TLS and upstream configurations, parses stream listeners, and resolves HTTP listener addresses. It does not bind a port, send health probes, or prove backend reachability. An occupied port can pass preflight.

## Translation coverage

The importer recognizes HTTP server blocks, exact DNS names, one compatible plaintext listen address, absolute server-level roots, slash-terminated prefix locations, URI-preserving HTTP(S) proxy targets and simple unweighted upstream groups. It expands exact includes and single-asterisk filename globs found in a dump. For raw input without section markers, diagnostics identify the supplied path or `<stdin>`. An unmatched include needs review, even if the original glob intentionally matched nothing.

TLS directives, inheritance, variables, regex locations, rewrites, header overrides, authentication, FastCGI, custom limits, and other unhandled directives receive diagnostics. For example, the Rill server supports SNI, but this importer leaves selection of a default certificate and named chains to the operator. Unknown-host behavior, upstream headers and buffering also need comparison. Keep NGINX running while testing the candidate against representative requests, including failed logins, missing files, unknown hosts and requests with query strings.
