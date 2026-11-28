# HTTPS, HTTP/2 and HTTP/3

Set `TLS_LISTEN` to enable HTTPS. Supply the certificate and key through `TLS_CERT` and `TLS_KEY`, or in the `tls` section of `JAPKA_CONFIG`. Japka reads a PEM certificate chain (leaf first) and matching PEM private key at startup. On Unix, send SIGHUP after replacing the files to rotate the certificate for new handshakes. See [graceful reloads](reload.md). The cleartext listener on `LISTEN` (default `127.0.0.1:8080` natively, or `127.0.0.1:$PORT` when `PORT` is set; `0.0.0.0:8080` in containers) remains active and cannot be disabled; set `JAPKA_HTTPS_REDIRECT` to redirect plaintext traffic to HTTPS.

Japka uses the same port for TCP (TLS 1.2 and TLS 1.3, negotiating HTTP/2 or HTTP/1.1 through ALPN) and UDP (TLS 1.3 QUIC negotiating HTTP/3). HTTPS responses advertise the local `TLS_LISTEN` UDP port through `Alt-Svc`. Make that port reachable over both transports. The listener uses the default certificate chain for missing or unmatched SNI and accepts clients without a client certificate. TCP TLS handshakes have a 10-second deadline, TLS HTTP/1.1 header reads have a 15-second deadline, HTTP/2 limits connections to 256 concurrent streams, and HTTP/3 caps header sections at 64 KiB. Requests targeting a hostname configured in `tls.hosts` over a connection with a different or missing SNI return HTTP 421 Misdirected Request. HTTP/3 sends each request through the same router as the TCP transports, evaluating access rules and access logging against the live QUIC peer address. Encrypted static responses use buffered file delivery. [Static file settings](static-io.md)

## Redirect plaintext HTTP

Set `JAPKA_HTTPS_REDIRECT=app.example` (or `app.example:8443`) at startup to return 308 from the plaintext HTTP listener. Japka preserves the request path and query. The configured authority supplies the destination even when the client sends a different Host. Access rules and rate limits still apply. All hosts redirect to this single public authority. Point it at a working HTTPS endpoint. Leaving the setting unset preserves ordinary plaintext service.

## Per-host certificates

Add exact DNS names to `tls.hosts` in the reload file. Each certificate must cover its configured name. Paths are relative to the configuration file. For example:

```json
{"tls":{"cert":"default.pem","key":"default.key","hosts":[
  {"host":"app.example","cert":"app.pem","key":"app.key"},
  {"host":"docs.example","cert":"docs.pem","key":"docs.key"}
]}}
```

TCP TLS and QUIC select the chain during the handshake using SNI. HTTP Host routing then selects the application or static site. The default chain remains required. On Unix, SIGHUP publishes the entire certificate set with routes and site roots. Duplicate names, invalid named certificates or mismatched keys reject the candidate.

## Local protocol check

From the repository root, generate a temporary localhost certificate and start Japka.

```sh
cargo build --release --locked
dir=$(mktemp -d)
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=localhost \
  -addext subjectAltName=DNS:localhost -keyout "$dir/key.pem" -out "$dir/cert.pem"
TLS_LISTEN=127.0.0.1:8443 TLS_CERT="$dir/cert.pem" TLS_KEY="$dir/key.pem" \
  UPSTREAMS=127.0.0.1:19092 target/release/japka &
pid=$!
```

Run the check in the same shell so `$dir` and `$pid` refer to the certificate and server above. It requires Python 3.11+, aioquic, curl_cffi, and a `curl` executable whose `curl --version` output lists `HTTP2`.

```sh
python3 -m venv "$dir/venv"
"$dir/venv/bin/pip" install aioquic curl_cffi
"$dir/venv/bin/python" checks/edge.py --ca "$dir/cert.pem" --port 8443 --echo-port 19092
kill "$pid"
```

The check verifies the certificate, negotiated TCP protocols and response bytes. An independent QUIC client retrieves the same file on two concurrent HTTP/3 streams. The echo check sends a 256 KiB POST through HTTP/3, verifies the upstream path, query, HTTP/1.1 version and response body, and checks removal of connection-specific response headers.

The framing checks use curl_cffi with HTTP/2 and HTTP/3 and reuse connections across HEAD, missing-file, conditional and range requests. With the echo backend enabled, they also check proxied HEAD and 204 responses.
