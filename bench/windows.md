# Native Windows and WSL2 baseline

Rill release `7082e92`, AMD Ryzen 9 9950X3D, Windows 11 Pro build 26300 and Ubuntu WSL2. The same HTTP/1.1 client source was compiled for each OS. Both ends ran on local loopback. The frontend used one logical CPU, the optional Rill backend a separate core, and the 64 keep-alive clients four other cores. Each of three rounds used two seconds of warm-up and five seconds of measurement, checking status, length and every body. Files were warm, with access logging, gzip and application caches disabled.

| Workload | Windows requests/s | WSL2 requests/s | Windows p95 ms | WSL2 p95 ms |
| --- | ---: | ---: | ---: | ---: |
| Static 4 KiB | 21,068 | 25,957 | 3.29 | 2.95 |
| Proxy 4 KiB | 37,953 | 22,038 | 1.95 | 3.24 |
| Static 256 KiB | 7,082 | 13,889 | 9.93 | 5.72 |
| Proxy 256 KiB | 7,559 | 9,806 | 9.54 | 7.50 |

Values are medians across rounds. The Windows 4 KiB static runs ranged from 21,053 to 45,117 requests/s. Native Windows used buffered file delivery. WSL2 used ext4 and Linux sendfile; disabling sendfile yielded 6,337 requests/s for static 256 KiB. All 1,024 idle connections remained usable on both systems. Frontend memory grew by 10.6 MiB of Windows working set and 7.5 MiB of Linux RSS. These measurements cover one host and exclude external networks and TLS.

## Windows TransmitFile experiment

Release build on the same Windows 11 Pro host, based on `3b8d34a`. The frontend ran on logical CPU 4 with one Tokio network worker and one filesystem worker. Native Windows load clients used logical CPUs 8, 10, 12 and 14. Every HTTP/1.1 keep-alive GET retrieved and verified a warm 256 KiB local file. For each concurrency, the same executable ran with `RILL_SENDFILE=0` and `1`; the middle round reversed mode order. Each of three rounds had two seconds of warm-up and five seconds of measurement.

| Concurrent clients | Buffered req/s | TransmitFile enabled req/s | Throughput change | Buffered p95 ms | Enabled p95 ms |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 6,112 | 8,723 | +42.7% | 0.180 | 0.121 |
| 2 | 7,203 | 8,255 | +14.6% | 0.296 | 0.336 |
| 64 | 7,030 | 7,945 | +13.0% | 9.920 | 11.619 |

Each value is the median of three rounds. At 64 clients, enabled p95 ranged from 8.165 to 27.752 ms. Enabled mode uses the direct-write listener and at most two TransmitFile slots; simultaneous requests above that cap use buffered reads. The measurements compare these complete modes. The executable SHA-256 was `b2e85f448f3567ea63c8fa7aa9857879fd7a31f48121de0bfd8a593c83de06e4`. [Individual rounds](windows-transmitfile.csv)
