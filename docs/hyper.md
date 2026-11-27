# HTTP transports

Rill uses the crates.io release of Hyper for upstream requests, TLS HTTP/1 and
HTTP/2. On the main plaintext port, the direct-file configuration selects
http-kit-h1 for HTTP/1 and stock Hyper for a prior-knowledge HTTP/2 connection.
Compression or disabled direct file delivery selects the ordinary Axum listener.

A complete small GET or HEAD uses the published HTTP/1 parser and response
encoder. The connection waits for the next request with a small receive buffer.
Partial heads, request bodies and upgrades pass their original bytes to
http-kit’s streaming connection driver. That driver handles subsequent requests
on the same socket and retains its own connection state. The low idle-RSS
measurement applies to connections using the compact path.

Both drivers route each request through the same Axum application. An eligible
static response carries an open file region. Headers precede the transfer,
and a short file ends the connection. Windows uses two bounded TransmitFile
workers and falls back to buffered transmission when they are busy. Linux
limits individual sendfile calls to 1 MiB and falls back to positional reads on
unsupported filesystems. Upgrades transfer any read-ahead bytes to the tunnel
and hold the upstream lease until relay completion.

## Linux HTTP worker scheduling

When Rill runs a single plaintext direct-file listener, each network worker can
accept from the same listening socket and drive I/O for its own connections.
A client stays on that worker for the lifetime of its connection. Routes,
health checks and application caches are shared across workers.

For a two-CPU Linux pod behind an ingress, setting `TOKIO_WORKER_THREADS=2`
and `RILL_BLOCKING_THREADS=2` enables this layout on the single plaintext
direct-file listener. Rill splits the blocking budget between the two workers.

With more than two network workers, Rill also uses this layout for a static
server that has at least one blocking worker per network worker, no upstream
routes, and no `RILL_CONFIG` file that could add routes on reload. Larger
proxy deployments use the shared runtime. Compression, additional TLS,
stream or static listeners, and a smaller blocking budget also select the
shared runtime.
[Scheduling measurements](../bench/kubernetes/README.md#linux-worker-scheduling)

See the [Rill vs. NGINX benchmarks](../bench/http-kit-static.md) and
[static file settings](static-io.md).
