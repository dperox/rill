# Static file settings

Set `DOCUMENT_ROOT` to an existing directory. The source build defaults to
`public/` relative to the current working directory. The Docker image uses
`/app/public`. On macOS, static responses use buffered reads. The sendfile path and immutable asset cache below are Linux-specific.

## Linux sendfile

`RILL_SENDFILE` accepts `0` or `1` and defaults to `1` on Linux.
Ordinary plaintext HTTP/1 GETs larger than 64 KiB use the open file descriptor
to transfer directly to the socket when compression is disabled. HEAD and
matching validators skip reads of the body. Small files through 64 KiB use
batched buffered reads and can share the response within a request group.

Use `RILL_SENDFILE=0` when cold or remote storage could stall a network
worker that also serves latency-sensitive requests. A sendfile call can wait
for source pages even with a nonblocking socket. TLS, HTTP/2, HTTP/3, byte
ranges, encoded paths and `COMPRESS=1` use buffered file delivery.
[Rill sendfile behavior and benchmark](../bench/sendfile.md)

For uncached files, atomic replacement updates the path for subsequent requests. A transfer
that has already opened the file continues to read the original inode.
Truncating an actively transferring file can close that connection before the
response completes. Publish replacements by renaming a complete file into
place.

## Immutable assets

Set `RILL_ASSET_CACHE_ENTRIES=128` to retain up to 128 versioned files across requests. The default is 0 (disabled) and the maximum is 4096. Use this only when a changed asset gets a new filename. A name such as `app.8f31.js` contains a dot-delimited hexadecimal fingerprint of 4–64 digits immediately before the extension. Rill recognizes the filename convention without checking a content digest. A query string such as `app.js?v=8f31` does not version the filename.

A successful ordinary GET can populate the cache on the Linux plaintext HTTP/1 listener with sendfile enabled and compression disabled. Subsequent eligible GET and HEAD requests reuse the response headers and evaluate request validators against the cached file. Files up to 64 KiB retain their bytes. Larger files retain an open descriptor for sendfile. The limit applies independently to the default root and each [configured site](routing.md#per-host-static-sites). At 128 entries, each cache can hold up to 8 MiB of inline file data, plus metadata. Each retained large asset uses one open file descriptor. Active downloads may hold descriptors or buffers after eviction.

FIFO eviction and process restart discard entries. A successful configuration reload also replaces every named site cache, even if its root stays the same. The default root keeps its cache across reloads. Replacing or deleting a versioned path can leave its old contents available to cache hits. Ranges, encoded paths, and other transports use the ordinary filesystem lookup. Publish each changed asset at a new path before updating references in mutable pages such as `index.html`.

## Buffered downloads

`RILL_STATIC_CHUNK_KIB` controls buffered file streams and accepts integers
from 4 to 256. It defaults to 64 KiB. The small-file threshold and sendfile
transfer size are independent of this setting. Larger buffered chunks can
improve throughput and increase memory held by clients with downloads in
flight. Compare settings at representative file sizes and client concurrency.

`RILL_BLOCKING_THREADS` sets the pool limit for filesystem tasks and DNS
lookups. The default is one blocking worker, regardless of the number of
async network workers. Set a positive integer from 1 to 65535. A larger pool can absorb more
concurrent slow operations and increase memory. The [benchmark guide](../bench/README.md)
shows how to measure throughput and idle connections.
