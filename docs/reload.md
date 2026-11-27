# Graceful configuration reloads

On Unix, send SIGHUP to replace HTTP routes and rotate the certificates used by the existing HTTPS and HTTP/3 listeners. Rill keeps its listening sockets open. New requests on existing keep-alive connections use the current routes. Requests already routed, active downloads, and established WebSocket tunnels continue on their original upstream. A client sees the new certificate on a new full TLS handshake.

## Start with a configuration file

Save the following as `rill.json`. The `routes` array uses the [HTTP routing format](routing.md). Each certificate path in the file is relative to the directory containing `rill.json`, unless absolute.

```json
{
  "routes": [
    { "prefix": "/api/", "upstreams": ["127.0.0.1:9001"] }
  ],
  "tls": {
    "cert": "certs/site-chain.pem",
    "key": "certs/site-key.pem"
  }
}
```

Provide the certificate chain and matching private key in PEM format, then start Rill from its project directory. This shell saves the server PID for the reload command below.

```sh
RILL_CONFIG="$PWD/rill.json" TLS_LISTEN=127.0.0.1:8443 ./target/release/rill &
rill_pid=$!
```

For an HTTP-only instance, omit both the `tls` object and `TLS_LISTEN`. The two JSON fields are independently optional. Omitting `routes` uses the startup values of `RILL_ROUTES` and `UPSTREAMS`. Supplying `routes` replaces both, and an empty array removes all proxy routes. Omitting `tls` uses the startup `TLS_CERT` and `TLS_KEY` paths. Environment certificate paths are relative to the process working directory. Unknown JSON fields cause validation to fail.

## Apply an update

For certificate rotation, stage both PEM files at new paths and reference them in the replacement JSON. Write the complete JSON to a sibling file first. Rename it into place and signal the same Rill process.

```sh
mv rill.next.json rill.json && kill -HUP "$rill_pid"
```

With the packaged container, publish the new configuration in a mounted directory and run `docker kill --signal=HUP <container-name>`. Mount the containing directory so atomic file replacement is visible inside the container.

Rill logs `Rill configuration reloaded` on success. Malformed JSON, invalid upstream definitions, and unreadable or mismatched TLS material produce `Rill reload rejected` with the error. The last accepted routes and certificates keep serving. Correct the files and send SIGHUP again. A malformed initial configuration prevents startup. Route validation checks upstream addresses and builds the transports. Backend availability is checked separately by requests or configured health probes.

Without `RILL_CONFIG`, SIGHUP rereads certificates at their original environment paths and rebuilds the environment-defined routes. Every successful reload starts fresh HTTP backend pools, health probes, balancing counters, and proxy response cache entries. The active rate limiter and static asset cache retain their state. Listener addresses, TLS enablement, TCP and UDP forwarding, document root, rate and compression policies, and runtime settings are selected at process startup.
