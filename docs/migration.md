# Migrating an NGINX configuration

Generate a dump on the NGINX host using the same binary, prefix and configuration file as the running deployment. Capture stdout separately from NGINX diagnostics. The dump may contain credentials or private header values; keep it local.

```sh
umask 077
nginx -T > nginx.dump && japka migrate nginx nginx.dump > migration.json
```

Japka also accepts the dump on stdin and reads included content from the dump itself. It writes one JSON report to stdout. Review and deploy the candidate in separate steps. Check the diagnostics for directives that need to be configured separately, such as authentication or request body size limits on a translated proxy route.

## Contract for automation

| Exit | Command and status | Action |
| --- | --- | --- |
| 0 | `migrate nginx` reports `translated` | Inspect the candidate and run preflight (only emitted when no `server` blocks require review). |
| 2 | `migrate nginx` reports `review_required` | Retain the report. Resolve every behavioral difference and unsupported directive before cutover (expected for any dump containing `server` blocks). |
| 1 | `migrate nginx` reports `error` | Correct the input. No candidate was produced. |
| 0 / 1 | `config test` reports `valid` / `invalid` | Read diagnostics on failure. |
| 1 | `cli` reports `error` (`invalid_arguments`) | Check CLI syntax with `japka --help` or version with `japka --version`. |

Migration, preflight and CLI argument errors use the [version 1 report schema](cli-report-v1.schema.json). Read the exit status, then `schema_version` and the diagnostic `code`. A field-specific preflight error can also include `path`. Migration diagnostics identify the source file, line and directive context. For example, `authentication_required` calls for reviewing access control, and `proxy_uri_replacement` flags a difference in the path sent to an upstream.

Run `japka config schema` for the accepted JSON file format. For a diff against an existing deployment under the same startup environment, use the [configuration review workflow](config-review.md).

`candidate.config` is the Japka JSON object. `candidate.environment` contains literal string settings to pass through the process environment. For example, after review:

```python
import json
import os
import subprocess
from pathlib import Path

report = json.loads(Path("migration.json").read_text())
assert report["schema_version"] == 1
candidate = report["candidate"]
path = Path("japka.candidate.json").resolve()
path.write_text(json.dumps(candidate["config"], indent=2))
env = {**os.environ, **candidate["environment"], "JAPKA_CONFIG": str(path)}
subprocess.run(["japka", "config", "test"], env=env, check=True)
```

Run this with the deployment's working directory, default `DOCUMENT_ROOT`, readable certificate files and intended environment. Preflight uses the server's configuration loaders, checks site directories, constructs TLS and upstream configurations, parses stream listeners, and resolves HTTP listener addresses. It does not bind a port, send health probes, or prove backend reachability. An occupied port can pass preflight.

## Translation coverage

The importer recognizes HTTP server blocks, exact DNS names, one compatible plaintext listen address, absolute server-level roots, slash-terminated prefix locations, URI-preserving HTTP(S) proxy targets and simple unweighted upstream groups. It expands exact includes and single-asterisk filename globs found in a dump. For raw input without section markers, diagnostics identify the supplied path or `<stdin>`. An unmatched include needs review, even if the original glob intentionally matched nothing.

TLS directives, inheritance, variables, regex locations, rewrites, header overrides, authentication, FastCGI, custom limits, and other unhandled directives receive diagnostics. For example, the Japka server supports SNI, but this importer leaves selection of a default certificate and named chains to the operator. Unknown-host behavior, upstream headers and buffering also need comparison. Keep NGINX running while testing the candidate against representative requests, including failed logins, missing files, unknown hosts and requests with query strings.
