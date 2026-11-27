# HTTP policies

These environment settings apply to the HTTP listeners. All settings below default to disabled. TCP and UDP stream listeners use their own transport settings. [Stream listeners](streams.md)

| Setting | Behavior |
| --- | --- |
| `ALLOW_IPS` | Comma-separated IPv4 or IPv6 addresses and CIDR networks. A nonempty list requires a matching peer address. |
| `DENY_IPS` | Comma-separated addresses and networks. A matching entry returns HTTP 403 even if the allow list also matches. |
| `RATE_LIMIT` | Maximum requests per second per peer IP, in process-local fixed one-second windows. A zero value disables the limit. Excess requests receive HTTP 429 with `Retry-After: 1`. |
| `COMPRESS` | Set to `1` to enable negotiated gzip or `0` to disable it. |
| `CACHE_COALESCE` | Set to `1` with a positive `CACHE_TTL` to combine simultaneous eligible misses for the same cache key. |
| `CACHE_TTL` | Maximum time in seconds Rill will keep eligible upstream responses. A zero value disables caching. |

Access and rate policies use the connected peer IP. IPv4-mapped IPv6 addresses are normalized to IPv4. Forwarded address headers have no effect. The access check runs before rate limiting, and static requests, proxy requests and cache hits share the same per-peer limit. The limiter tracks up to 16,384 distinct addresses per window and returns HTTP 429 to further untracked addresses until the next window.

Gzip uses Tower HTTP negotiation and predicates. It skips HEAD, partial content, existing content encodings, responses marked `Cache-Control: no-transform`, and responses with integrity digests. Rill weakens strong ETags when it applies gzip. Responses eligible for compression include `Vary: Accept-Encoding`. On Linux, setting `COMPRESS=1` selects buffered static delivery across the plaintext listener, including requests for identity encoding. The sendfile path and immutable asset cache require compression to be disabled. [Static file settings](static-io.md)

## Proxy cache

Only bodyless GET requests with an ordinary set of request headers can use the cache. The allowlist is `Host`, `Accept`, `Accept-Encoding`, `Accept-Language`, and `User-Agent`. Authorization, cookies, conditional headers, ranges, forwarding headers, and application-specific headers bypass caching. The key includes path and query, URI authority where present, transport security, and the values of the allowed request headers.

An upstream response opts in with `Cache-Control: public, max-age=N` or `Cache-Control: public, s-maxage=N`. When both ages are present the shared age takes precedence. Extra cache-control directives, `Set-Cookie`, `Vary`, pre-encoded responses, and partial responses bypass storage. Only HTTP 200 responses with a known body length between one byte and 64 KiB are eligible. The implementation caps the request key and response metadata at 8 KiB each. Responses carrying `WWW-Authenticate` or `Trailer` also bypass storage. Rill accounts for upstream `Age` and `Date`, and serves cached hits with `Age`. Keep this disabled for upstreams that emit publicly cacheable headers on responses that vary by an unkeyed input.

The cache retains up to 256 entries, evicting the oldest when full, and buffers copies of up to 16 eligible response streams concurrently. A fill stores the copy when the fixed-length upstream body finishes. Compression runs after cache lookup, so the stored representation is the upstream identity body. An unsafe request such as POST, PUT or DELETE that returns a 2xx or 3xx status invalidates cached variants of the same path and query. Entries and expiry are process-local.

When coalescing is enabled, up to 16 distinct cache keys can have an active fill. Followers wait at most two seconds and check the cache again. If the response is private, the leader disconnects, the entry is invalidated, or the wait expires without a valid cached response, each follower can fetch independently. Only a complete, publicly cacheable response is shared. A request for a distinct key proceeds independently.

## Verify inside WSL

    cargo build --locked
    python3 checks/policy.py

The socket checks cover cache hits, expiry, request and response bypasses, write invalidation, gzip and range behavior, per-peer rate limits, and IP rules.
