# Japka performance

Measure static file delivery and reverse-proxy throughput against a native binary or Apple container. The suite checks the status and bytes of every measured response. [Run the end-to-end suite](#run-the-end-to-end-suite)

## Native macOS and Apple containers

The published Japka 0.3.0 macOS ARM64 binary and Linux ARM64 image ran on an M4 Mac mini with 16 GiB RAM, macOS 27.2.1, and Apple `container` 1.2.0. The container had two vCPUs and 1 GiB RAM. Each Japka process used one network worker and one blocking worker. Both environments used the default upstream client.

The table shows medians from three Goose runs per environment with 64 concurrent users, two seconds of warm-up, and five measured seconds per workload. The client ran on the Mac and measured complete HTTP/1.1 responses over loopback or the guest’s forwarded port. All 6,089,368 measured responses returned HTTP 200 and matched the expected bytes. [Per-run measurements](performance-v0.3.0.csv) · [Release artifacts](https://github.com/japka-rs/japka/releases/tag/v0.3.0)

| HTTP/1.1 GET | Native req/s | Native p95 (ms) | Container req/s | Container p95 (ms) |
| --- | ---: | ---: | ---: | ---: |
| Static 4 KiB | 139,020 | 0.564 | 62,677 | 1.352 |
| Proxy 4 KiB | 76,033 | 0.988 | 44,954 | 1.704 |
| Static 256 KiB | 22,115 | 3.472 | 13,974 | 5.536 |
| Proxy 256 KiB | 14,430 | 5.792 | 10,912 | 7.136 |

## Fixed-upstream comparison

In a native 4 KiB HTTP/1.1 proxy benchmark on an M4 Mac mini, Japka served **86,782 requests/s** with its fixed upstream pool at **0.845 ms** p95. NGINX served **90,187 requests/s** in the same setup, putting Japka at **96.2%** of its throughput.

| Frontend | Median req/s | Median p95 (ms) |
| --- | ---: | ---: |
| Japka, default upstream client | 81,206 | 0.904 |
| Japka, fixed upstream pool | 86,782 | 0.845 |
| NGINX 1.31.10 | 90,187 | 0.775 |

Set `JAPKA_FIXED_UPSTREAM=1` at startup for eligible GET routes backed by a single plaintext IP address and port. [Fixed upstream connections](routing.md#fixed-upstream-connections)

The test used macOS 27.2.1, one network worker per frontend, and a shared loopback origin. A two-thread wrk client opened 64 persistent connections. Each sample used two seconds of warm-up and five measured seconds. The table reports medians across five rounds with rotated frontend order. The three configurations shown validated 6,623,306 measured responses against the expected HTTP 200 status and fixture bytes. [Raw samples](performance-pr5-fixed.csv)

## Run the end-to-end suite

The standalone Goose client in `perf/` runs each of the four workloads separately. It uses HTTP/1.1 keep-alive, reads every body, and verifies HTTP 200 and every fixture byte. Each transaction covers one complete response, including validation. A bad status, wrong protocol, truncated transfer, or changed byte fails the run. Japka’s published Cargo package has no load-test dependencies. [Goose client](../perf/src/main.rs) · [Suite launcher](../perf/run.py)

From the repository root, build the server and client once. The smoke check starts a temporary frontend and origin and sends ten requests per workload. Linux CI runs this correctness check on pushes and pull requests.

```sh
cargo build --release --locked
cargo build --manifest-path perf/Cargo.toml --release --locked
python3 perf/run.py --japka target/release/japka --smoke
```

For a timed run, select the native binary or an Apple container image. The Mac runs the same load client in both cases. The container launcher puts the origin inside the frontend's guest and publishes the frontend on loopback. The following commands use 64 concurrent users, two seconds of warm-up, and ten measured seconds per workload. They create a new directory for each run.

```sh
runs=$(mktemp -d)
python3 perf/run.py --japka target/release/japka --users 64 --warmup 2 --seconds 10 --output "$runs/native"

# With Apple's container service running on an ARM64 Mac
container build --platform linux/arm64 -t japka:perf -f deploy/Dockerfile .
python3 perf/run.py --image japka:perf --users 64 --warmup 2 --seconds 10 --output "$runs/guest"
```

Each case produces a Goose JSON report and a compact summary with the validated response count, requests/s and **full-transaction p95 in microseconds**. The suite computes timed throughput over the measured hold and the completion of in-flight requests, using microsecond phase timestamps. Goose's built-in HTTP request percentiles measure time to response headers. Use the transaction p95 when comparing complete transfers. `run.json` records the conditions of a successful suite. Use `--case proxy-4k` to time one workload and `--output` to choose an unused directory. [Goose response timing](https://docs.rs/goose/0.19.2/goose/goose/struct.GooseResponse.html#method.discard_body)

### Compare revisions

Run the same client against the baseline and candidate binaries or images on the same host. Alternate their order and keep the native and container comparisons separate. The command below compares the per-case medians of three timed runs per build. With the optional 5% tolerance it exits with status 1 if throughput falls or p95 rises by more than 5% for any case. Without a tolerance it prints the changes. Shared CI runners use the smoke check because their timing varies. [Comparison tool](../perf/compare.py)

```sh
python3 perf/compare.py \
  --baseline "$runs/baseline-1" "$runs/baseline-2" "$runs/baseline-3" \
  --candidate "$runs/candidate-1" "$runs/candidate-2" "$runs/candidate-3" \
  --max-regression 5
```

For Apple container comparisons, keep the guest CPU and memory limits and the host network path consistent across runs.

### Single-URL measurements

**oha 1.17.0** measures one URL and reports throughput, full-response p95 and p99, HTTP status counts, and transport errors in JSON. Set `url` to the fixture URL and `fixture` to its local file. The command checks the fixture bytes once before the run and verifies the status and error counts afterward. [oha usage](https://github.com/hatoo/oha/tree/v1.17.0#usage) · [JSON fields and units](https://github.com/hatoo/oha/blob/v1.17.0/schema.json) · [Full-body timing](https://github.com/hatoo/oha/blob/v1.17.0/src/client.rs#L711-L749)

```sh
set -eu
cargo install --locked oha --version 1.17.0
curl -fsS "$url" | cmp "$fixture" -
oha --http-version 1.1 --disable-compression --no-tui --output-format json \
  -c 64 -z 5s -w -t 2s "$url" > oha.json
jq -e 'select((.statusCodeDistribution | keys) == ["200"] and
              .errorDistribution == {} and .metrics.requests_per_sec > 0) |
       {responses: .statusCodeDistribution["200"], requests_per_sec: .metrics.requests_per_sec,
        p95_ms: .metrics.latency_ms.p95, p99_ms: .metrics.latency_ms.p99}' oha.json
```
