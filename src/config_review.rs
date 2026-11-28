//! Discovery and offline review of the same deployment inputs consumed by preflight.
mod catalog;

use crate::{Mode, reload::FileConfig, routing};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    path::Path,
};

pub(crate) fn path_string(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::other("Review output requires UTF-8 configuration paths"))
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, bytes);
    hash.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn file_schema() -> schemars::Schema {
    schemars::generate::SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<FileConfig>()
}

fn print(value: &impl Serialize) -> io::Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)
}

pub(crate) fn schema() -> io::Result<()> {
    print(&file_schema())
}

pub(crate) fn list() -> io::Result<()> {
    print(&json!({
        "schema_version": 1, "command": "config list", "environment": catalog::settings(),
        "file": {"change_requires": file_action(), "schema": file_schema()}
    }))
}

fn file_action() -> &'static str {
    if cfg!(unix) { "reload" } else { "restart" }
}

#[derive(Default, Serialize)]
pub(crate) struct Snapshot {
    pub(crate) file_sha256: Option<String>,
    pub(crate) proxy: BTreeMap<&'static str, Value>,
    pub(crate) routes_source: &'static str,
    pub(crate) tls_source: &'static str,
    pub(crate) routes: Vec<routing::Definition>,
    pub(crate) sites: BTreeMap<String, String>,
    tls: BTreeMap<String, Value>,
}

impl Snapshot {
    pub(crate) fn certificate(
        &mut self,
        host: Option<&str>,
        cert: &Path,
        key: &Path,
    ) -> io::Result<()> {
        self.tls.insert(
            host.unwrap_or("*").to_ascii_lowercase(),
            json!({
                "cert": path_string(&std::path::absolute(cert)?)?, "key": path_string(&std::path::absolute(key)?)?,
            }),
        );
        Ok(())
    }

    fn load(path: Option<&Path>) -> io::Result<Self> {
        let mut snapshot = Self::default();
        crate::runtime(Mode::Inspect {
            path,
            snapshot: &mut snapshot,
        })?;
        Ok(snapshot)
    }

    fn routes_by_host_and_prefix(&self) -> BTreeMap<(&str, &str), Value> {
        self.routes
            .iter()
            .map(|route| {
                (
                    (route.host.as_deref().unwrap_or("*"), route.prefix.as_str()),
                    json!(route),
                )
            })
            .collect()
    }
}

fn report(command: &str, result: io::Result<Value>) -> io::Result<()> {
    match result {
        Ok(mut value) => {
            value["schema_version"] = json!(1);
            value["command"] = json!(command);
            print(&value)
        }
        Err(error) => {
            print(
                &json!({"schema_version": 1, "command": command, "status": "invalid",
                "diagnostics": [crate::settings::diagnostic(&error)]}),
            )?;
            std::process::exit(1)
        }
    }
}

pub(crate) fn show() -> io::Result<()> {
    report(
        "config show",
        Snapshot::load(None).map(|s| json!({"status":"valid","effective":s})),
    )
}

#[derive(Serialize)]
struct Change {
    key: Value,
    change: &'static str,
    before: Value,
    after: Value,
}

#[derive(Default, Serialize)]
struct Changes {
    unchanged: usize,
    changes: Vec<Change>,
}

impl Changes {
    fn describe(&self, label: &str, review: &mut Vec<String>) {
        for item in &self.changes {
            let key = &item.key;
            let name = if label == "route" {
                format!("host {} prefix {}", key["host"], key["prefix"])
            } else {
                key.to_string()
            };
            let describe_value = |value: &Value| {
                if label == "route" {
                    format!(
                        "upstreams {} (failover {})",
                        value["upstreams"], value["failover"]
                    )
                } else if label == "proxy setting" {
                    format!(
                        "{} ({})",
                        value["value"],
                        value["source"].as_str().expect("setting source")
                    )
                } else {
                    value.to_string()
                }
            };
            review.push(match item.change {
                "added" => format!("Add {label} {name}: {}", describe_value(&item.after)),
                "removed" => format!("Remove {label} {name}: {}", describe_value(&item.before)),
                _ => format!(
                    "Change {label} {name}: {} -> {}",
                    describe_value(&item.before),
                    describe_value(&item.after)
                ),
            });
        }
        review.push(format!("Unchanged {label} entries: {}", self.unchanged));
    }
}

