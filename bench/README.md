# HTTP benchmarks

Use [compare.py](compare.py) for throughput, p95 latency and peak front-end RSS,
and [connections.py](connections.py) for idle persistent connections. The
[Linux sendfile report](sendfile.md) covers the immutable asset cache, sendfile, and TCP socket settings.

## Throughput and latency

Run from the repository root on Linux or WSL2 with Python 3, `taskset`, and `wrk` with Lua support. Choose distinct permitted CPUs on separate cores using `lscpu -e=CPU,CORE`. Rill runs on one frontend core. Proxy workloads use a separate Rill process to serve the upstream file on another core. The driver verifies the status and body of each response and samples frontend process RSS every 200 ms.

```bash
cargo build --release --locked
pins=(--server-cpu 4 --backend-cpu 6 --client-cpus 8 10 12 14)
python3 bench/compare.py "${pins[@]}" --seconds 3
python3 bench/compare.py "${pins[@]}" --seconds 3 --workload static --payload-bytes 262144 --sendfile-ab
python3 bench/compare.py "${pins[@]}" --seconds 3 --asset-cache-ab --payload-bytes 262144
```

The default is 64 clients and three rounds. Use `--baseline-rill` to compare an earlier Rill binary. Use `--static-layout index` for a directory index or `--static-layout many` for 64 rotating filenames. Asset-cache comparisons use fingerprinted filenames. Set `--wrk` or `WRK` to select the load generator.

## Idle connections

```sh
python3 bench/connections.py --server-cpu 4 --client-cpus 8 10 12 14 --connections 4096
```

The driver checks a complete 4 KiB response on each socket before and after a
five-second idle interval. It reports the number of sockets still held, process
RSS and CPU ticks. RSS excludes kernel socket buffers and the filesystem page
cache. Use `--rill` to choose a different Rill binary.

These tests use warm files over loopback. Keep CPU placement, concurrency,
body size and server configuration fixed when comparing measurements.
