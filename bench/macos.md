# Rill on macOS and Apple containers

A Mac mini with an Apple M4 (10 CPU cores, 16 GiB memory) ran Rill `3af81fc` on macOS 27.2.1 and in a Linux ARM64 microVM using Apple `container` 1.2.0. The same macOS client measured both endpoints on November 27, 2026. The table gives the median throughput and median per-round p95 latency across three rounds.

| HTTP/1.1 GET | Native req/s | Apple container req/s | Native p95 ms | Apple container p95 ms |
| --- | ---: | ---: | ---: | ---: |
| Static 4 KiB | 164,466 | 68,074 | 0.508 | 1.103 |
| Proxy 4 KiB | 71,832 | 45,979 | 1.066 | 1.836 |
| Static 256 KiB | 19,759 | 13,635 | 4.489 | 7.115 |
| Proxy 256 KiB | 13,132 | 10,821 | 6.580 | 7.275 |

The native executable had higher median throughput in all four workloads. Its 256 KiB proxy results varied the most, from 7,963 to 14,001 requests/s with p95 latency from 6.055 to 36.649 ms. The container ranged from 10,817 to 11,028 requests/s for that workload.

## What was measured

Homebrew wrk 4.2.0 used two client threads and 64 persistent connections. Each sample had two seconds of warm-up and five measured seconds. Native and container order reversed in the middle round. Both frontends had one Tokio worker and one blocking worker. For proxy GETs, a second Rill process with the same worker counts served the file over loopback within the same OS. Its files were in a separate document root.

The container had two vCPUs and 1 GiB of memory. macOS scheduled the host processes and VM without CPU pinning. The native client connected to a loopback listener for the native binary and to a published loopback port for the container. Native Rill read warm files from the host filesystem. The container read warm files copied onto its Linux filesystem before measurement. Both used their default file-delivery modes, with gzip, access logging, and application caches disabled.

All 6,182,752 measured responses had status 200 and matched the expected body byte-for-byte. The published-port results include the Mac-to-VM networking path. These are end-to-end local HTTP measurements on one machine. [Per-round throughput, latency, and verified response counts](macos-rounds.csv)

The [fixed-upstream comparison](fixed-upstream.md) measures the opt-in single-peer GET path against the PR #3 baseline and NGINX with the same workloads.

## Rill and NGINX on the same Mac

A second run interleaved Rill 0.1.1 and NGINX 1.31.10 on the same M4 Mac. Each row compares the two servers within one deployment. Throughput and p95 latency are medians of three rounds. The percentage is the change in throughput from NGINX to Rill.

| Deployment | GET workload | Rill req/s | NGINX req/s | Rill vs. NGINX | Rill p95 ms | NGINX p95 ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Native | Static 4 KiB | 165,704 | 48,486 | +241.8% | 0.471 | 1.472 |
| Native | Proxy 4 KiB | 71,627 | 82,201 | -12.9% | 1.056 | 0.923 |
| Native | Static 256 KiB | 18,626 | 26,051 | -28.5% | 5.188 | 2.905 |
| Native | Proxy 256 KiB | 12,602 | 12,988 | -3.0% | 6.943 | 5.992 |
| Apple container | Static 4 KiB | 66,604 | 61,040 | +9.1% | 1.216 | 1.253 |
| Apple container | Proxy 4 KiB | 50,763 | 44,317 | +14.5% | 1.632 | 2.245 |
| Apple container | Static 256 KiB | 14,706 | 14,506 | +1.4% | 5.753 | 5.575 |
| Apple container | Proxy 256 KiB | 10,477 | 11,245 | -6.8% | 7.488 | 8.281 |

On native macOS, Rill served the 4 KiB static file at 3.42 times NGINX’s throughput. NGINX led the other three native workloads. In Apple containers the 256 KiB static medians were within 1.4%. For the 256 KiB container proxy, NGINX had higher throughput while Rill had lower p95 latency.

The macOS client ran wrk 4.2.0 with two threads and 64 persistent connections. Each of the 48 samples had two seconds of warm-up and five seconds of measurement. The serving order reversed in round two. All 10,865,981 measured responses returned HTTP 200 and matched the expected bytes.

Native Rill was the release build from `3af81fc`; native NGINX came from Homebrew. Each container ran the same comparison image, combining the published Rill v0.1.1 ARM64 binary with the official NGINX 1.31.10 Linux image. Each microVM had two vCPUs, 1 GiB of memory and guest-local files. Both container frontends received traffic through published loopback ports. The host scheduler chose CPU placement.

NGINX used a single worker, `sendfile on`, HTTP/1.1 upstream keep-alive, and disabled access logging, compression and proxy caching. NGINX used its default proxy buffering configuration and logged temporary-file buffering for some native 256 KiB proxy responses. Rill used one network worker and one blocking worker. Both native proxies used the same single-worker Rill origin. Each container used an equivalent Rill origin over guest loopback. [All 48 rounds](macos-nginx-rounds.csv) · [Comparison image](Dockerfile.macos-nginx)

To repeat the comparison, install native NGINX and wrk and verify `nginx -v` matches the version in the comparison image. From the checkout with a release build of Rill and Apple container services running:

```sh
container build -f bench/Dockerfile.macos-nginx -t rill-nginx:mac-bench .
python3 bench/macos.py --nginx "$(command -v nginx)" --image rill-nginx:mac-bench > macos-nginx-rounds.csv
```

## Reproduce the Rill baseline

On an Apple silicon Mac with stable Rust, Python 3, wrk with Lua, and Apple `container`, run from the repository root:

```sh
cargo build --release --locked
container system start
container build -f deploy/Dockerfile -t rill:local .
python3 bench/macos.py --image rill:local > macos-rounds.csv
```

The driver starts both frontends and their backends, measures each pair from macOS, and stops its processes and container. Use `--wrk` and `--rill` to select alternative executable paths. [Deployment and release downloads](../docs/deployment.md#macos)
