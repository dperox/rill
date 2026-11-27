# Native Windows and WSL2 baseline

Rill release `7082e92`, AMD Ryzen 9 9950X3D, Windows 11 Pro build 26300 and Ubuntu WSL2. The same HTTP/1.1 client source was compiled for each OS. Both ends ran on local loopback. The frontend used one logical CPU, the optional Rill backend a separate core, and the 64 keep-alive clients four other cores. Each of three rounds used two seconds of warm-up and five seconds of measurement, checking status, length and every body. Files were warm, with access logging, gzip and application caches disabled.

| Workload | Windows requests/s | WSL2 requests/s | Windows p95 ms | WSL2 p95 ms |
| --- | ---: | ---: | ---: | ---: |
| Static 4 KiB | 21,068 | 25,957 | 3.29 | 2.95 |
| Proxy 4 KiB | 37,953 | 22,038 | 1.95 | 3.24 |
| Static 256 KiB | 7,082 | 13,889 | 9.93 | 5.72 |
| Proxy 256 KiB | 7,559 | 9,806 | 9.54 | 7.50 |

Values are medians across rounds. The Windows 4 KiB static runs ranged from 21,053 to 45,117 requests/s. Native Windows used buffered file delivery. WSL2 used ext4 and Linux sendfile; disabling sendfile yielded 6,337 requests/s for static 256 KiB. All 1,024 idle connections remained usable on both systems. Frontend memory grew by 10.6 MiB of Windows working set and 7.5 MiB of Linux RSS. These measurements cover one host and exclude external networks and TLS.
