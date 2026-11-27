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

The previous Hyper fork is documented at revision `351ace3`. See the
[shared-port measurements](../bench/http-kit-static.md) and
[static file settings](static-io.md).
