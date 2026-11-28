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

## Kubernetes

Start with the [HTTP Deployment and Service](deployment.md#kubernetes). Keep `namespace` set to the namespace where you deployed Japka. Use your backend Services’ ClusterIPs as the numeric upstream addresses in the example below.

The base manifest enables HTTP only. Create a `japka-streams` ConfigMap with
numeric upstream socket addresses that the Pods can reach, then apply the
Deployment and Service patches below.

```sh
kubectl -n "$namespace" create configmap japka-streams \
  --from-literal=JAPKA_TCP_LISTENERS=0.0.0.0:9000=192.0.2.10:19000 \
  --from-literal=JAPKA_UDP_LISTENERS=0.0.0.0:5353=192.0.2.20:15353
```

```sh
kubectl -n "$namespace" patch deployment japka --type=strategic --patch "$(cat <<'STREAMS_PATCH'
spec:
  template:
    spec:
      containers:
        - name: japka
          env:
            - name: JAPKA_TCP_LISTENERS
              valueFrom:
                configMapKeyRef:
                  name: japka-streams
                  key: JAPKA_TCP_LISTENERS
            - name: JAPKA_UDP_LISTENERS
              valueFrom:
                configMapKeyRef:
                  name: japka-streams
                  key: JAPKA_UDP_LISTENERS
            - name: JAPKA_STREAM_BALANCE
              valueFrom:
                configMapKeyRef:
                  name: japka-streams
                  key: JAPKA_STREAM_BALANCE
                  optional: true
          ports:
            - name: tcp-proxy
              containerPort: 9000
              protocol: TCP
            - name: udp-proxy
              containerPort: 5353
              protocol: UDP
STREAMS_PATCH
)"
kubectl -n "$namespace" rollout status deployment/japka
kubectl -n "$namespace" patch service japka --type=strategic --patch "$(cat <<'STREAMS_SERVICE'
spec:
  ports:
    - name: tcp-proxy
      port: 9000
      targetPort: tcp-proxy
      protocol: TCP
    - name: udp-proxy
      port: 5353
      targetPort: udp-proxy
      protocol: UDP
STREAMS_SERVICE
)"
```

The Service keeps its existing address and HTTP port. Restart the Deployment
when you change the ConfigMap values.
