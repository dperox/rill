# <img src="https://raw.githubusercontent.com/dperox/japka/main/public/brand/japka-logo.png" width="360" alt="Japka">

Japka is a web server, reverse proxy, and load balancer written in Rust. It can serve your website from disk and forward requests under `/api/` to your application. When the application runs on several backends, Japka distributes requests across them.

- **Routing and protocols.** Route by hostname and path, serve HTTPS over HTTP/2 or HTTP/3, and proxy WebSocket connections.
- **Health checks and reloads.** Send traffic to healthy backends and update routes and certificates on Unix while existing requests finish.
- **AI-assisted configuration.** An agent can validate a proposed change and show you its effect before deployment. [Configuration review](docs/config-review.md)

## Install with Cargo

```sh
cargo install japka
```

From the directory containing your **public website files**, start Japka and open <http://127.0.0.1:8080/>.

```sh
DOCUMENT_ROOT=. japka
```

For a sample site or Windows instructions, see the [deployment guide](docs/deployment.md#install-with-cargo). [Japka on crates.io](https://crates.io/crates/japka) · [Prebuilt binaries](https://github.com/dperox/japka/releases/latest)

## Run with Docker

```sh
docker run --rm -p 127.0.0.1:8080:8080 ghcr.io/dperox/japka:latest
```

Open <http://127.0.0.1:8080/> for the bundled site. [Docker deployment](docs/deployment.md#docker) · [Apple containers](docs/deployment.md#apple-containers)

## Review an agent’s configuration change

Compare the active configuration with the agent’s proposal before approving it.

```sh
japka config plan active.json candidate.json --json
```

When an agent proposes adding a route, the plan shows its host, path prefix, and backends, along with how many existing routes stay in place. On Unix, a trusted deploy job can reload the approved file, check the response from `/api/`, and restore the previous file if the check fails. [Try the walkthrough](docs/config-review.md) · [Configuration instructions for agents](AGENTS.md#navigate-configuration)

## Port an NGINX deployment

Translate an `nginx -T` dump into a candidate configuration, review the reported differences, and compare responses before switching traffic. [NGINX migration guide](docs/migration.md)

[Routing examples](docs/routing.md) · [Feature guide](docs/features.md) · [Mac mini benchmarks against NGINX](bench/README.md) · [Logo and icon](docs/brand.md) · [Contributing](AGENTS.md) · [MIT license](LICENSE)
