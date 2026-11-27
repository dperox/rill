# Rill vs. NGINX

## Throughput by core count

Median requests per second with 64 clients. The last column divides the four-core result by the one-core result for the same server and workload.

| Workload | Server | 1 core | 2 cores | 4 cores | 4 cores / 1 core |
|---|---|---:|---:|---:|---:|
| Static 4 KiB | Rill | 31,637 | 66,054 | 160,518 | 5.07× |
| Static 4 KiB | NGINX | 28,890 | 68,321 | 201,334 | 6.97× |
| Static 256 KiB | Rill | 17,709 | 48,238 | 56,604 | 3.20× |
| Static 256 KiB | NGINX | 17,738 | 58,068 | 53,144 | 3.00× |
| Proxy 4 KiB | Rill | 24,240 | 39,377 | 78,741 | 3.25× |
| Proxy 4 KiB | NGINX | 23,119 | 58,921 | 60,436 | 2.61× |
| Proxy 256 KiB | Rill | 9,740 | 12,940 | 24,940 | 2.56× |
| Proxy 256 KiB | NGINX | 2,603 | 5,675 | 13,320 | 5.12× |

## One frontend core

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

## Two and four frontend cores

With four cores, NGINX led on 4 KiB static files (201k vs. 161k requests/s).
Rill led on 256 KiB proxy responses (24.9k vs. 13.3k requests/s) and had
lower p95 latency on that workload (4.00 vs. 8.64 ms).

| Frontend cores | Workload | Rill req/s | NGINX req/s | Rill p95 | NGINX p95 |
|---:|---|---:|---:|---:|---:|
| 2 | Serve 4 KiB | 66,054 | 68,321 | 1.40 ms | 1.53 ms |
| 2 | Serve 256 KiB | 48,238 | 58,068 | 1.79 ms | 1.20 ms |
| 2 | Proxy 4 KiB | 39,377 | 58,921 | 2.37 ms | 1.60 ms |
| 2 | Proxy 256 KiB | 12,940 | 5,675 | 8.51 ms | 19.78 ms |
| 4 | Serve 4 KiB | 160,518 | 201,334 | 0.80 ms | 0.55 ms |
| 4 | Serve 256 KiB | 56,604 | 53,144 | 1.37 ms | 1.14 ms |
| 4 | Proxy 4 KiB | 78,741 | 60,436 | 1.36 ms | 1.35 ms |
| 4 | Proxy 256 KiB | 24,940 | 13,320 | 4.00 ms | 8.64 ms |

Both servers used the same set of physical cores, with 64 clients throughout.
Rill used one configured network worker per core. NGINX used one worker
process per core and `reuseport` to distribute connections. The origin stayed
on one core and the load generator on four, so proxy results measure the
whole request path as the frontend gains cores. Multi-process NGINX RSS in
the raw data sums the master and workers and can count shared pages more than once.
[All 48 multi-core samples](nginx-multicore-rounds.csv)

## Why the proxy settings matter

Both frontends reused HTTP/1.1 connections to the same Rill origin. NGINX used
its default response buffering and logged temporary-file use during the
256 KiB test. Rill streamed the response. Median peak frontend memory for
that workload was **41.3 MiB for Rill and 11.0 MiB for NGINX**. At 4 KiB,
both used about 10 MiB and had similar latency. Different buffer settings,
network speeds or concurrency can change these results.

## Test setup

Rill v0.1.0 (release build) and NGINX 1.30.8 ran natively on Ubuntu under
WSL2 on an AMD Ryzen 9 9950X3D. The single-core run used CPU 4. The two-core run used CPUs 4 and 2, and
the four-core run used CPUs 4, 2, 0 and 16 (one logical CPU per physical core). The origin ran on CPU 6. Four load-generator
threads on CPUs 8, 10, 12 and 14 kept 64 plaintext HTTP/1.1 connections open.
Files were warm in the OS cache. Both servers used sendfile for eligible
static responses. Access logs, compression and application caches were off.

Each round had two seconds of warmup and three seconds of measurement.
The driver alternated server order and checked the status and complete body
of every response. These tests cover warm loopback traffic. TLS, cold storage and external
networks can produce different results.

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

For the two- and four-core runs, replace `--server-cpu 4` with
`--server-cpus 4 2` or `--server-cpus 4 2 0 16`. Keep the other arguments fixed.

The [benchmark driver](compare.py) generates the NGINX configuration and starts
both servers on temporary ports.
