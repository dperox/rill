# Rill

Rill is a Rust web server, reverse proxy and load balancer. A single executable serves static files, routes HTTP requests to application servers, terminates TLS for HTTP/1.1, HTTP/2 and HTTP/3, and forwards TCP connections and UDP datagrams. Use environment variables for startup settings and an optional file for live updates to HTTP routes, per-host static roots, and TLS credentials. Its HTTP regression suite checks response status, headers and body bytes using Rust Anything.

## Run from source

On Linux or Ubuntu under WSL2, install stable Rust with Cargo, a C compiler and a linker. For a fresh checkout, clone the project on the Linux filesystem. The default site is `public/`, resolved relative to the process working directory.

```sh
git clone https://github.com/dperox/rill.git
cd rill
cargo build --release --locked
./target/release/rill
```

In another terminal, run `curl -i http://127.0.0.1:8080/`. The sample page says **Rill is running**. To serve your own site, set `DOCUMENT_ROOT` to an existing directory. `LISTEN` sets the bind address. When it is unset, Rill binds to loopback and uses `PORT`, which defaults to 8080.

## Run with Docker or Kubernetes

Build the Linux image from the repository root and publish it on localhost. The image includes the sample site and runs as UID 10001.

```sh
docker build -f deploy/Dockerfile -t rill:local .
docker run --rm -p 127.0.0.1:8080:8080 rill:local
```

For Kubernetes, make the image available to each node first. For example, `kind load docker-image rill:local` loads it into the default local kind cluster. Then deploy the two replicas and access the ClusterIP Service from your machine.

```sh
kubectl apply -f deploy/kubernetes.yaml
kubectl rollout status deployment/rill
kubectl port-forward service/rill 8080:8080
```

Choose a free local port if another Rill instance is running. For bind mounts, registry images, TLS, and optional transport listeners, see the [deployment guide](docs/deployment.md).

## Proxy an application

With an HTTP application listening on port 9001, start Rill with the following settings. Run one Rill frontend per listen address.

```sh
UPSTREAMS=127.0.0.1:9001 ./target/release/rill
```

A request for `/api/products?limit=5` reaches the backend with that path and query and `Host: backend`. Other paths still serve the document root. Add comma-separated upstreams for round-robin balancing, or use `RILL_ROUTES` for host and path-prefix rules. HTTP/1.1 WebSocket upgrades use the same routes. HTTPS and mutual TLS upstreams and active health checks are optional. [Routing examples](docs/routing.md)

## Configuration

| Task | Settings and guide |
| --- | --- |
| Serve files and cache versioned assets | `DOCUMENT_ROOT`, `RILL_SENDFILE`, `RILL_STATIC_CHUNK_KIB`, `RILL_ASSET_CACHE_ENTRIES`. [Static file settings](docs/static-io.md) |
| Route HTTP and balance backends | `UPSTREAMS`, `RILL_ROUTES`, `RILL_HTTP_BALANCE`. [HTTP routing](docs/routing.md) |
| Serve independent sites on one listener | Add exact hostnames and document roots to the `sites` array in `RILL_CONFIG`. [Per-host static sites](docs/routing.md#per-host-static-sites) |
| Inspect HTTP requests and backend failures | Set `RILL_ACCESS_LOG=1` for JSON access records on stderr. [Access logging](docs/policy.md#access-logging) |
| Reload routes, sites and certificates on Unix | Set `RILL_CONFIG` to a JSON file and send `SIGHUP`. [Graceful reloads](docs/reload.md) |
| Verify upstream readiness and identity | Configure TCP or HTTP probes with `RILL_HEALTH_INTERVAL_SECS` and `RILL_HEALTH_PATH`. HTTPS pools support a private CA and client certificates. [Upstream configuration](docs/routing.md#upstream-https-and-mutual-tls) |
| Enable HTTPS and QUIC | Set `TLS_LISTEN` and supply the server certificate and key. HTTP/3 uses UDP on the same port as HTTPS over TCP. [TLS and protocol checks](docs/edge.md) |
| Cache proxy responses, compress or limit requests | `CACHE_TTL`, `CACHE_COALESCE`, `COMPRESS`, `RATE_LIMIT`, `ALLOW_IPS`, `DENY_IPS`. [HTTP policies](docs/policy.md) |
| Forward TCP, UDP or mail to backends | `RILL_TCP_LISTENERS`, `RILL_UDP_LISTENERS`, `RILL_STREAM_BALANCE`. [Transport configuration](docs/streams.md) |

Rill defaults to **one blocking worker** for filesystem operations and DNS. Set `RILL_BLOCKING_THREADS` to override it. Async network workers default to the available CPU count and can be set independently with `TOKIO_WORKER_THREADS`. [Runtime and storage tuning](docs/static-io.md)

## Verification and performance

`cargo test --locked` runs the Rust unit tests. The [testing guide](docs/testing.md) covers the HTTP and transport integration suites, including how to replay the 56 recorded HTTP cases. The [implementation notes](docs/crates.md) describe the Rust dependencies and the patches in Rill’s Hyper fork.

With one frontend CPU and 64 HTTP/1.1 clients on WSL2, Rill’s immutable asset cache increased throughput for a warm 256 KiB file from 17,481 to 21,016 requests/s. See the [measurements](bench/sendfile.md) and [benchmark commands](bench/README.md) for the workload and settings.

## Contributing

The [feature guide](docs/features.md) connects operator-visible behavior to acceptance tests. [AGENTS.md](AGENTS.md) lists repository skills and development commands.

## License

Rill’s original code and documentation are available under the [MIT License](LICENSE). Rill includes the [MIT notice for Hyper](notices/HYPER-LICENSE). The [Hyper patch notes](docs/hyper.md) describe the fork.
