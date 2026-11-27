# Crates and implementation choices

| Work | Packages used in Rill | Reason |
| --- | --- | --- |
| HTTP and upstream reuse | Axum, Tower, Hyper, hyper-util | A shared router handles the application policies. Hyper manages HTTP framing and flow control. The hyper-util client pools upstream HTTP/1 connections. Hyper-rustls adds verified TLS and client certificates for HTTPS upstreams. Rill dispatches through a Tower service to reduce boxed handler cloning. |
| TLS, QUIC and HTTP/3 | tokio-rustls, Quinn, h3, h3-quinn | Rustls negotiates TLS with ALPN. Quinn handles QUIC transport. The h3 adapter streams request and response frames through the same application as HTTP/1 and HTTP/2. |
| Socket concurrency | Tokio | Nonblocking TCP/UDP sockets, bounded UDP queues, per-session tasks, TCP half-close handling, and a reusable receive buffer per listener and upstream UDP session. |
| Static files | tower-http, Tokio, tokio-util, mime_guess, httpdate, rustix, libc | ServeDir covers range and encoded paths. The ordinary-file handler groups up to eight requests per blocking task and reads files through 64 KiB inline. Rustix wraps Linux sendfile for eligible large plaintext HTTP/1 responses. Buffered downloads stream through Tokio. The opt-in immutable asset cache retains small bodies or an open descriptor for larger files. |
| HTTP policies | tower-http, ipnet, bytes, http-body-util | Negotiated gzip, CIDR matching and streaming bodies. Cache eligibility, limits and invalidation are Rill policies. |

The pinned versions are in [Cargo.lock](../Cargo.lock). Rill pins a fork of Hyper 1.13.0 for server-side HTTP/1 buffer sizing and ordered direct-body support. See the [patches and update instructions](hyper.md). Performance measurements are in [the benchmark guide](../bench/README.md). The plaintext writer and file sender share a Tokio socket, with socket2 providing write-side shutdown. Libc supplies platform constants including the nonblocking flag for file opens, which avoids stalling on a FIFO during the preliminary file-type check.

Upstream references include [Tokio filesystem behavior](https://docs.rs/tokio/latest/tokio/fs/), [Hyper utilities](https://docs.rs/hyper-util/), [Tower HTTP](https://docs.rs/tower-http/), [Quinn](https://docs.rs/quinn/), and [h3](https://docs.rs/h3/).

See [static file settings](static-io.md) for storage and buffering options.
