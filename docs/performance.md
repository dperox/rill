# Japka performance

On November 28, 2026, the cleanup changed native macOS throughput by less than 0.4% across all four workloads. On Apple containers, three workloads had higher throughput and lower p95 latency. The initial 4 KiB proxy test showed an 18.22% throughput drop and a 22.06% p95 increase across separate guests. All three follow-ups reversed both median changes. With the frontend and origin assigned to the same guest CPUs for both builds, the cleanup had 0.89% higher median throughput and 0.78% lower median p95. The follow-ups point to guest-to-guest and run-to-run variation as the likely explanation for the initial result.

## Native macOS and Apple containers

Each value is the median of four measured rounds. Throughput is in requests per second, and p95 is the 95th-percentile request latency in milliseconds. Changes are relative to the baseline and use the unrounded medians.

| Platform | HTTP/1.1 GET | Baseline req/s | Cleanup req/s | Change | Baseline p95 ms | Cleanup p95 ms | Change |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Native macOS | Static 4 KiB | 162,979 | 162,389 | −0.36% | 0.4590 | 0.4565 | −0.54% |
| Native macOS | Proxy 4 KiB | 69,648 | 69,591 | −0.08% | 1.0535 | 1.0575 | +0.38% |
| Native macOS | Static 256 KiB | 21,408 | 21,334 | −0.35% | 3.6935 | 3.7015 | +0.22% |
| Native macOS | Proxy 256 KiB | 13,965 | 13,954 | −0.08% | 5.8825 | 5.9150 | +0.55% |
| Apple container | Static 4 KiB | 69,313 | 70,127 | +1.17% | 1.0705 | 1.0395 | −2.90% |
| Apple container | Proxy 4 KiB | 57,120 | 46,714 | −18.22% | 1.3325 | 1.6265 | +22.06% |
| Apple container | Static 256 KiB | 14,757 | 14,859 | +0.70% | 4.9390 | 4.9190 | −0.40% |
| Apple container | Proxy 256 KiB | 11,539 | 11,740 | +1.74% | 6.7035 | 6.4875 | −3.22% |

