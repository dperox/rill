# Japka and NGINX on a Mac mini

The same M4 Mac mini ran Japka 0.2.0 and NGINX 1.31.10 as native macOS processes and in Linux ARM64 guests using Apple’s `container`. A client on the Mac fetched warm files directly from each server and through its reverse proxy.

## Results

Throughput is in requests per second. Latency is the 95th percentile in milliseconds. Each value is the median of three rounds. All 10,894,091 measured responses returned HTTP 200 and matched the expected bytes. [Per-round measurements](macos-nginx-rounds.csv)

| Deployment | HTTP/1.1 GET | Japka req/s | NGINX req/s | Japka p95 ms | NGINX p95 ms |
| --- | --- | ---: | ---: | ---: | ---: |
| Native | Static 4 KiB | 159,861 | 49,287 | 0.466 | 1.328 |
| Native | Proxy 4 KiB | 68,538 | 78,492 | 1.071 | 0.881 |
| Native | Static 256 KiB | 21,349 | 27,617 | 3.717 | 2.347 |
| Native | Proxy 256 KiB | 14,007 | 13,469 | 5.882 | 6.583 |
| Apple container | Static 4 KiB | 69,395 | 64,308 | 1.042 | 1.094 |
| Apple container | Proxy 4 KiB | 46,233 | 51,604 | 1.638 | 1.466 |
| Apple container | Static 256 KiB | 14,508 | 14,143 | 5.023 | 4.982 |
| Apple container | Proxy 256 KiB | 11,733 | 8,968 | 6.528 | 14.188 |

For native 4 KiB requests, Japka served static files at 3.24 times NGINX’s throughput, while NGINX handled more proxied requests. In containers, NGINX also led on 4 KiB proxy requests. With 256 KiB proxy responses, Japka handled more requests per second in both deployments.

## Test conditions

- **Host.** Mac mini, Apple M4 (4 performance and 6 efficiency cores), 16 GB RAM, macOS 27.2.1, November 27, 2026. macOS scheduled the client, native servers, and guests without CPU pinning.
- **Binaries.** The native Japka binary came from the macOS ARM64 release archive, and the Linux binary came from the GHCR image for the same commit (`8980f9d`). Native NGINX came from Homebrew. Both guests used the same comparison image with Japka and official NGINX binaries. [Japka release and checksums](https://github.com/japka-rs/japka/releases/tag/v0.2.0) · [Pinned input images](Dockerfile.macos-nginx)
- **Client.** wrk 4.2.0 used two threads and 64 persistent connections. Each sample had two seconds of warm-up followed by five seconds of measurement. The four servers ran in reverse order in the second round. Latency includes the client’s connection to the native loopback listener or the guest’s published port.
- **Servers.** Japka used one network worker and one blocking worker. NGINX used one worker, sendfile, upstream HTTP/1.1 keep-alive and its default proxy buffering. Compression and access logging were disabled. The two native proxies shared a Japka origin. Each guest ran an equivalent origin on guest loopback.
- **Containers.** Apple `container` 1.2.0 assigned each guest two vCPUs and 1 GiB of memory. Each guest served a copy of the fixture files from its own filesystem.

## Reproduce the comparison

Run from the repo root on Apple silicon with Python 3, GitHub CLI, Lua-enabled wrk, Apple `container`, and native NGINX matching the version above. These commands use the same Japka binaries as the recorded run.

```sh
work=$(mktemp -d)
gh release download v0.2.0 --repo japka-rs/japka --dir "$work" --pattern 'japka-0.2.0-macos-arm64.tar.gz*'
(cd "$work" && shasum -a 256 -c *.sha256 && tar -xzf japka-0.2.0-macos-arm64.tar.gz)
container system start
container build --platform linux/arm64 -f bench/Dockerfile.macos-nginx -t japka-nginx:mac-bench .
python3 bench/macos.py --japka "$work/japka/japka" > "$work/rounds.csv"
```
