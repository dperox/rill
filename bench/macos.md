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

## Reproduce

On an Apple silicon Mac with stable Rust, Python 3, wrk with Lua, and Apple `container`, run from the repository root:

```sh
cargo build --release --locked
container system start
container build -f deploy/Dockerfile -t rill:local .
python3 bench/macos.py --image rill:local > macos-rounds.csv
```

The driver starts both frontends and their backends, measures each pair from macOS, and stops its processes and container. Use `--wrk` and `--rill` to select alternative executable paths. [Deployment and release downloads](../docs/deployment.md#macos)
