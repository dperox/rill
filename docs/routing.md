# Virtual hosts and load balancing

`UPSTREAMS` configures the default `/api/` pool. Use `RILL_ROUTES` when a single listener needs to serve multiple applications. This configuration sends one virtual host to a documentation service and gives every other host a shared store route.

```sh
export RILL_ROUTES='[
  {"host":"docs.example","prefix":"/","upstreams":["127.0.0.1:9010"]},
  {"prefix":"/store/","upstreams":["127.0.0.1:9020","127.0.0.1:9021"]}
]'
export RILL_HTTP_BALANCE=least_connections
cargo run --release --locked
```

Exact hostname matches take priority. Within each group the longest matching prefix wins. Hostnames are case insensitive and matching ignores the request authority's port. Prefixes start and end with a slash and match the incoming URI path. Rill forwards the original path and query and falls back to the document root when no rule matches. Duplicate host/prefix pairs, empty pools, and prefixes containing `?` or `#` fail startup. If both variables are set, Rill adds `UPSTREAMS` as a host-independent `/api/` route alongside the JSON rules. Defining the same host-independent prefix in `RILL_ROUTES` then creates a duplicate. With neither variable set, all paths go to the static handler.

Bare upstream addresses and `http://` URLs use plaintext HTTP/1.1 and receive `Host: backend`. HTTPS upstreams also use HTTP/1.1 and receive their configured authority in the Host header. The default, `RILL_HTTP_BALANCE=round_robin`, rotates through equal-weight upstreams. Least connections counts outstanding proxied HTTP requests, including response bodies in flight. It selects the least busy peer and rotates ties. The plain listener and encrypted transports share pool state. With active checks enabled, selection skips unhealthy peers. Each forwarded request selects one peer. A transport error before the upstream response headers arrive returns 502. If those headers take more than 60 seconds, Rill returns 504. Once the headers arrive, Rill streams the response body.

`python3 checks/routing.py` checks host and path-prefix selection and verifies that new requests reach the less busy backend while another request is in flight.

## Upstream HTTPS and mutual TLS

Set an HTTPS URL in either upstream list. For example, `UPSTREAMS=https://orders.internal:8443` verifies the server certificate against the system trust store and the hostname in the URL. Set `RILL_UPSTREAM_CA` to a PEM bundle to use a private trust store instead. Set both `RILL_UPSTREAM_CERT` and `RILL_UPSTREAM_KEY` to present a client certificate chain and matching private key. These settings apply to every HTTPS upstream in the process. Use separate processes for backends requiring different client identities. Mount private keys as read-only secrets. Restart to reload certificates.

## Active health checks

Set `RILL_HEALTH_INTERVAL_SECS` to a positive whole number of seconds to enable a TCP connection probe for each backend in each HTTP pool. The default is 0 (disabled). Set `RILL_HEALTH_PATH=/ready` to use an HTTP GET over the configured transport instead. A 2xx response marks the backend healthy. The probe times out after two seconds and the next interval starts after the probe completes. Each backend starts eligible and the first probe runs immediately. One failed check removes a backend from selection and one successful check restores it. Both balancing modes return 503 when all peers are unhealthy. An individual request is sent to one selected upstream. Use an HTTP readiness endpoint when TLS verification or application readiness should determine eligibility.
