# Static file settings

Set `DOCUMENT_ROOT` to an existing directory. The native binary defaults to
`public/` relative to the current working directory. The Docker image uses
`/app/public`. Direct file transfer (`JAPKA_SENDFILE`) defaults to `1` on Linux
(via `sendfile`) and Windows (via `TransmitFile`), and `0` on macOS (buffered
reads). The immutable asset cache below is Linux-specific.

Static serving verifies that resolved paths remain inside the configured root
directory (symlinks escaping the site root and special files such as FIFOs
return 404). Dotfiles (such as `.env` or `.git`) inside the root are not
filtered, so point `DOCUMENT_ROOT` and `sites[].root` at isolated static
artifacts rather than a source repository or secrets directory.

## Linux sendfile

Ordinary plaintext HTTP/1 GETs larger than 64 KiB use the open file descriptor
to transfer directly to the socket when compression is disabled. HEAD and
matching validators skip reads of the body. Small files through 64 KiB use
batched buffered reads and can share the response within a request group.

The same plaintext port accepts static, proxy, and WebSocket requests.
It supports HTTP/1 and prior-knowledge HTTP/2. Filesystems that do not
support sendfile use buffered reads.

## Static-only listener

Set `JAPKA_STATIC_LISTEN` (cross-platform) to add a dedicated plaintext listener
that serves static files only. Requests matching a configured proxy prefix
return HTTP 404. The listener accepts HTTP/1 and prior-knowledge HTTP/2
and applies HTTP access rules, rate limits, and `JAPKA_HTTPS_REDIRECT`.

Use `JAPKA_SENDFILE=0` when cold or remote storage could stall a network
worker that also serves latency-sensitive requests. A sendfile call can wait
for source pages even with a nonblocking socket. TLS, HTTP/2, HTTP/3, byte
ranges, encoded paths and `COMPRESS=1` use buffered file delivery.

For uncached files, atomic replacement updates the path for subsequent requests. A transfer
that has already opened the file continues to read the original inode.
Truncating an actively transferring file can close that connection before the
response completes. Publish replacements by renaming a complete file into
place.

## Windows TransmitFile

Native Windows uses Winsock TransmitFile by default for eligible plaintext HTTP/1 GET responses larger than 64 KiB. Set `JAPKA_SENDFILE=0` before startup to use buffered delivery. The dispatcher flushes the response headers before starting the transfer and waits for the advertised byte count before processing the next response on that connection.

Japka reserves at most two direct transfers per process. Additional downloads immediately take the buffered path. Two dedicated completion workers keep slow transfers off the Tokio network and filesystem pools. A reset or short transfer ends that response. HEAD, byte ranges, TLS, and gzip keep their existing handling. The immutable asset cache is Linux-only.

## Immutable assets

Set `JAPKA_ASSET_CACHE_ENTRIES=128` to retain up to 128 versioned files across requests. The default is 0 (disabled) and the maximum is 4096. Use this only when a changed asset gets a new filename. A name such as `app.8f31.js` contains a dot-delimited hexadecimal fingerprint of 4–64 digits immediately before the extension. Japka recognizes the filename convention without checking a content digest. A query string such as `app.js?v=8f31` does not version the filename.

A successful ordinary GET can populate the cache on the Linux plaintext HTTP/1 listener with sendfile enabled and compression disabled. Subsequent eligible GET and HEAD requests reuse the response headers and evaluate request validators against the cached file. Files up to 64 KiB retain their bytes. Larger files retain an open descriptor for sendfile. The limit applies independently to the default root and each [configured site](routing.md#per-host-static-sites). At 128 entries, each cache can hold up to 8 MiB of inline file data, plus metadata. Each retained large asset uses one open file descriptor. Active downloads may hold descriptors or buffers after eviction.

FIFO eviction and process restart discard entries. A successful configuration reload also replaces every named site cache, even if its root stays the same. The default root keeps its cache across reloads. Replacing or deleting a versioned path can leave its old contents available to cache hits. Ranges, encoded paths, and other transports use the ordinary filesystem lookup. Publish each changed asset at a new path before updating references in mutable pages such as `index.html`.

## Buffered downloads

`JAPKA_STATIC_CHUNK_KIB` controls buffered file streams and accepts integers
from 4 to 256. It defaults to 64 KiB. The small-file threshold and sendfile
transfer size are independent of this setting. Larger buffered chunks can
improve throughput and increase memory held by clients with downloads in
flight. Compare settings at representative file sizes and client concurrency.

`JAPKA_BLOCKING_THREADS` sets the process-wide limit on blocking workers for
filesystem tasks and DNS lookups. It accepts a positive integer from 1 to
65535. The default is one on the general runtime. The Linux plaintext
scheduler selects up to four blocking workers automatically for the CPU
allocations and workloads below. A larger pool can handle more concurrent slow
operations and use more memory.

## Linux HTTP worker scheduling

When Japka runs a single plaintext direct-file listener, each network worker can
accept from the same listening socket and drive I/O for its own connections.
A client stays on that worker for the lifetime of its connection. Routes,
health checks and application caches are shared across workers.

Japka selects worker counts at startup. For a single Linux plaintext listener
with direct file delivery enabled, the defaults depend on the CPU allocation
and workload.

| CPU allocation and workload | Network workers | Maximum blocking workers |
|---|---:|---:|
| Two CPUs of quota shared across a wider set of CPUs | 1 | 2 |
| Pinned to two CPUs, serving files or proxying requests | 2 | 2 |
| Three or four CPUs, serving files without upstream routes or a reloadable config file | One per available CPU | One per network worker |
| Four CPUs, with upstream routes or a reloadable config file | 4 | 1 |

Japka reads the available parallelism and, for the two-CPU decision, the Linux
thread affinity mask. If it cannot read that mask, it uses two network workers.
Other automatic configurations use the available CPU count and one blocking
worker. TLS, compression, disabled direct file delivery and additional
listeners use the general runtime settings.

Set `TOKIO_WORKER_THREADS` or `JAPKA_BLOCKING_THREADS` independently to
override either count. With two or more network workers, the independent
I/O workers need a blocking budget of at least one per network worker.
Larger proxies and file-configured servers with more than two workers use
the shared scheduler, which lets a reload add upstream routes. Japka reports
both worker counts in its startup log.
