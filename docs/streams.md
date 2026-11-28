# TCP, UDP, and mail transport

Japka accepts additional TCP and UDP listeners alongside HTTP. Each listener has
its own upstream pool and round-robin cursor. Configure listeners with
semicolon-separated `BIND=UPSTREAM,UPSTREAM` entries. Use numeric IPv4 or
bracketed IPv6 socket addresses, including ports.

```sh
LISTEN=127.0.0.1:8080 \
JAPKA_TCP_LISTENERS='127.0.0.1:9000=127.0.0.1:19000,127.0.0.1:19001' \
JAPKA_UDP_LISTENERS='127.0.0.1:5353=127.0.0.1:15353,127.0.0.1:15354' \
JAPKA_STREAM_BALANCE=round_robin \
japka
```

Set `JAPKA_STREAM_BALANCE=least_connections` to choose the upstream with
the fewest active TCP connections or UDP sessions. Ties rotate among peers.
Round robin is the default. Both modes make a single selection per connection
or UDP session.

- TCP forwards bytes in both directions, propagates half-closes, and caps
  concurrent connections at 4,096 per listener. Japka closes the client
  connection if it cannot establish the upstream connection within five seconds
  or the connection fails immediately.
- UDP forwards whole datagrams. Each source IP and port keeps its selected
  upstream and upstream socket until idle for 5 seconds before the first
  backend reply, or 30 seconds once the backend has answered. Each listener
  supports up to 1,024 concurrent sessions, 32 concurrent sessions per source
  IP, and queues up to 16 client datagrams per session. The listener drops
  datagrams that exceed these limits.
- HTTP policies (`ALLOW_IPS`, `DENY_IPS`, `RATE_LIMIT`), `JAPKA_ACCESS_LOG`, TLS
  termination, and SIGHUP reloads apply only to HTTP listeners, not TCP/UDP
  stream listeners.

## IMAP, POP3, and SMTP

To expose existing mail backends, map one TCP listener per protocol. For
example, with IMAP on port 1143, POP3 on 1110, and SMTP on 1025:

```sh
JAPKA_TCP_LISTENERS='127.0.0.1:8143=127.0.0.1:1143;127.0.0.1:8110=127.0.0.1:1110;127.0.0.1:8025=127.0.0.1:1025' \
japka
```

The upstream sends its normal greeting and handles commands, authentication, and
any TLS handshake or upgrade. Japka selects the backend when the TCP connection opens. The backend handles mail authentication.

Run the integration checks to launch local mock mail backends and exercise IMAP
`NOOP`, POP3 `NOOP`, and SMTP `EHLO`, plus TCP load balancing, TCP half-closes,
and UDP session affinity and datagram boundaries.

```sh
cargo build --locked
python3 checks/streams.py target/debug/japka
```

## Kubernetes

Use a backend Service ClusterIP as the numeric upstream address. The deployment guide shows the matching TCP and UDP listener and Service ports. [Kubernetes forwarding example](deployment.md#add-tcp-and-udp-forwarding)
