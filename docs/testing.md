# Testing Japka

Run these commands from the repository root on Linux or macOS. Use stable Rust, Cargo, a C toolchain, Python 3.11 or newer, and an OpenSSL CLI that supports `req -addext`. CI runs on Linux and on Apple silicon and Intel macOS runners. The macOS jobs also package and preflight the release archive. The HTTP/3 check has additional client dependencies listed in the [TLS guide](edge.md#local-protocol-check).

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
export JAPKA="$PWD/target/release/japka"
```

## HTTP regression checks

Build Rust Anything to replay Japka's committed HTTP expectations.

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

The main suite replays 56 HTTP/1.1 requests against Japka and compares status, selected headers and body bytes with the committed fixtures. The responses were recorded directly from Japka using temporary files with fixed contents and timestamps and loopback test backends. It covers file lookup and errors, GET and HEAD, single byte ranges, conditional requests, and proxy methods, payloads and round-robin selection. Edit the inputs and expected responses together when intentionally changing the HTTP contract. Rust Anything skips some headers, including ETag, Last-Modified and Content-Length. The static size check verifies these headers directly.

Set `JAPKA` to select the binary. The scripts default to `target/debug/japka`. To check both Linux file-serving modes, run:

```sh
JAPKA_SENDFILE=1 python3 checks/compare.py
JAPKA_SENDFILE=0 python3 checks/compare.py
```

For the opt-in single-upstream transport, set `JAPKA_FIXED_UPSTREAM=1` when running the policy, reload, WebSocket and proxy streaming checks. The upstream suite also exercises connection reuse, retirement and backend disconnects.

## Run the socket checks

```sh
python3 checks/static_updates.py
python3 checks/mixed_io.py
python3 checks/http1_buffers.py
python3 checks/proxy_streaming.py
python3 checks/policy.py
python3 checks/security.py
python3 checks/streams.py "$JAPKA"
```

On Linux, also exercise direct file transfer and inspect cached file descriptors through `/proc`:

```sh
python3 checks/sendfile.py
python3 checks/asset_cache.py
```

These checks start loopback listeners and use temporary fixtures. The stream check exercises TCP, UDP and mail backends. See the [TLS guide](edge.md) for the separate HTTPS and QUIC check and the [Mac mini benchmark](../bench/README.md) for native and container throughput and latency.

## Native Windows file delivery

From the checkout in PowerShell, point `JAPKA` at an absolute Windows executable path and run `python checks/windows_transmitfile.py`. The check enables TransmitFile, holds two large downloads open while a third completes, then resets the slow connections. It also exercises pipelining, conditional and range responses, and the gzip fallback.

## Feature acceptance

Run `./checks/features.sh` from the repository root. It builds a locked debug binary unless `JAPKA` selects an existing one. The suite covers HTTP features, NGINX migration and the configuration review workflow. To run only the review check:

```sh
python3 checks/config_review.py
```

On Unix this check starts a local service, adds a route while preserving an inherited route, rejects stale approval inputs, rolls back after a failed probe and retries an interrupted rollout. [Configuration review guide](config-review.md)

These checks use loopback connections and temporary fixtures. With the optional HTTP/3 dependencies from the [TLS guide](edge.md#local-protocol-check), run `python3 checks/reload.py --http3` to check QUIC routing and default certificate rotation. Run `python3 checks/edge_routing.py --http3` to verify a named certificate over QUIC after rotation. The [feature guide](features.md) describes the client and operator behaviors they verify.