fn changes<K: Ord, V: PartialEq + Serialize>(
    before: &BTreeMap<K, V>,
    after: &BTreeMap<K, V>,
    key_fields: impl Fn(&K) -> Value,
) -> Changes {
    let keys: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    let mut result = Changes::default();
    for key in keys {
        let old = before.get(key);
        let new = after.get(key);
        if old == new {
            result.unchanged += 1;
            continue;
        }
        let kind = match (old, new) {
            (None, _) => "added",
            (_, None) => "removed",
            _ => "modified",
        };
        result.changes.push(Change {
            key: key_fields(key),
            change: kind,
            before: json!(old),
            after: json!(new),
        });
    }
    result
}

pub(crate) fn environment_digest() -> String {
    let mut inputs = Vec::new();
    for (name, ..) in catalog::ENVIRONMENT {
        // Both files are passed explicitly. Each is resolved in the same startup environment.
        if *name != "JAPKA_CONFIG"
            && let Some(value) = crate::settings::var_os(name)
        {
            inputs.extend_from_slice(name.as_bytes());
            inputs.push(0);
            inputs.extend_from_slice(value.as_encoded_bytes());
            inputs.push(0);
        }
    }
    digest(&inputs)
}

pub(crate) fn plan(base: &str, candidate: &str) -> io::Result<()> {
    report(
        "config plan",
        (|| {
            let load = |label, path: &str| {
                Snapshot::load(Some(Path::new(path)))
                    .map_err(|e| crate::settings::context(e, label))
            };
            let before = load("baseline", base)?;
            let after = load("candidate", candidate)?;
            let routes = changes(
                &before.routes_by_host_and_prefix(),
                &after.routes_by_host_and_prefix(),
                |(host, prefix)| json!({"host":host,"prefix":prefix}),
            );
            let proxy = changes(&before.proxy, &after.proxy, |field| json!(field));
            let sites = changes(&before.sites, &after.sites, |host| json!(host));
            let tls = changes(&before.tls, &after.tls, |host| json!(host));
            let unchanged = before.file_sha256 == after.file_sha256
                && before.sites == after.sites
                && before.tls == after.tls;
            let action = if unchanged { "none" } else { file_action() };
            let mut review = vec![format!("Deployment action: {action}")];
            if before.routes_source != after.routes_source {
                review.push(format!(
                    "Route source: {} -> {}",
                    before.routes_source, after.routes_source
                ));
            }
            if before.tls_source != after.tls_source {
                review.push(format!(
                    "TLS source: {} -> {}",
                    before.tls_source, after.tls_source
                ));
            }
            if !proxy.changes.is_empty() {
                proxy.describe("proxy setting", &mut review);
            }
            routes.describe("route", &mut review);
            sites.describe("site", &mut review);
            tls.describe("certificate", &mut review);
            Ok(json!({
                "status": "valid", "version": env!("CARGO_PKG_VERSION"), "platform": std::env::consts::OS,
                "target": path_string(&std::path::absolute(base)?)?, "working_directory": path_string(&std::env::current_dir()?)?,
                "environment_sha256": environment_digest(),
                "baseline_sha256": before.file_sha256, "candidate_sha256": after.file_sha256,
                "action": action, "review": review,
                "sources": {"routes":{"before":before.routes_source,"after":after.routes_source},
                            "tls":{"before":before.tls_source,"after":after.tls_source}},
                "proxy": proxy, "routes": routes, "sites": sites, "tls": tls
            }))
        })(),
    )
}
