# Japka and NGINX on a Mac mini

An M4 Mac mini ran Japka and NGINX 1.31.10 as native macOS processes and in Linux ARM64 guests using Apple’s `container`. A client on the Mac fetched warm files directly from each server and through its reverse proxy.

## Results

Throughput is in requests per second. Latency is the 95th percentile in milliseconds. Each value is the median of three rounds, and every measured response returned HTTP 200 and matched the expected bytes.

### Native

The native processes ran a Japka release build from this revision. The 9,775,647 measured responses passed validation. [Per-round measurements](macos-native-rounds.csv)

| HTTP/1.1 GET | Japka req/s | NGINX req/s | Japka p95 ms | NGINX p95 ms |
| --- | ---: | ---: | ---: | ---: |
| Static 4 KiB | 237,814 | 62,577 | 0.296 | 1.046 |
| Proxy 4 KiB | 107,956 | 124,784 | 0.676 | 0.577 |
| Static 256 KiB | 25,744 | 44,196 | 2.830 | 1.487 |
| Proxy 256 KiB | 16,811 | 15,197 | 4.554 | 5.065 |

For 4 KiB requests, Japka served static files at 3.80 times NGINX’s throughput and handled 87% of NGINX’s proxied requests. NGINX led on 256 KiB static files. With 256 KiB proxy responses, Japka handled more requests per second.

### Apple container

These rows come from an earlier run of Japka 0.2.0 on macOS 27.2.1. The CSV also holds that run’s native rows. [Per-round measurements](macos-nginx-rounds.csv)

| HTTP/1.1 GET | Japka req/s | NGINX req/s | Japka p95 ms | NGINX p95 ms |
| --- | ---: | ---: | ---: | ---: |
| Static 4 KiB | 69,395 | 64,308 | 1.042 | 1.094 |
| Proxy 4 KiB | 46,233 | 51,604 | 1.638 | 1.466 |
| Static 256 KiB | 14,508 | 14,143 | 5.023 | 4.982 |
| Proxy 256 KiB | 11,733 | 8,968 | 6.528 | 14.188 |

In containers, NGINX led on 4 KiB proxy requests. With 256 KiB proxy responses, Japka handled more requests per second.

## Test conditions

- **Host.** Mac mini, Apple M4 (4 performance and 6 efficiency cores), 16 GB RAM. The native run used macOS 26.6.2 on November 27, 2026. macOS scheduled the client, native servers, and guests without CPU pinning.
- **Binaries.** The native Japka binary was built with `cargo build --release --locked` and Rust 1.98.1. Native NGINX came from Homebrew. For the container run, the Linux Japka binary came from the GHCR image for the 0.2.0 commit (`8980f9d`). Both guests used the same comparison image with Japka and official NGINX binaries. [Pinned input images](Dockerfile.macos-nginx)
- **Client.** wrk 4.2.0 used two threads and 64 persistent connections. Each sample had two seconds of warm-up followed by five seconds of measurement. The servers ran in reverse order in the second round. Latency includes the client’s connection to the native loopback listener or the guest’s published port.
- **Servers.** Japka used one network worker and one blocking worker. NGINX used one worker, sendfile, upstream HTTP/1.1 keep-alive and its default proxy buffering. Compression and access logging were disabled. The two native proxies shared a Japka origin. Each guest ran an equivalent origin on guest loopback.
- **Containers.** Apple `container` 1.2.0 assigned each guest two vCPUs and 1 GiB of memory. Each guest served a copy of the fixture files from its own filesystem.

## Reproduce the comparison

Run from the repo root on Apple silicon with Python 3, Lua-enabled wrk and native NGINX matching the version above. For the native rows:

```sh
cargo build --release --locked
python3 bench/macos.py --japka target/release/japka --native-only > native-rounds.csv
```

To include the guests, also install Apple `container`, then build the comparison image and omit `--native-only`:

```sh
container system start
container build --platform linux/arm64 -f bench/Dockerfile.macos-nginx -t japka-nginx:mac-bench .
python3 bench/macos.py --japka target/release/japka > rounds.csv
```
