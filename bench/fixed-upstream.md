# Fixed-upstream proxy performance on Apple M4

The final candidate (`b55bc5f`) ran with `RILL_FIXED_UPSTREAM=1`. The baseline was PR #3 (`77e8b43`), and the comparison server was NGINX 1.31.10. For native 4 KiB GETs, the throughput gap went from 9,666 to 3,216 requests/s, closing 66.7% of the original gap. The calculation uses the differences between the three six-round medians.

## Final opt-in proxy results

| Runtime | Response | Baseline req/s | Optimized req/s | NGINX req/s | Rill change | p95 ms, baseline / optimized / NGINX |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| macOS | 4 KiB | 70,522 | 76,972 | 80,187 | +9.1% | 1.040 / 0.958 / 0.867 |
| macOS | 256 KiB | 14,074 | 14,066 | 13,551 | -0.1% | 5.825 / 5.896 / 6.512 |
| Apple container | 4 KiB | 53,634 | 51,658 | 50,710 | -3.7% | 1.429 / 1.486 / 1.492 |
| Apple container | 256 KiB | 11,812 | 11,358 | 8,576 | -3.8% | 6.486 / 8.637 / 13.927 |

Optimized Rill beat baseline in all six native 4 KiB rounds. Native 256 KiB throughput changed by less than 1% in both passes. In Apple containers the 4 KiB throughput change ranged from +3.1% to −3.7% across the two passes. At 256 KiB both container runs showed lower throughput and higher p95 with the optimization. The default general client is the better choice for the measured large-response container workload.

## Workload and validation

The Mac mini had an Apple M4 with 10 cores, 16 GiB of memory and macOS 27.2.1. Native and Linux ARM64 Rill came from locked release builds using Rust 1.100.0. Apple `container` 1.2.0 gave each guest two vCPUs and 1 GiB RAM. The OS scheduled all processes without CPU pinning. Each proxy used one worker, and each Rill process had one blocking worker. Access logging, compression and proxy caching were disabled. NGINX used HTTP/1.1 upstream keep-alive and default proxy buffering. It reported spilling some native 256 KiB responses to temporary files.

The same macOS wrk 4.2.0 client used two threads and 64 persistent downstream connections. Each sample had a two-second warm-up and five measured seconds. Every frontend served each timing slot once over six proxy rounds. Both native Rill frontends and native NGINX shared one baseline Rill origin. Each guest had an identical baseline origin, guest-local fixture files and a published loopback port. Only the final candidate frontends received the tuning flag.

Throughput and p95 latency are medians of the per-round values. The driver checked every HTTP status and every response body and rejected samples containing errors. The two benchmark passes validated 37,822,517 measured responses. [Raw results for all 180 samples](macos-fixed-proxy-rounds.csv)

## Initial six-round proxy run and static controls

Revision `e2cdb15` ran the specialization before the startup switch was added. The large-response container result prompted the decision to make the tuning opt-in. The same baseline, NGINX, fixtures and client settings were used in both passes. Short runs on this shared Mac showed variation, especially through guest port forwarding.

| Runtime | Response | Baseline req/s | Optimized req/s | NGINX req/s | Rill change | p95 ms, baseline / optimized / NGINX |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| macOS | 4 KiB | 67,741 | 72,606 | 77,866 | +7.2% | 1.230 / 1.052 / 0.935 |
| macOS | 256 KiB | 13,585 | 13,509 | 13,323 | -0.6% | 6.086 / 6.183 / 6.575 |
| Apple container | 4 KiB | 46,392 | 47,845 | 46,772 | +3.1% | 1.696 / 1.814 / 1.769 |
| Apple container | 256 KiB | 11,112 | 10,346 | 10,028 | -6.9% | 7.184 / 10.021 / 11.892 |

Static controls used three rounds per frontend on `e2cdb15`. These requests exercised the frontend and file-serving path.

| Runtime | Response | Baseline req/s | Optimized req/s | NGINX req/s | Rill change | p95 ms, baseline / optimized / NGINX |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| macOS | 4 KiB | 162,894 | 172,821 | 50,128 | +6.1% | 0.455 / 0.422 / 1.314 |
| macOS | 256 KiB | 21,289 | 21,424 | 27,618 | +0.6% | 3.739 / 3.698 / 2.374 |
| Apple container | 4 KiB | 68,471 | 66,833 | 63,354 | -2.4% | 1.077 / 1.137 / 1.124 |
| Apple container | 256 KiB | 15,365 | 15,083 | 14,487 | -1.8% | 4.917 / 4.941 / 4.985 |

## Reproduce

The commands below exercise the final candidate. To reproduce the initial pass, substitute `e2cdb15` for the candidate revision.

The benchmark uses native NGINX 1.31.10, wrk 4.2.0 with Lua, Rust 1.100.0, and Apple `container` 1.2.0. Start Apple container services and run these commands from a checkout containing both measured commits. Separate build directories keep the identically named Rill packages distinct.

```sh
work=$(mktemp -d)
mkdir "$work/image" "$work/base" "$work/candidate"
git archive --format=tar.gz -o "$work/image/base.tgz" 77e8b43 Cargo.toml Cargo.lock src
git archive --format=tar.gz -o "$work/image/candidate.tgz" b55bc5f Cargo.toml Cargo.lock src
for build in base candidate; do
  tar -xf "$work/image/$build.tgz" -C "$work/$build"
  cargo build --release --locked --manifest-path "$work/$build/Cargo.toml" --target-dir "$work/$build/target"
done
cp bench/Dockerfile.fixed-proxy "$work/image/Dockerfile"
container build -t rill-fixed-bench:local "$work/image"
container builder stop
for workload in proxy static; do
  rounds=6
  if [ "$workload" = static ]; then rounds=3; fi
  python3 bench/macos.py --image rill-fixed-bench:local --nginx "$(command -v nginx)" \
    --baseline-rill "$work/base/target/release/rill" --rill "$work/candidate/target/release/rill" \
    --fixed-upstream --workload "$workload" --rounds "$rounds" > "$work/$workload.csv"
done
```
