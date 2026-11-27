# Rill

Rill is an open-source alternative to NGINX, written in Rust. It sits at the front door of your website. When someone asks for a photo, Rill can send the file itself. When they ask what’s in their shopping cart, Rill passes the request to your app and brings the answer back. If several servers run your app, Rill can share the work among them.

## Why Rill?

A web server handles requests from strangers all day. A mistake as small as reading past the end of a message can expose someone else’s data. Rill uses Rust because its memory-safety checks help prevent this kind of bug. HTTPS keeps traffic encrypted on its way to and from your site.

Rill’s goal is to keep sites fast as they grow, while using less memory and keeping setup simple. One program can serve your files and route traffic to your app. It supports modern web standards like HTTP/3, and you can start with a folder of files and add routing rules when you need them. [Rill vs. NGINX benchmarks](bench/http-kit-static.md)

## Try it

Run the sample site with Docker:

```sh
docker run --rm -p 127.0.0.1:8080:8080 ghcr.io/dperox/rill:0.1.1
```

Open <http://127.0.0.1:8080/> to see **Rill is running**. You can also download a Linux or Windows executable. [Downloads](https://github.com/dperox/rill/releases/latest) · [Deployment guide](docs/deployment.md)

## Already using NGINX?

Rill can read your NGINX configuration, suggest a starting configuration, and flag settings that need your review. [Migration guide](docs/migration.md)

[Routing examples](docs/routing.md) · [Feature guide](docs/features.md) · [Contributing](AGENTS.md) · [MIT license](LICENSE)
