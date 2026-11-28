# <img src="https://raw.githubusercontent.com/japka-rs/japka/main/public/brand/japka-logo.png" width="360" alt="Japka">

Japka is a web server, reverse proxy, and load balancer written in Rust. It can serve your website from disk and forward requests under `/api/` to your application. When the application runs on several backends, Japka distributes requests across them.

- **Routing and protocols.** Route by hostname and path, serve HTTPS over HTTP/2 or HTTP/3, and proxy WebSocket connections.
- **Health checks and reloads.** Send traffic to healthy backends and update routes and certificates on Unix while existing requests finish.
- **Memory safety.** Rust’s ownership and borrowing rules prevent use-after-free bugs and data races in safe Rust code. Japka compiles to a native binary and runs without a garbage collector.
- **AI-assisted configuration.** An agent can prepare your setup, validate it, and show you what would change before deployment. [Copy the setup prompt](docs/README.md#get-running)

## Install with Cargo

```sh
cargo install japka
```

From the directory containing your **public website files**, start Japka and open <http://127.0.0.1:8080/>.

```sh
DOCUMENT_ROOT=. japka
```

Try the [sample site](docs/deployment.md#install-with-cargo) or download a [prebuilt binary](https://github.com/japka-rs/japka/releases/latest). [Japka on crates.io](https://crates.io/crates/japka)

## Run with Docker

```sh
docker run --rm -p 127.0.0.1:8080:8080 ghcr.io/japka-rs/japka:latest
```

Open <http://127.0.0.1:8080/> for the bundled site. [Mount your own site](docs/deployment.md#serve-your-own-site) · [Deploy to Kubernetes](docs/deployment.md#kubernetes)

## Performance on an M4 Mac mini

For 4 KiB responses, Japka served static files faster in both deployments, while NGINX handled more proxied requests. The table shows median requests per second over three rounds with 64 persistent HTTP/1.1 connections.

| 4 KiB workload | Japka native | NGINX native | Japka in Apple container | NGINX in Apple container |
| --- | ---: | ---: | ---: | ---: |
| Static file | 159,861 | 49,287 | 69,395 | 64,308 |
| Reverse proxy | 68,538 | 78,492 | 46,233 | 51,604 |

With 256 KiB proxy responses, Japka handled more requests per second in both deployments. The full report includes p95 latency, server settings, and reproduction steps. [Mac mini benchmark](bench/README.md)

## Review an agent’s configuration change

Compare the active configuration with the agent’s proposal before approving it.

```sh
japka config plan active.json candidate.json --json
```

When an agent proposes adding a route, the plan shows its host, path prefix, and backends, along with how many existing routes stay in place. On Unix, a trusted deploy job can reload the approved file, check the response from `/api/`, and restore the previous file if the check fails. [Try the walkthrough](docs/config-review.md) · [Configuration instructions for agents](AGENTS.md#navigate-configuration)

## Port an NGINX deployment

Translate an `nginx -T` dump into a candidate configuration, review the reported differences, and compare responses before switching traffic. [NGINX migration guide](docs/migration.md)

[Documentation](docs/README.md) · [Feature guide](docs/features.md) · [Logo and icon](docs/brand.md) · [Contributing](AGENTS.md) · [MIT license](LICENSE)
