# Fork-free HTTP/1 static listener prototype

Base: Rill `351ace3`. Stock Hyper 1.13.0 serves the regular listener. Set
`RILL_STATIC_LISTEN` to bind an additional plaintext HTTP/1 static listener
driven by http-kit-h1 0.6.2. Both use the existing router, static-file
selection and middleware. The dedicated listener handles bodyless GET and HEAD
and returns 404 for a matched proxy route. Its connections run on one Tokio
LocalSet. This Linux experiment does not implement Windows TransmitFile or a
single-port replacement. The original fork-specific initial allocations are
absent.

```sh
cargo build --release --locked
DOCUMENT_ROOT=public LISTEN=127.0.0.1:8080 RILL_STATIC_LISTEN=127.0.0.1:8081 target/release/rill
```

### Measurements

WSL2, Ryzen 9 9950X3D, warm named files, 64 concurrent HTTP/1.1 clients,
one frontend worker/core, client CPUs 8, 10, 12, 14. Three alternating rounds
with two seconds of warmup and three seconds measured. All response bodies
were checked. Entries below are medians.

| File | Driver | Requests/s | p95 ms | Peak frontend RSS MiB |
|---|---|---:|---:|---:|
| 256 KiB | http-kit | 16,025 | 5.367 | 9.23 |
| 256 KiB | patched Hyper | 15,991 | 5.466 | 7.51 |
| 4 KiB | http-kit | 23,049 | 4.339 | 9.43 |
| 4 KiB | patched Hyper | 29,336 | 2.760 | 7.75 |

At 4,096 idle connections, process RSS was 76.07 MiB with http-kit and
32.60 MiB with patched Hyper. Both retained all connections, served all 8,192
pre/post-idle responses and used zero measured CPU ticks during five seconds
idle. The prototype process also holds an unused ordinary listener.

### Reproduce

Build the unmodified base in a separate checkout and set `PATCHED` to the
absolute path to its release binary. Set `WRK` to a Lua-enabled wrk if it is
outside PATH. Run from the prototype root on a machine allowing these CPUs.

```sh
RILL="$PWD/bench/kit-static-listener" python3 checks/sendfile.py
for size in 262144 4096; do
  python3 bench/compare.py --rill bench/kit-static-listener --baseline-rill "$PATCHED" \
    --workload static --server-cpu 4 --backend-cpu 6 --client-cpus 8 10 12 14 \
    --seconds 3 --payload-bytes "$size"
done
for binary in bench/kit-static-listener "$PATCHED"; do
  python3 bench/connections.py --rill "$binary" --server-cpu 4 --client-cpus 8 10 12 14 --connections 4096
done
```

The static listener passed the repository sendfile suite in both modes. With
`strace -f -e trace=sendfile`, a verified 256 KiB GET produced one successful
262,144-byte sendfile call in mode 1 and no sendfile calls in mode 0.
