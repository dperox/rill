# Rill's HTTP/1 transport patches

Rill pins [its Hyper fork](https://github.com/dperox/hyper/tree/832307636942a64bb5e47fa61cd17906383d9aa3) to a full Git revision in the root Cargo manifest. The six source changes apply to upstream Hyper 1.13.0. [Review the patch against upstream](https://github.com/dperox/hyper/compare/55247767429fb29753f724b467869aa6203db532...832307636942a64bb5e47fa61cd17906383d9aa3).

HTTP/1 server connections
start with a 1 KiB adaptive socket read and 512 bytes for encoding response headers.
Large reads still grow geometrically up to the existing limit. The response header
vector grows on demand. HTTP/1 clients keep their upstream 8 KiB defaults.
An explicit maximum buffer configuration still uses upstream's 8 KiB read start.

The local server-only `hyper::ext::Http1DirectBody` response extension lets a caller
write a fixed-length plaintext body with an OS transfer such as Linux sendfile.
It is implemented in `src/ext/direct_body.rs` and the HTTP/1 dispatcher, connection,
encoder and buffered I/O modules. The caller supplies the exact connection and a
one-shot future that reports how many bytes it wrote. The dispatcher checks the
Content-Length, flushes pending headers even with pipelined requests buffered,
and polls the transfer under normal backpressure. The connection resumes keep-alive
only after the reported count matches the encoder. Errors and short writes end
the connection. HTTP/2 ignores the extension. The caller selects a regular body
for TLS, compression and other transports that need to transform the data.

When upgrading, apply the patches to the new upstream release in the fork. Run
`cargo test -p hyper --lib --features full` there. Update Hyper’s exact version
and Git revision in Rill’s Cargo.toml, then update Cargo.lock and check
`cargo tree -i hyper --locked` for a single resolved Hyper. Review the upstream
license against `notices/HYPER-LICENSE`. Run Rill’s HTTP compatibility,
sendfile and idle-connection checks before merging the upgrade.
