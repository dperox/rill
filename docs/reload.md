# Graceful configuration reloads

On Unix, send SIGHUP to replace HTTP routes and per-host static roots and rotate the certificates used by the existing HTTPS and HTTP/3 listeners. Japka keeps its listening sockets open. New requests on existing keep-alive connections use the current routes. Requests already routed, active downloads, and established WebSocket tunnels continue on their original upstream. A client sees the new certificate on a new full TLS handshake.

## Start with a configuration file

Save the following as `japka.json`. The `routes` array uses the [HTTP routing format](routing.md). Each certificate path in the file is relative to the directory containing `japka.json`, unless absolute.

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

Provide the certificate chain and matching private key in PEM format, then start Japka from its project directory. This shell saves the server PID for the reload command below.

```sh
JAPKA_CONFIG="$PWD/japka.json" TLS_LISTEN=127.0.0.1:8443 japka &
japka_pid=$!
```

For an HTTP-only instance, omit both the `tls` object and `TLS_LISTEN`. The optional [sites array](routing.md#per-host-static-sites) replaces host-specific static roots; omitting it clears them. The JSON fields are independently optional. An omitted or null `routes` field uses the startup values of `JAPKA_ROUTES` and `UPSTREAMS`. Supplying `routes` replaces both, and an empty array removes all proxy routes. An omitted or null `tls` field uses the startup `TLS_CERT` and `TLS_KEY` paths. Environment certificate paths are relative to the process working directory. The optional `proxy` object changes HTTP balancing and backend health checks in the same reload. [HTTP pool settings](routing.md#reloadable-http-pool-settings) Unknown JSON fields cause validation to fail.

## Apply an update

For an agent-proposed change, use the [configuration review walkthrough](config-review.md) to compare the active and candidate files and apply the approved digest. The commands below show the manual Unix procedure.

For independently managed domains, include the [per-host chains](edge.md#per-host-certificates) in `tls.hosts`. Omitting the list restores default-chain-only service.

For certificate rotation, stage both PEM files at new paths and reference them in the replacement JSON. Write the complete JSON to a sibling file first. Rename it into place and signal the same Japka process.

```sh
mv japka.next.json japka.json && kill -HUP "$japka_pid"
```

With the packaged container, publish the new configuration in a mounted directory and run `docker kill --signal=HUP <container-name>`. Mount the containing directory so atomic file replacement is visible inside the container.

Japka logs `Japka configuration reloaded` on success. Japka emits a `Japka configuration active` record for a file-backed generation at startup and after a successful reload. Its PID, config path and file digest let the deploy helper identify the generation that accepted the file. Malformed JSON, invalid upstream definitions, and unreadable or mismatched TLS material produce `Japka reload rejected` with the error. Invalid site roots and duplicate site hostnames also reject the update. The last accepted routes, roots and certificates keep serving. Correct the files and send SIGHUP again. A malformed initial configuration prevents startup. Route validation checks upstream addresses and builds the transports. Backend availability is checked separately by requests or configured health probes.

Without `JAPKA_CONFIG`, SIGHUP rereads certificates at their original environment paths and rebuilds the environment-defined routes. Every successful reload starts fresh HTTP backend pools, health probes, balancing counters, and an empty proxy response cache. Each named static site also starts with a fresh asset cache. The default root keeps its asset cache, and the active rate limiter retains its counters. Changing listener addresses, TLS enablement, stream forwarding, the default `DOCUMENT_ROOT`, access logging, rate or compression policies, or runtime settings requires a restart.
