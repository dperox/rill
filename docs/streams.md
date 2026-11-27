# TCP, UDP, and mail transport

Rill accepts additional TCP and UDP listeners alongside HTTP. Each listener has
its own upstream pool and round-robin cursor. Configure listeners with
semicolon-separated `BIND=UPSTREAM,UPSTREAM` entries. Use numeric IPv4 or
bracketed IPv6 socket addresses, including ports.

```sh
LISTEN=127.0.0.1:8080 \
RILL_TCP_LISTENERS='127.0.0.1:9000=127.0.0.1:19000,127.0.0.1:19001' \
RILL_UDP_LISTENERS='127.0.0.1:5353=127.0.0.1:15353,127.0.0.1:15354' \
RILL_STREAM_BALANCE=round_robin \
target/debug/rill
```

Set `RILL_STREAM_BALANCE=least_connections` to choose the upstream with
the fewest active TCP connections or UDP sessions. Ties rotate among peers.
Round robin is the default. Both modes make a single selection per connection
or UDP session.

- TCP forwards bytes in both directions, propagates half-closes, and caps
  concurrent connections at 4,096 per listener. Rill closes the client
  connection if it cannot establish the upstream connection within five seconds
  or the connection fails immediately.
- UDP forwards whole datagrams. Each source IP and port keeps its selected
  upstream and upstream socket until idle for 5 seconds before the first
  backend reply, or 30 seconds once the backend has answered. Each listener
  supports up to 1,024 concurrent sessions, 32 concurrent sessions per source
  IP, and queues up to 16 client datagrams per session. The listener drops
  datagrams that exceed these limits.
- HTTP policies (`ALLOW_IPS`, `DENY_IPS`, `RATE_LIMIT`), `RILL_ACCESS_LOG`, TLS
  termination, and SIGHUP reloads apply only to HTTP listeners, not TCP/UDP
  stream listeners.

## IMAP, POP3, and SMTP

To expose existing mail backends, map one TCP listener per protocol. For
example, with IMAP on port 1143, POP3 on 1110, and SMTP on 1025:

```sh
RILL_TCP_LISTENERS='127.0.0.1:8143=127.0.0.1:1143;127.0.0.1:8110=127.0.0.1:1110;127.0.0.1:8025=127.0.0.1:1025' \
target/debug/rill
```

The upstream sends its normal greeting and handles commands, authentication, and
any TLS handshake or upgrade. Rill selects the backend when the TCP connection opens. The backend handles mail authentication.

Run the integration checks to launch local mock mail backends and exercise IMAP
`NOOP`, POP3 `NOOP`, and SMTP `EHLO`, plus TCP load balancing, TCP half-closes,
and UDP session affinity and datagram boundaries.

```sh
cargo build --locked
python3 checks/streams.py target/debug/rill
```

## Kubernetes

The base manifest exposes HTTP on port 8080. To add a TCP proxy on port 9000
or a UDP proxy on port 5353, configure the listener to bind to a Pod-reachable
address such as `0.0.0.0` and supply its upstreams. Add the matching port and
protocol to the Deployment and Service. A backend Service ClusterIP is a stable
numeric upstream address for a stream listener. [Deployment guide](deployment.md)
