# HTTP transports

Japka uses the crates.io release of Hyper for upstream requests, TLS HTTP/1 and
HTTP/2. On the main `LISTEN` plaintext port, the direct-file configuration selects
http-kit-h1 for HTTP/1 and stock Hyper for a prior-knowledge HTTP/2 connection.
Compression or disabled direct file delivery selects the ordinary Axum listener
on the main `LISTEN` port (`JAPKA_STATIC_LISTEN` always uses `http-kit-h1` /
prior-knowledge HTTP/2).

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

When Japka runs a single plaintext direct-file listener, each network worker can
accept from the same listening socket and drive I/O for its own connections.
A client stays on that worker for the lifetime of its connection. Routes,
health checks and application caches are shared across workers.

Japka selects worker counts at startup. For a single Linux plaintext listener
with direct file delivery enabled, the defaults depend on the CPU allocation
and workload.

| CPU allocation and workload | Network workers | Maximum blocking workers |
|---|---:|---:|
| Two CPUs of quota shared across a wider set of CPUs, as in the ingress lab | 1 | 2 |
| Pinned to two CPUs, serving files or proxying requests | 2 | 2 |
| Three or four CPUs, serving files without upstream routes or a reloadable config file | One per available CPU | One per network worker |
| Four CPUs, with upstream routes or a reloadable config file | 4 | 1 |

Japka reads the available parallelism and, for the two-CPU decision, the Linux
thread affinity mask. If it cannot read that mask, it uses two network workers.
Other automatic configurations use the available CPU count and one blocking
worker. TLS, compression, disabled direct file delivery and additional
listeners use the general runtime settings.

Set `TOKIO_WORKER_THREADS` or `JAPKA_BLOCKING_THREADS` independently to
override either count. With two or more network workers, the independent
I/O workers need a blocking budget of at least one per network worker.
Larger proxies and file-configured servers with more than two workers use
the shared scheduler, which lets a reload add upstream routes. Japka reports
both worker counts in its startup log.
[Default-setting benchmarks](../bench/kubernetes/README.md#automatic-worker-defaults)
and [native multicore measurements](../bench/http-kit-static.md#linux-worker-scheduling-after-v010).

[Transport dependencies](crates.md) · [Static file settings](static-io.md)
