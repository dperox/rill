# Testing Rill

Run these commands from the repository root on Linux or macOS. Use stable Rust, Cargo, a C toolchain, Python 3.11 or newer, and an OpenSSL CLI that supports `req -addext`. CI runs on Linux. The HTTP/3 check has additional client dependencies listed in the [TLS guide](edge.md#local-protocol-check).

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
export RILL="$PWD/target/release/rill"
```

## HTTP regression checks

Build Rust Anything to replay Rill's committed HTTP expectations.

```sh
git clone https://github.com/erkamyaman/rust-anything.git ../rust-anything-toolchain
git -C ../rust-anything-toolchain checkout 5b77ef9190a9e9eac92a2b7f796adbe1e2b35868
(cd ../rust-anything-toolchain && cargo build --locked)
export RUST_ANYTHING="$(realpath ../rust-anything-toolchain/target/debug/rust-anything)"
```

## Run the HTTP checks

```sh
python3 checks/compare.py
python3 checks/routing.py
python3 checks/proxy_failure.py
python3 checks/upstreams.py
python3 checks/static_sizes.py
```

The main suite replays 56 HTTP/1.1 requests against Rill and compares status, selected headers and body bytes with the committed fixtures. The responses were recorded directly from Rill using temporary files with fixed contents and timestamps and loopback test backends. It covers file lookup and errors, GET and HEAD, single byte ranges, conditional requests, and proxy methods, payloads and round-robin selection. Edit the inputs and expected responses together when intentionally changing the HTTP contract. Rust Anything skips some headers, including ETag, Last-Modified and Content-Length. The static size check verifies these headers directly.

Set `RILL` to select the binary. The scripts default to `target/debug/rill`. To check both Linux file-serving modes, run:

```sh
RILL_SENDFILE=1 python3 checks/compare.py
RILL_SENDFILE=0 python3 checks/compare.py
```

## Run the socket checks

```sh
python3 checks/static_updates.py
python3 checks/mixed_io.py
python3 checks/http1_buffers.py
python3 checks/proxy_streaming.py
python3 checks/policy.py
python3 checks/streams.py "$RILL"
```

On Linux, also exercise direct file transfer and inspect cached file descriptors through `/proc`:

```sh
python3 checks/sendfile.py
python3 checks/asset_cache.py
```

These checks start loopback listeners and use temporary fixtures. The stream check exercises TCP, UDP and mail backends. See the [TLS guide](edge.md) for the separate HTTPS and QUIC check and the [benchmark guide](../bench/README.md) for throughput and idle-memory comparisons.

## Feature acceptance

Run `./checks/features.sh` from the repository root. It builds a locked debug binary, then runs the reload, WebSocket, static site, access log, redirect, SNI, failover and migration CLI acceptance checks against that binary. Set `RILL` to an absolute binary path to test an existing build instead.

These checks use loopback connections and temporary fixtures. With the optional HTTP/3 dependencies from the [TLS guide](edge.md#local-protocol-check), run `python3 checks/reload.py --http3` to check QUIC routing and default certificate rotation. Run `python3 checks/edge_routing.py --http3` to verify a named certificate over QUIC after rotation. The [feature guide](features.md) describes the client and operator behaviors they verify.
