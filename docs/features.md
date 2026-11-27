# Feature development

Describe a feature in terms of what a client or operator can do. A proposal can start as a [feature request](../.github/ISSUE_TEMPLATE/feature.md). Include one example of success and one failure the implementation must handle. Deliver one independently testable behavior at a time.

From the repository root, `./checks/features.sh` exercises reloads, WebSockets, configuration migration and validation, per-host static roots, access logs, redirects, certificate selection, and upstream failover. The table lists the separate checks for other capabilities. Build prerequisites and invocation details are in the [test guide](testing.md).

## Acceptance contracts

| Feature | Observable behavior | Acceptance check |
| --- | --- | --- |
| [Configuration migration and validation](migration.md) | An operator or agent can inspect an NGINX dump and validate a Rill deployment configuration without starting the service. | [CLI contract](../checks/migration.py) |
| [Static HTTP responses](static-io.md) | Clients receive directory indexes, GET and HEAD responses, byte ranges, validators and conditional responses. | [Recorded HTTP cases](../checks/compare.py), [header and size checks](../checks/static_sizes.py), [live file replacement](../checks/static_updates.py) |
| [Windows TransmitFile](static-io.md#windows-transmitfile) | Two large plaintext downloads can use Windows direct file transfer while additional readers use buffered delivery. | [Native Windows socket checks](../checks/windows_transmitfile.py) |
| [Linux sendfile and immutable assets](static-io.md#linux-sendfile) | Large plaintext downloads can use sendfile. Fingerprinted assets can reuse bytes or an open descriptor across requests. | [Sendfile](../checks/sendfile.py), [asset cache](../checks/asset_cache.py), [mixed traffic](../checks/mixed_io.py) |
| [HTTP routing and balancing](routing.md) | Host and path select an upstream pool. Round robin or least connections selects a peer, and Rill streams the response. A refused connection returns 502. | [Routing](../checks/routing.py), [fixed-peer reuse](../checks/upstreams.py), [streaming](../checks/proxy_streaming.py), [upstream failure](../checks/proxy_failure.py) |
| [Backend health and TLS](routing.md#active-health-checks) | Probes exclude failed peers and restore recovered peers. HTTPS pools verify backend identity and can present a client certificate. | [Health, HTTPS and mutual TLS](../checks/upstreams.py) |
| [Frontend TLS and HTTP/3](edge.md) | Clients negotiate HTTP/1.1 or HTTP/2 over TLS or use HTTP/3 over QUIC on the same port. | [TCP and QUIC protocol checks](../checks/edge.py) |
| [HTTP policies](policy.md) | IP rules deny requests, per-peer limits return 429, and eligible responses use negotiated gzip. | [HTTP policies](../checks/policy.py) |
| [Proxy response cache](policy.md#proxy-cache) | Public responses can be reused, concurrent misses coalesced, and successful writes invalidate cached variants. | [Eligibility and expiry](../checks/policy.py), [coalescing and invalidation](../checks/upstreams.py) |
| [TCP, UDP and mail forwarding](streams.md) | Listeners balance TCP connections and UDP sessions. Mail backends handle authentication and protocol exchanges. | [Transport checks](../checks/streams.py) |
| [HTTPS redirects](edge.md#redirect-plaintext-http) | Plain HTTP redirects to the configured public authority with the path and query intact. | [Edge routing](../checks/edge_routing.py) |
| [Per-host certificates](edge.md#per-host-certificates) | Fresh handshakes get the named or default certificate, including after rotation. | [SNI and rollback](../checks/edge_routing.py) |
| [Connection failover](routing.md#connection-failover) | A bodyless read can reach a second peer after a connection error. | [Read and write behavior](../checks/edge_routing.py) |
| [Graceful HTTP configuration reload](reload.md) | An operator changes routes or rotates a server certificate while Rill serves traffic. Existing work completes. A rejected candidate leaves the last working configuration in service. | [Reload a live process](../checks/reload.py) |
| [WebSocket proxying](routing.md#websockets) | A client opens an HTTP/1.1 WebSocket on an HTTP route and exchanges messages with its backend over plaintext or TLS. A refused handshake remains an HTTP response. | [Exercise the handshake and tunnel](../checks/websocket.py) |
| [Per-host static sites](routing.md#per-host-static-sites) | Hosts serve independent document roots and reload atomically. | [Site isolation and reload](../checks/sites.py) |
| [Access logging](policy.md#access-logging) | Operators can identify the backend selected for a failed request. | [Access records](../checks/sites.py) |

Add a row here when introducing a new feature with its own contract. Keep detailed configuration examples in the corresponding operator guide.
