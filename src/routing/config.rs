//! Resolve HTTP pool settings once for each candidate generation.
use crate::{env_utf8, settings::field_error, upstream::Health};
use axum::http::uri::PathAndQuery;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io};

#[derive(Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Balance {
    #[default]
    RoundRobin,
    LeastConnections,
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProxyConfig {
    /// HTTP balancing for every route. Omitted or null inherits JAPKA_HTTP_BALANCE.
    balance: Option<Balance>,
    /// Seconds between probes. Zero disables probes. Omitted or null inherits JAPKA_HEALTH_INTERVAL_SECS.
    #[schemars(range(max = 4294967295_u64))]
    health_interval_secs: Option<u32>,
    /// HTTP probe path starting with /. Omission inherits JAPKA_HEALTH_PATH. Explicit null selects TCP.
    #[serde(default, deserialize_with = "present_path")]
    #[schemars(!default, with = "Option<String>", regex(pattern = "^/"))]
    health_path: Option<Option<String>>,
}

// Serde normally collapses omission and JSON null into the same Option.
fn present_path<'de, D: Deserializer<'de>>(input: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(input).map(Some)
}

#[derive(Serialize)]
struct Setting<T> {
    value: T,
    source: &'static str,
}

fn resolve<T: Default>(
    file: Option<T>,
    variable: &'static str,
    parse: impl FnOnce(String) -> io::Result<T>,
) -> io::Result<Setting<T>> {
    let (value, source) = if let Some(value) = file {
        (value, "file")
    } else if let Some(raw) = env_utf8(variable)? {
        (parse(raw)?, "environment")
    } else {
        (T::default(), "default")
    };
    Ok(Setting { value, source })
}

pub(crate) struct PoolSettings {
    balance: Setting<Balance>,
    interval: Setting<u32>,
    path: Setting<Option<PathAndQuery>>,
}

impl ProxyConfig {
    pub(crate) fn resolve(self) -> io::Result<PoolSettings> {
        let balance = resolve(self.balance, "JAPKA_HTTP_BALANCE", |s| match s.as_str() {
            "round_robin" => Ok(Balance::RoundRobin),
            "least_connections" => Ok(Balance::LeastConnections),
            _ => Err(io::Error::other(
                "JAPKA_HTTP_BALANCE must be round_robin or least_connections",
            )),
        })?;
        let interval = resolve(
            self.health_interval_secs,
            "JAPKA_HEALTH_INTERVAL_SECS",
            |s| {
                s.parse().map_err(|_| {
                    io::Error::other("JAPKA_HEALTH_INTERVAL_SECS must be an unsigned integer")
                })
            },
        )?;
        let path = resolve(self.health_path, "JAPKA_HEALTH_PATH", |s| Ok(Some(s)))?;
        let field = if path.source == "file" {
            "proxy.health_path"
        } else {
            "JAPKA_HEALTH_PATH"
        };
        let parsed = path
            .value
            .as_deref()
            .map(|s| {
                if !s.starts_with('/') {
                    return Err(field_error(field.into(), "health path must start with /"));
                }
                s.parse::<PathAndQuery>()
                    .map_err(|e| field_error(field.into(), e))
            })
            .transpose()?;
        if parsed.is_some() && interval.value == 0 {
            let field = if interval.source == "file" && path.source != "file" {
                "proxy.health_interval_secs"
            } else {
                field
            };
            return Err(field_error(
                field.into(),
                "an HTTP health path requires a positive probe interval; set proxy.health_path to null to clear an inherited path",
            ));
        }
        Ok(PoolSettings {
            balance,
            interval,
            path: Setting {
                value: parsed,
                source: path.source,
            },
        })
    }
}

impl PoolSettings {
    pub(crate) fn least_connections(&self) -> bool {
        self.balance.value == Balance::LeastConnections
    }

    pub(crate) fn health(&self) -> Option<Health> {
        Health::new(self.interval.value, self.path.value.clone())
    }

    pub(crate) fn for_review(&self) -> BTreeMap<&'static str, Value> {
        BTreeMap::from([
            ("balance", json!(self.balance)),
            ("health_interval_secs", json!(self.interval)),
            (
                "health_path",
                json!(Setting {
                    value: self.path.value.as_ref().map(PathAndQuery::as_str),
                    source: self.path.source
                }),
            ),
        ])
    }
}
