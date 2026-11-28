# HTTP benchmarks

Use the drivers below to measure the current Japka build. The linked reports record measurements taken under the project’s former name, Rill. Their revisions, release numbers and raw samples identify those historical binaries.

| Workload | Measurements |
| --- | --- |
| Linux static files and reverse proxy | [Throughput, latency and memory against NGINX](http-kit-static.md) · [Sendfile and immutable asset cache](sendfile.md) |
| HTTPS through Kubernetes ingress | [Three-replica load and rolling updates](kubernetes/README.md) |
| Apple silicon | [Native and Apple container results](macos.md) · [Fixed-upstream GETs](fixed-upstream.md) |
| Windows and WSL2 | [Native loopback and direct file delivery](windows.md) |

## Throughput and latency

Run from the repository root on Linux with Python 3, `taskset`, and `wrk` with Lua support. Choose distinct permitted CPUs on separate cores using `lscpu -e=CPU,CORE`. Use `--server-cpu 4` for one frontend core or, for example, `--server-cpus 4 2 0 16` for four. Japka and NGINX share the same CPU placement in alternating runs, with one configured network worker per selected core. Proxy workloads use a separate Japka process to serve the upstream file on another core. The driver verifies the status and body of each response and samples frontend process RSS every 200 ms.

```bash
cargo build --release --locked
pins=(--server-cpu 4 --backend-cpu 6 --client-cpus 8 10 12 14)
python3 bench/compare.py "${pins[@]}" --seconds 3
python3 bench/compare.py "${pins[@]}" --seconds 3 --workload static --payload-bytes 262144 --sendfile-ab
python3 bench/compare.py "${pins[@]}" --seconds 3 --asset-cache-ab --payload-bytes 262144
```

The default is 64 clients and three rounds. Use `--baseline-japka` to compare an earlier Japka binary. Use `--static-layout index` for a directory index or `--static-layout many` for 64 rotating filenames. Asset-cache comparisons use fingerprinted filenames. Set `--wrk` or `WRK` to select the load generator.

## Idle connections

```sh
python3 bench/connections.py --server-cpu 4 --client-cpus 8 10 12 14 --connections 4096
```

The driver checks a complete 4 KiB response on each socket before and after a
five-second idle interval. It reports the number of sockets still held, process
RSS and CPU ticks. RSS excludes kernel socket buffers and the filesystem page
cache. Use `--japka` to choose a different Japka binary.

These tests use warm files over loopback. Keep CPU placement, concurrency,
body size and server configuration fixed when comparing measurements.
