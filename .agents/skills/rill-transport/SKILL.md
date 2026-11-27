---
name: rill-transport
description: Maintain Rill’s HTTP connection lifecycle, WebSocket upgrades, sendfile path, and plaintext file-region writer, and select transport-specific regression checks.
---

Trace the affected data path before editing. src/plain.rs owns the plaintext socket used for ordered direct writes. src/edge.rs handles TLS and QUIC. The shared router dispatches proxy requests; src/upstream.rs owns the upstream HTTP client. Keep the negotiated protocol, connection ownership and response framing intact when switching paths.

For an HTTP/1 upgrade, verify hop-by-hop headers on both hops, preservation of buffered bytes and backpressure, and lease lifetime through tunnel closure. Exercise ws and wss with checks/websocket.py. For reload-sensitive changes, run checks/reload.py while an old request is in flight.

For file I/O or buffer changes, run checks/sendfile.py and checks/http1_buffers.py. Replay checks/compare.py with RILL_SENDFILE=0 and 1. For memory claims use bench/connections.py; for throughput use the matching workload in bench/README.md.

For driver changes, follow docs/hyper.md, exercise checks/shared_listener.py and verify the registry dependency lockfile. Keep notices/HYPER-LICENSE in the container.
