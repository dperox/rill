# Feature development

Describe a feature in terms of what a client or operator can do. A proposal can start as a [feature request](../.github/ISSUE_TEMPLATE/feature.md). Include one example of success and one failure the implementation must handle. Deliver one independently testable behavior at a time.

Run the two feature checks together with `./checks/features.sh` on Linux or WSL.

## Acceptance contracts

| Feature | Observable behavior | Acceptance check |
| --- | --- | --- |
| [Graceful HTTP configuration reload](reload.md) | An operator changes routes or rotates a server certificate while Rill serves traffic. Existing work completes. A rejected candidate leaves the last working configuration in service. | [Reload a live process](../checks/reload.py) |
| [WebSocket proxying](routing.md#websockets) | A client opens an HTTP/1.1 WebSocket on an HTTP route and exchanges messages with its backend over plaintext or TLS. A refused handshake remains an HTTP response. | [Exercise the handshake and tunnel](../checks/websocket.py) |

For changes to earlier features, the [test guide](testing.md) maps static serving, caching, routing, policy and transport checks to their commands. Add a row here when introducing a new feature with its own contract. Keep detailed configuration examples in the corresponding operator guide.
