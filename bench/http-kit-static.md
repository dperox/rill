# Rill vs. NGINX

Rill served about 10% more requests per second for a 4 KiB static file. For a
256 KiB static file, the two servers finished within 0.2% of each other.
The biggest difference appeared when forwarding a 256 KiB response from an
application: Rill handled more requests and used more frontend memory.

| Workload | Rill req/s | NGINX req/s | Rill p95 | NGINX p95 |
|---|---:|---:|---:|---:|
| Serve a 4 KiB file | 31,637 | 28,890 | 2.38 ms | 2.61 ms |
| Serve a 256 KiB file | 17,709 | 17,738 | 4.08 ms | 4.03 ms |
| Proxy a 4 KiB response | 24,240 | 23,119 | 3.12 ms | 3.08 ms |
| Proxy a 256 KiB response | 9,740 | 2,603 | 7.88 ms | 44.15 ms |

Requests per second measures throughput. The p95 is the time within which
95% of requests finished. Each entry is the median of three rounds.
[Individual rounds, memory use and response counts](nginx-rounds.csv)

## Why the proxy settings matter

Both frontends reused HTTP/1.1 connections to the same Rill origin. NGINX used
its default response buffering and logged temporary-file use during the
256 KiB test. Rill streamed the response. Median peak frontend memory for
that workload was **41.3 MiB for Rill and 11.0 MiB for NGINX**. At 4 KiB,
both used about 10 MiB and had similar latency. Different buffer settings,
network speeds or concurrency can change these results.

## Test setup

Rill v0.1.0 (release build) and NGINX 1.30.8 ran natively on Ubuntu under
WSL2 on an AMD Ryzen 9 9950X3D. Each frontend ran alone during its turn on
CPU 4 with one network worker. The origin ran on CPU 6. Four load-generator
threads on CPUs 8, 10, 12 and 14 kept 64 plaintext HTTP/1.1 connections open.
Files were warm in the OS cache. Both servers used sendfile for eligible
static responses. Access logs, compression and application caches were off.

Each round had two seconds of warmup and three seconds of measurement.
The driver alternated server order and checked the status and complete body
of every response. These tests cover single-core loopback traffic. TLS,
cold storage and multi-core scaling need separate measurements.

## Reproduce it

Use Linux, a release Rust toolchain, Python 3, NGINX, `taskset` and a
Lua-enabled `wrk`. Run from the repository root and adjust the CPU numbers
to distinct available cores on your machine.

```bash
cargo build --release --locked
for bytes in 4096 262144; do
  python3 bench/compare.py --nginx "$(command -v nginx)" \
    --payload-bytes "$bytes" --server-cpu 4 --backend-cpu 6 \
    --client-cpus 8 10 12 14 --connections 64 --warmup 2 --seconds 3 --repeat 3
done
```

The [benchmark driver](compare.py) generates the NGINX configuration and starts
both servers on temporary ports.
