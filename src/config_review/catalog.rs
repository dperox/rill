//! Startup environment exposed by this binary. All values are passed as strings.
use serde_json::{Value, json};

pub(super) const ENVIRONMENT: &[(&str, &str, &str, &str)] = &[
    (
        "ALLOW_IPS",
        "comma-separated IPs or CIDRs",
        "",
        "Allow requests only from matching peers when set.",
    ),
    (
        "CACHE_COALESCE",
        "0 or 1",
        "0",
        "Combine concurrent cache misses. Enabling requires positive CACHE_TTL.",
    ),
    (
        "CACHE_TTL",
        "u64 seconds",
        "0",
        "Maximum proxy response cache lifetime. Zero disables caching.",
    ),
    (
        "COMPRESS",
        "0 or 1",
        "0",
        "Enable negotiated HTTP gzip compression.",
    ),
    (
        "DENY_IPS",
        "comma-separated IPs or CIDRs",
        "",
        "Reject requests from matching peers before checking the allow list.",
    ),
    (
        "DOCUMENT_ROOT",
        "directory path",
        "public",
        "Serve unmatched hosts from this directory, relative to the working directory.",
    ),
    (
        "JAPKA_ACCESS_LOG",
        "0 or 1",
        "0",
        "Write JSON HTTP access records to stderr.",
    ),
    (
        "JAPKA_ASSET_CACHE_ENTRIES",
        "integer 0..4096",
        "0",
        "Maximum immutable asset entries per static site on the Linux direct file path.",
    ),
    (
        "JAPKA_BLOCKING_THREADS",
        "integer 1..65535",
        "1 or scheduler-selected on Linux",
        "Limit filesystem and DNS blocking workers.",
    ),
    (
        "JAPKA_CONFIG",
        "JSON file path",
        "unset",
        "Read sites, routes, proxy settings and TLS references from this file. Unix reloads reread the same path.",
    ),
    (
        "JAPKA_FIXED_UPSTREAM",
        "0 or 1",
        "0",
        "Use the fixed single-peer plaintext GET optimization.",
    ),
    (
        "JAPKA_HEALTH_INTERVAL_SECS",
        "u32 seconds",
        "0",
        "Interval between backend health checks. Zero disables checks.",
    ),
    (
        "JAPKA_HEALTH_PATH",
        "HTTP path starting with /",
        "unset (TCP probe)",
        "Use HTTP checks with a positive JAPKA_HEALTH_INTERVAL_SECS.",
    ),
    (
        "JAPKA_HTTPS_REDIRECT",
        "public host with optional port",
        "unset",
        "Redirect plaintext HTTP to this HTTPS authority.",
    ),
    (
        "JAPKA_HTTP_BALANCE",
        "round_robin or least_connections",
        "round_robin",
        "Select peers in HTTP upstream pools.",
    ),
    (
        "JAPKA_ROUTES",
        "JSON array matching the routes field",
        "[]",
        "Inherited HTTP routes when the file omits routes or sets it to null.",
    ),
    (
        "JAPKA_SENDFILE",
        "0 or 1",
        "1 on Linux and Windows, 0 elsewhere",
        "Enable eligible direct plaintext file transfers.",
    ),
    (
        "JAPKA_STATIC_CHUNK_KIB",
        "integer 4..256",
        "64",
        "Buffered static response chunk size in KiB.",
    ),
    (
        "JAPKA_STATIC_LISTEN",
        "host:port",
        "unset",
        "Add a dedicated plaintext static listener.",
    ),
    (
        "JAPKA_STREAM_BALANCE",
        "round_robin or least_connections",
        "round_robin",
        "Select peers for TCP connections and UDP sessions.",
    ),
    (
        "JAPKA_TCP_LISTENERS",
        "BIND=PEER[,PEER];... (numeric socket addresses)",
        "",
        "Configure TCP forwarding listeners.",
    ),
    (
        "JAPKA_UDP_LISTENERS",
        "BIND=PEER[,PEER];... (numeric socket addresses)",
        "",
        "Configure UDP forwarding listeners.",
    ),
    (
        "JAPKA_UPSTREAM_CA",
        "PEM file path",
        "system trust roots",
        "Trust these roots for HTTPS upstreams.",
    ),
    (
        "JAPKA_UPSTREAM_CERT",
        "PEM chain file path",
        "unset",
        "Present this client certificate with JAPKA_UPSTREAM_KEY.",
    ),
    (
        "JAPKA_UPSTREAM_KEY",
        "PEM private key file path",
        "unset",
        "Client key matching JAPKA_UPSTREAM_CERT.",
    ),
    (
        "LISTEN",
        "host:port",
        "127.0.0.1:$PORT",
        "Bind the primary plaintext HTTP listener.",
    ),
    (
        "PORT",
        "TCP port",
        "8080",
        "Port for the loopback listener when LISTEN is unset.",
    ),
    (
        "RATE_LIMIT",
        "u64 requests per second",
        "0",
        "Limit requests per peer. Zero disables the limit.",
    ),
    (
        "TLS_CERT",
        "PEM chain file path",
        "unset",
        "Default frontend certificate when the file omits tls. Requires TLS_KEY.",
    ),
    (
        "TLS_KEY",
        "PEM private key file path",
        "unset",
        "Default frontend key matching TLS_CERT.",
    ),
    (
        "TLS_LISTEN",
        "host:port",
        "unset",
        "Bind TLS over TCP and QUIC over UDP. Requires frontend credentials.",
    ),
    (
        "TOKIO_WORKER_THREADS",
        "positive integer",
        "available CPU count with Linux scheduler adjustment",
        "Set network worker threads.",
    ),
    (
        "UPSTREAMS",
        "comma-separated HTTP or HTTPS endpoints",
        "",
        "Append the default /api/ route to inherited routes.",
    ),
];

pub(super) fn settings() -> Vec<Value> {
    ENVIRONMENT
        .iter()
        .map(|(name, accepts, default, description)| {
            let mut entry = json!({
                "name": name, "accepts": accepts, "default": default, "description": description,
                "alias": name.strip_prefix("JAPKA_").map(|suffix| format!("RILL_{suffix}")),
                "change_requires": "restart",
            });
            if let Some(field) = match *name {
                "JAPKA_HTTP_BALANCE" => Some("proxy.balance"),
                "JAPKA_HEALTH_INTERVAL_SECS" => Some("proxy.health_interval_secs"),
                "JAPKA_HEALTH_PATH" => Some("proxy.health_path"),
                _ => None,
            } {
                entry["file_field"] = json!(field);
            }
            entry
        })
        .collect()
}
