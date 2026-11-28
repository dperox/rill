# <img src="https://raw.githubusercontent.com/dperox/japka/main/public/brand/japka-logo.png" width="360" alt="Japka">

Japka is an open-source alternative to NGINX, written in Rust. It sits at the front door of your website. When someone asks for a photo, Japka can send the file itself. When they ask what’s in their shopping cart, Japka passes the request to your app and brings the answer back. If several servers run your app, Japka can share the work among them.

## Why Japka?

A web server handles requests from strangers all day. A mistake as small as reading past the end of a message can expose someone else’s data. Japka uses Rust because its memory-safety checks help prevent this kind of bug. HTTPS keeps traffic encrypted on its way to and from your site.

Japka’s goal is to keep sites fast as they grow, while using less memory and keeping setup simple. One program can serve your files and route traffic to your app. It supports modern web standards like HTTP/3, and you can start with a folder of files and add routing rules when you need them. [Japka vs. NGINX benchmarks](bench/http-kit-static.md)

## Try it

Run the bundled sample site with Docker.

```sh
docker run --rm -p 127.0.0.1:8080:8080 ghcr.io/dperox/japka:latest
```

Open <http://127.0.0.1:8080/> to see **Japka is running**. To run natively, build with `cargo build --release --locked` and start `target/release/japka`. [Deployment guide](docs/deployment.md)

On Apple silicon, Apple’s `container` can run the same image. [Run on a Mac](docs/deployment.md#macos) · [Native and Apple container benchmarks](bench/macos.md)

## Already using NGINX?

Japka can read your NGINX configuration, suggest a starting configuration, and flag settings that need your review. [Migration guide](docs/migration.md)

[Routing examples](docs/routing.md) · [Feature guide](docs/features.md) · [Logo and icon](docs/brand.md) · [Contributing](AGENTS.md) · [MIT license](LICENSE)
