# <img src="https://raw.githubusercontent.com/dperox/japka/main/public/brand/japka-logo.png" width="360" alt="Japka">

Japka is an open-source alternative to NGINX, written in Rust. It sits at the front door of your website. When someone asks for a photo, Japka can send the file itself. When they ask what’s in their shopping cart, Japka passes the request to your app and brings the answer back. If several servers run your app, Japka can share the work among them.

## Install with Cargo

```sh
cargo install japka
```

From the directory containing your website’s `index.html`, run this command on macOS or Linux to serve that directory.

```sh
DOCUMENT_ROOT=. japka
```

Open <http://127.0.0.1:8080/> to see your site. For a sample page or Windows instructions, see the [deployment guide](docs/deployment.md#install-with-cargo). [Japka on crates.io](https://crates.io/crates/japka) · [Prebuilt binaries](https://github.com/dperox/japka/releases/latest)

## Why Japka?

A web server handles requests from strangers all day. A mistake as small as reading past the end of a message can expose someone else’s data. Japka uses Rust because its memory-safety checks help prevent this kind of bug. HTTPS keeps traffic encrypted on its way to and from your site.

Japka’s goal is to keep sites fast as they grow, while using less memory and keeping setup simple. One program can serve your files and route traffic to your app. It supports modern web standards like HTTP/3, and you can start with a folder of files and add routing rules when you need them. [Japka vs. NGINX benchmarks](bench/http-kit-static.md)

## Run with Docker

```sh
docker run --rm -p 127.0.0.1:8080:8080 ghcr.io/dperox/japka:latest
```

Open <http://127.0.0.1:8080/> to see the bundled sample site. [Docker deployment](docs/deployment.md#docker) · [Apple containers](docs/deployment.md#apple-containers)

## Already using NGINX?

Japka can read your NGINX configuration, suggest a starting configuration, and flag settings that need your review. [Migration guide](docs/migration.md)

[Routing examples](docs/routing.md) · [Feature guide](docs/features.md) · [Logo and icon](docs/brand.md) · [Contributing](AGENTS.md) · [MIT license](LICENSE)
