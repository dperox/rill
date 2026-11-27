# Rill file-serving measurements

These historical measurements used warm files on the WSL2 Linux filesystem, 64 HTTP/1.1 keep-alive clients and one frontend CPU. They describe the tested releases and host. Run the [current benchmark](README.md) for your deployment.

## Immutable asset cache

| Workload | Cache off (req/s) | Cache on (req/s) | Cache-on p95 | Cache-on peak RSS |
| --- | ---: | ---: | ---: | ---: |
| 4 KiB, one file | 32,654 | 45,283 | 1.603 ms | 6.78 MiB |
| 256 KiB, one file | 17,481 | 21,016 | 3.480 ms | 6.91 MiB |
| 256 KiB, 64 rotating files | 16,776 | 22,069 | 3.296 ms | 7.13 MiB |

Rill used 128 cache entries with one blocking worker. Medians cover three three-second samples after two-second warmups.

## Linux sendfile

| File | Buffered (req/s) | Sendfile (req/s) | Sendfile p95 |
| --- | ---: | ---: | ---: |
| 256 KiB | 6,944 | 16,245 | 6.444 ms |
| 1 MiB | 2,320 | 14,180 | 4.471 ms |

These medians cover four rounds per mode. Historical idle-connection measurements with the asset cache off held 4,096 connections at 31.92 MiB RSS, with all 8,192 responses verified across the five-second idle interval.

## TCP send buffers and corking

| Socket setting | 256 KiB file (req/s) | 1 MiB file (req/s) |
| --- | ---: | ---: |
| Linux default | 20,371 | 14,168 |
| 128 KiB effective send buffer | 12,862 | 5,327 |
| 2 MiB effective send buffer | 20,299 | 13,963 |
| 8 MiB effective send buffer | 20,399 | 14,069 |
| Default buffer with TCP corking | 18,629 | 10,234 |

The default socket started at 2,626,560 bytes. Each setting used three rotated measurements, with a one-second warmup and three seconds of timed requests. A larger send buffer gave little change at either size, while corking slowed both workloads. [Rill samples, latency, RSS and verified counts](socket-tuning.csv)