The published Japka 0.2.0 run on the same host recorded 46,233 req/s and 1.638 ms p95 for the container 4 KiB proxy, close to the cleanup’s initial result. Across all eight workloads, cleanup throughput was at most 0.4% lower and p95 at most 0.6% higher than the published run. The archived benchmark includes the fixture generator, validation script, raw samples, and reproduction commands. [Japka 0.2.0 benchmark](https://github.com/japka-rs/japka/blob/dbad8f8ab62a2b39164bd6b3942773f0e347a0db/bench/README.md)

## Apple-container 4 KiB proxy follow-ups

The initial comparison gave each build a separate guest and forwarded port. After round two, both builds restarted with new guests and ports and in reverse startup order. Measurement order alternated each round. The follow-ups used the same forwarded port for both builds and alternated their order within successive pairs. Each Japka proxy sample used a frontend and origin from the same build.

| Setup | Pairs | Baseline req/s | Cleanup req/s | Change | Baseline p95 ms | Cleanup p95 ms | Change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| One guest, restart both processes per sample | 8 | 48,862 | 51,784 | +5.98% | 1.5990 | 1.5020 | −6.07% |
| Fresh guest per sample | 8 | 50,391 | 53,709 | +6.58% | 1.5795 | 1.4700 | −6.93% |
| One guest, frontend pinned to vCPU 0 and origin to vCPU 1 | 8 | 47,675 | 48,100 | +0.89% | 1.5995 | 1.5870 | −0.78% |

In the pinned run, throughput changed by −0.28% to +1.13% across individual pairs. The geometric mean of paired ratios estimated +0.44% throughput (90% confidence interval +0.06% to +0.82%) and −0.63% p95 (−1.29% to +0.04%). These intervals assume independent rounds and approximately normal log ratios and describe one host and configuration.

## Method and scope

- **Builds.** Both locked release builds reported Japka 0.2.2. The baseline was `dbad8f8ab62a2b39164bd6b3942773f0e347a0db`. The measured cleanup was committed as `f85e2d1`. Native ARM64 builds used Rust 1.100.1 and Linux ARM64 builds used Rust 1.100.0. [Cleanup revision](https://github.com/japka-rs/japka/commit/f85e2d13a4c5d2dfdbd0149d979b47017aa31e4f)
- **Host and load.** An Apple M4 Mac mini with 16 GiB RAM ran macOS 27.2.1 and Apple `container` 1.2.0 with two configured vCPUs and 1 GiB RAM per guest. Japka processes each used one network worker and one blocking worker. The client, wrk 4.2.0, used two threads and 64 persistent connections to the native loopback listener or the guest’s forwarded port.
- **Fixtures and checks.** The client fetched warm 4 KiB and 256 KiB files made from repeated `0123456789abcdef` bytes, directly and through a reverse proxy. After two seconds of warm-up, it measured five seconds per sample in the initial and fresh-guest runs and eight seconds in the two single-guest follow-ups. All 144 measured samples passed the socket, timeout, and response-count checks. This includes NGINX 1.31.10 reference samples from the initial comparison. All **40,207,900 measured responses** returned HTTP 200 and matched the expected bytes. [Per-sample measurements](performance-samples.csv)
- **Scope.** The results cover warm plaintext HTTP/1.1 static and proxy requests at concurrency 64 on this host. TLS, HTTP/2, and HTTP/3 were outside the test scope.

## Aviv’s native GET benchmark

Aviv measured the header-removal and GET-dispatch changes on an M4 Mac mini running macOS 26.6.2 with Rust 1.98.1. The native harness used the default upstream client, one network worker, one blocking worker, and a shared Japka origin. The table gives the medians of three rounds with 64 persistent HTTP/1.1 connections. All 9,775,647 responses returned HTTP 200 and matched the fixture. These measurements used a different host setup from the cleanup comparison above. [PR #5](https://github.com/japka-rs/japka/pull/5) · [Per-round measurements](performance-pr5-native.csv) · [Harness at the measured revision](https://github.com/japka-rs/japka/blob/649b4534a1fdaa2e09052b4576ed1d922c4e165f/bench/macos.py)

| HTTP/1.1 GET | Japka req/s | NGINX req/s | Japka p95 ms | NGINX p95 ms |
| --- | ---: | ---: | ---: | ---: |
| Static 4 KiB | 237,814 | 62,577 | 0.296 | 1.046 |
| Proxy 4 KiB | 107,956 | 124,784 | 0.676 | 0.577 |
| Static 256 KiB | 25,744 | 44,196 | 2.830 | 1.487 |
| Proxy 256 KiB | 16,811 | 15,197 | 4.554 | 5.065 |

The 4 KiB proxy result was 86.5% of NGINX throughput. The dedicated upstream pool remains an opt-in setting for eligible single-peer routes. [Fixed-upstream configuration](routing.md)

### Fixed-upstream comparison

A separate M4 Mac mini running macOS 27.2.1 compared the release build of `e949c39` against the integrated PR. All five frontends used the same loopback Japka 0.2.0 origin. The client fetched the same 4 KiB proxied fixture with two wrk threads and 64 persistent connections. Each frontend occupied each timing slot once across five rounds, with two seconds of warm-up and five measured seconds per sample. Both Japka builds used Rust 1.100.1 and one network worker. NGINX 1.31.10 used one worker.

| Native 4 KiB proxy | Median req/s | Median p95 ms | Throughput relative to NGINX |
| --- | ---: | ---: | ---: |
| Before PR, default client | 77,394 | 0.947 | 85.8% |
| After PR, default client | 81,206 | 0.904 | 90.0% |
| Before PR, fixed pool | 84,758 | 0.871 | 94.0% |
| After PR, fixed pool | 86,782 | 0.845 | 96.2% |
| NGINX | 90,187 | 0.775 | 100.0% |

With the default client, the PR raised median throughput by 4.93%. With `JAPKA_FIXED_UPSTREAM=1`, it raised median throughput by 2.39%. All 10,767,382 measured responses matched the expected HTTP 200 status and bytes. These are end-to-end throughput and latency measurements. [All warm-up and measured samples](performance-pr5-fixed.csv)

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
  --baseline "$runs/before-1" "$runs/before-2" "$runs/before-3" \
  --candidate "$runs/after-1" "$runs/after-2" "$runs/after-3" \
  --max-regression 5
```

The published measurements above used wrk. Establish a fresh baseline with Goose before comparing changes in throughput or latency. If an Apple-container regression appears, check whether it persists when both builds use the same guest and forwarded port.

### Single-URL measurements

**oha 1.17.0 is a smaller option for measuring one URL.** It reports throughput and full-response p95 and p99 in JSON. Set `url` to the native or published fixture URL and `fixture` to the corresponding local file. The preflight below checks the bytes once. During the load run, oha reports status and transport errors. An HTTP 503 can still produce exit status 0, so check both distributions. [oha usage](https://github.com/hatoo/oha/tree/v1.17.0#usage) · [JSON fields and units](https://github.com/hatoo/oha/blob/v1.17.0/schema.json) · [Full-body timing](https://github.com/hatoo/oha/blob/v1.17.0/src/client.rs#L711-L749)

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
