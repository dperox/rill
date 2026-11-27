# HTTPS, HTTP/2 and HTTP/3

Set `TLS_LISTEN`, `TLS_CERT` and `TLS_KEY` together to enable the HTTPS endpoint. Rill reads a PEM certificate chain (leaf first) and matching PEM private key at startup. Restart Rill after replacing the files. The cleartext listener continues to use `LISTEN`.

Rill uses the same port for TCP and UDP. TCP negotiates HTTP/2 or HTTP/1.1 through ALPN. QUIC negotiates HTTP/3 over UDP. HTTPS responses advertise the UDP port through `Alt-Svc`. Make that port reachable over both transports. The listener uses one certificate chain for all hostnames and accepts clients without a client certificate. TCP TLS handshakes have a ten-second deadline. HTTP/3 sends each request through the same router as the TCP transports, including access rules and upstream selection. Encrypted static responses use buffered file delivery. [Static file settings](static-io.md)

## Local protocol check

From the repository root, generate a temporary localhost certificate and start Rill.

```sh
cargo build --release --locked
dir=$(mktemp -d)
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=localhost \
  -addext subjectAltName=DNS:localhost -keyout "$dir/key.pem" -out "$dir/cert.pem"
TLS_LISTEN=127.0.0.1:8443 TLS_CERT="$dir/cert.pem" TLS_KEY="$dir/key.pem" \
  UPSTREAMS=127.0.0.1:19092 target/release/rill &
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
