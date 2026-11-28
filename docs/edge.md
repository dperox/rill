# HTTPS, HTTP/2 and HTTP/3

Set `TLS_LISTEN` to enable HTTPS. Supply the certificate and key through `TLS_CERT` and `TLS_KEY`, or in the `tls` section of `JAPKA_CONFIG`. Japka reads a PEM certificate chain (leaf first) and matching PEM private key at startup. On Unix, send SIGHUP after replacing the files to rotate the certificate for new handshakes. See [graceful reloads](reload.md). The cleartext listener on `LISTEN` (default `127.0.0.1:8080` natively, or `127.0.0.1:$PORT` when `PORT` is set; `0.0.0.0:8080` in containers) remains active and cannot be disabled; set `JAPKA_HTTPS_REDIRECT` to redirect plaintext traffic to HTTPS.

Japka uses the same port for TCP (TLS 1.2 and TLS 1.3, negotiating HTTP/2 or HTTP/1.1 through ALPN) and UDP (TLS 1.3 QUIC negotiating HTTP/3). HTTPS responses advertise the local `TLS_LISTEN` UDP port through `Alt-Svc`. Make that port reachable over both transports. The listener uses the default certificate chain for missing or unmatched SNI and accepts clients without a client certificate. TCP TLS handshakes have a 10-second deadline, TLS HTTP/1.1 header reads have a 15-second deadline, HTTP/2 limits connections to 256 concurrent streams, and HTTP/3 caps header sections at 64 KiB. Requests targeting a hostname configured in `tls.hosts` over a connection with a different or missing SNI return HTTP 421 Misdirected Request. HTTP/3 sends each request through the same router as the TCP transports, evaluating access rules and access logging against the live QUIC peer address. Encrypted static responses use buffered file delivery. [Static file settings](static-io.md)

## Redirect plaintext HTTP

Set `JAPKA_HTTPS_REDIRECT=app.example` (or `app.example:8443`) at startup to return 308 from the plaintext HTTP listener. Japka preserves the request path and query. The configured authority supplies the destination even when the client sends a different Host. Japka applies access rules and rate limits to these requests. All hosts redirect to this single public authority. Point it at a working HTTPS endpoint. Leave the setting unset to serve requests over plaintext HTTP.

## Per-host certificates

Add exact DNS names to `tls.hosts` in the reload file. Each certificate must cover its configured name. Paths are relative to the configuration file. For example:

```json
{"tls":{"cert":"default.pem","key":"default.key","hosts":[
  {"host":"app.example","cert":"app.pem","key":"app.key"},
  {"host":"docs.example","cert":"docs.pem","key":"docs.key"}
]}}
```

TCP TLS and QUIC select the chain during the handshake using SNI. HTTP Host routing then selects the application or static site. Japka requires a default certificate chain. On Unix, SIGHUP publishes the entire certificate set with routes and site roots. Duplicate names, invalid named certificates or mismatched keys reject the candidate.

## Verify HTTPS protocols

With a client that supports each protocol, request a known page on your HTTPS listener.
Use its configured hostname so curl verifies the certificate and sends the correct SNI.
For a private certificate authority, add `--cacert /path/to/ca.pem`.

```sh
curl --http2 --fail --silent --show-error --output /dev/null \
  --write-out '%{http_code} HTTP/%{http_version}\n' https://app.example/
curl --http3-only --fail --silent --show-error --output /dev/null \
  --write-out '%{http_code} HTTP/%{http_version}\n' https://app.example/
```

A successful page request prints `200 HTTP/2` or `200 HTTP/3` respectively.

## Kubernetes TLS and HTTP/3

Start with the [HTTP Deployment and Service](deployment.md#kubernetes). Keep `namespace` set to the namespace where you deployed Japka. Create a TLS Secret in the Deployment’s namespace. The patch mounts it
read-only and enables TLS on TCP and UDP port 8443. Plain HTTP and the TCP
probes continue on 8080. To redirect HTTP clients, set `JAPKA_HTTPS_REDIRECT`
to the public HTTPS authority. [Redirect settings](#redirect-plaintext-http)

```sh
kubectl -n "$namespace" create secret tls japka-tls --cert=./tls/tls.crt --key=./tls/tls.key
kubectl -n "$namespace" patch deployment japka --type=strategic --patch "$(cat <<'PATCH'
spec:
  template:
    spec:
      securityContext:
        fsGroup: 10001
      volumes:
        - name: tls
          secret:
            secretName: japka-tls
            defaultMode: 0440
      containers:
        - name: japka
          env:
            - name: TLS_LISTEN
              value: "0.0.0.0:8443"
            - name: TLS_CERT
              value: "/run/tls/tls.crt"
            - name: TLS_KEY
              value: "/run/tls/tls.key"
          volumeMounts:
            - name: tls
              mountPath: /run/tls
              readOnly: true
PATCH
)"
kubectl -n "$namespace" rollout status deployment/japka
kubectl -n "$namespace" apply -f - <<'SERVICE'
apiVersion: v1
kind: Service
metadata:
  name: japka-tls
spec:
  type: ClusterIP
  selector:
    app: japka
  ports:
    - name: https
      port: 8443
      targetPort: 8443
      protocol: TCP
    - name: http3
      port: 8443
      targetPort: 8443
      protocol: UDP
SERVICE
```

Use the TLS Service from inside the cluster. For external HTTP/3 clients,
use a load balancer or gateway that forwards both transports. Preserve
the advertised QUIC port. `kubectl port-forward` can test HTTPS over TCP.
HTTP/3 needs a route to the UDP port. After the mounted Secret updates, send SIGHUP to each Japka process to rotate its certificate. A Deployment rollout also picks up the new files. [Graceful reloads](reload.md)
