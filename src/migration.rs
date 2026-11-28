//! Read-only CLI diagnostics and conservative translation of NGINX HTTP configuration.
mod dump;
use dump::{Directive, Dump};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read},
    path::Path,
};

const MAX_EXPAND_DEPTH: usize = 64;
const MAX_DIAGNOSTICS: usize = 10_000;
const MAX_LABEL_BYTES: usize = 256;

fn bound_label(value: &str) -> String {
    if value.len() <= MAX_LABEL_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_LABEL_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

#[derive(Debug, Serialize)]
pub struct Problem {
    code: &'static str,
    message: String,
    file: Option<String>,
    line: Option<usize>,
}
impl Problem {
    fn syntax(file: &str, line: usize) -> Self {
        Self {
            code: "invalid_syntax",
            message: "Malformed or excessively nested configuration dump".into(),
            file: Some(file.into()),
            line: Some(line),
        }
    }
}
fn finish(command: &str, result: &str, code: i32, details: Value) -> io::Result<()> {
    let mut output = details;
    output["schema_version"] = json!(1);
    output["command"] = json!(command);
    output["status"] = json!(result);
    serde_json::to_writer_pretty(io::stdout().lock(), &output)?;
    println!();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
pub fn validation(result: io::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) => finish("config test", "valid", 0, json!({"diagnostics":[]})),
        Err(e) => finish(
            "config test",
            "invalid",
            1,
            json!({"diagnostics":[{"code":"invalid_configuration","message":e.to_string()}]}),
        ),
    }
}
pub fn usage() -> io::Result<()> {
    finish(
        "cli",
        "error",
        1,
        json!({"diagnostics":[{"code":"invalid_arguments","message":"Use japka --help for syntax"}]}),
    )
}
pub fn run(path: &str) -> io::Result<()> {
    let result = (|| {
        let reader: Box<dyn Read> = if path == "-" {
            Box::new(io::stdin())
        } else {
            Box::new(std::fs::File::open(path)?)
        };
        let mut text = String::new();
        reader
            .take(16 * 1024 * 1024 + 1)
            .read_to_string(&mut text)?;
        if text.len() > 16 * 1024 * 1024 {
            return Err(io::Error::other("dump exceeds 16 MiB"));
        }
        Ok(text)
    })();
    let text = match result {
        Ok(t) => t,
        Err(e) => {
            return finish(
                "migrate nginx",
                "error",
                1,
                json!({"diagnostics":[{"code":"input_error","message":e.to_string()}]}),
            );
        }
    };
    let dump = match Dump::read(&text, if path == "-" { "<stdin>" } else { path }) {
        Ok(d) => d,
        Err(e) => return finish("migrate nginx", "error", 1, json!({"diagnostics":[e]})),
    };
    let mut report = Report::default();
    let mut seen = BTreeSet::from([dump.root.clone()]);
    let mut budget = 100_000usize;
    let mut nodes = dump.files[&dump.root].clone();
    if let Err(e) = expand(
        &mut nodes,
        &dump,
        &mut vec![dump.root.clone()],
        &mut seen,
        &mut budget,
        0,
        &mut report,
    ) {
        return finish("migrate nginx", "error", 1, json!({"diagnostics":[e]}));
    }
    for name in dump.files.keys().filter(|n| !seen.contains(*n)) {
        report.review = true;
        report.diagnostics.push(json!({"status":"unsupported","code":"unreferenced_section","file":name,"line":1,"context":"main","message":"Section was not reached through a supported include"}));
    }
    if nodes.is_empty() {
        return finish(
            "migrate nginx",
            "error",
            1,
            json!({"diagnostics":[{"code":"empty_configuration","message":"No directives found"}]}),
        );
    }
    for n in &nodes {
        if block(n, "http", 0) {
            report.ok(n, "main");
            report.http(&n.children);
        } else {
            report.unsupported(n, "main");
        }
    }
    let status = if report.review {
        "review_required"
    } else {
        "translated"
    };
    finish(
        "migrate nginx",
        status,
        if report.review { 2 } else { 0 },
        json!({"candidate":{"config":{"routes":report.routes,"sites":report.sites},"environment":report.env},"diagnostics":report.diagnostics}),
    )
}

fn expand(
    nodes: &mut Vec<Directive>,
    dump: &Dump,
    stack: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    budget: &mut usize,
    depth: usize,
    report: &mut Report,
) -> Result<(), Problem> {
    if depth > MAX_EXPAND_DEPTH {
        let (file, line) = nodes.first().map_or(("", 1), |n| (n.file.as_str(), n.line));
        return Err(Problem::syntax(file, line));
    }
    let src = std::mem::take(nodes);
    for mut n in src {
        if *budget == 0 || stack.len() > 40 {
            return Err(Problem::syntax(&n.file, n.line));
        }
        *budget -= 1;
        if n.words[0] == "include" && !n.block && n.words.len() == 2 {
            let files = dump.included(&n.words[1]);
            if files.is_empty() {
                nodes.push(n);
                continue;
            }
            report.ok(&n, "include");
            for file in files {
                if stack.contains(file) {
                    return Err(Problem::syntax(&n.file, n.line));
                }
                seen.insert(file.clone());
                stack.push(file.clone());
                let mut child = dump.files[file].clone();
                expand(&mut child, dump, stack, seen, budget, depth, report)?;
                nodes.extend(child);
                stack.pop();
            }
        } else {
            expand(
                &mut n.children,
                dump,
                stack,
                seen,
                budget,
                depth + 1,
                report,
            )?;
            nodes.push(n);
        }
    }
    Ok(())
}

fn block(n: &Directive, name: &str, args: usize) -> bool {
    n.block && n.words[0] == name && n.words.len() == args + 1
}
fn literal(n: &Directive) -> Option<&str> {
    if !n.block && n.words.len() == 2 && !n.words[1].is_empty() && !n.words[1].contains(['$', '\\'])
    {
        Some(&n.words[1])
    } else {
        None
    }
}
fn dns(s: &str) -> bool {
    s.len() <= 253
        && s.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}
fn endpoint(s: &str) -> bool {
    !s.contains(['@', '/', '$', '\\', '%'])
        && s.parse::<axum::http::uri::Authority>().is_ok_and(|a| {
            (dns(a.host())
                || a.host().parse::<std::net::Ipv4Addr>().is_ok()
                || a.host()
                    .trim_matches(['[', ']'])
                    .parse::<std::net::Ipv6Addr>()
                    .is_ok())
                && (a.as_str() == a.host() || a.port_u16().is_some())
        })
}

#[derive(Default)]
struct Report {
    review: bool,
    truncated: bool,
    diagnostics: Vec<Value>,
    env: BTreeMap<String, String>,
    sites: Vec<Value>,
    routes: Vec<Value>,
    seen_sites: BTreeSet<String>,
    seen_routes: BTreeSet<(String, String)>,
    groups: BTreeMap<String, Vec<String>>,
}
impl Report {
    fn note(&mut self, n: &Directive, ctx: &str, status: &str, code: &str, msg: &str) {
        self.review |= status != "translated";
        if self.diagnostics.len() >= MAX_DIAGNOSTICS {
            if !self.truncated {
                self.truncated = true;
                self.review = true;
                self.diagnostics.push(json!({
                    "file": n.file,
                    "line": n.line,
                    "context": bound_label(ctx),
                    "status": "unsupported",
                    "code": "diagnostics_truncated",
                    "message": "Diagnostic output limit reached; additional directives omitted"
                }));
            }
            return;
        }
        self.diagnostics.push(json!({
            "file": n.file,
            "line": n.line,
            "context": bound_label(ctx),
            "directive": bound_label(&n.words[0]),
            "status": status,
            "code": code,
            "message": msg
        }));
    }
    fn ok(&mut self, n: &Directive, ctx: &str) {
        self.note(
            n,
            ctx,
            "translated",
            "mapped",
            "Represented in candidate or expanded structurally",
        );
    }
    fn unsupported(&mut self, n: &Directive, ctx: &str) {
        let (code, msg) = match n.words[0].as_str() {
            "proxy_pass"
                if literal(n).is_some_and(|v| {
                    v.split_once("://")
                        .is_some_and(|(_, rest)| rest.contains('/'))
                }) =>
            {
                (
                    "proxy_uri_replacement",
                    "Japka preserves the request path. Review NGINX prefix replacement, including a trailing slash",
                )
            }
            "include" => (
                "unresolved_include",
                "Supply nginx -T output containing the included files",
            ),
            "auth_request" | "auth_basic" | "auth_basic_user_file" => (
                "authentication_required",
                "Preserve authentication before cutover",
            ),
            "proxy_set_header" | "real_ip_header" | "set_real_ip_from" => (
                "header_policy",
                "Review upstream headers and trusted client identity",
            ),
            "try_files" | "alias" | "rewrite" => (
                "uri_mapping",
                "Reproduce file fallback or path rewriting before cutover",
            ),
            "client_max_body_size" => ("body_limit", "Preserve the request body limit"),
            _ => (
                "unsupported_directive",
                "No mapping for this directive or its options in this context",
            ),
        };
        self.note(n, ctx, "unsupported", code, msg);
        if !n.children.is_empty() {
            let child_ctx = bound_label(&format!("{ctx}/{}", bound_label(&n.words[0])));
            for child in &n.children {
                self.unsupported(child, &child_ctx);
            }
        }
    }
    fn http(&mut self, nodes: &[Directive]) {
        for n in nodes.iter().filter(|n| block(n, "upstream", 1)) {
            let peers: Option<Vec<_>> = n
                .children
                .iter()
                .map(|p| {
                    literal(p)
                        .filter(|s| p.words[0] == "server" && endpoint(s))
                        .map(str::to_owned)
                })
                .collect();
            let entry = self.groups.entry(n.words[1].clone()).or_default();
            if let Some(peers) = peers
                && !peers.is_empty()
                && entry.is_empty()
            {
                *entry = peers;
                self.ok(n, "http");
                for p in &n.children {
                    self.ok(p, "http/upstream");
                }
            } else {
                entry.clear();
                self.unsupported(n, "http");
            }
        }
        for n in nodes.iter().filter(|n| !block(n, "upstream", 1)) {
            if block(n, "server", 0) {
                self.server(n);
            } else {
                self.unsupported(n, "http");
            }
        }
    }
    fn server(&mut self, n: &Directive) {
        let mut seen_hosts = BTreeSet::new();
        let hosts: Vec<_> = n
            .children
            .iter()
            .filter(|d| d.words[0] == "server_name" && !d.block)
            .flat_map(|d| d.words[1..].iter())
            .filter(|h| dns(h))
            .map(|h| h.to_ascii_lowercase())
            .filter(|h| seen_hosts.insert(h.clone()))
            .collect();
        let ctx = bound_label(&format!("http/server[{}]", hosts.join(",")));
        self.note(n,&ctx,"behavior_differs","default_server","Review listener defaults, unmatched hosts, inherited policies and NGINX implicit defaults before deployment");
        for d in &n.children {
            match d.words[0].as_str() {
                "server_name" if !hosts.is_empty() && d.words[1..].iter().all(|h| dns(h)) => {
                    self.ok(d, &ctx)
                }
                "listen" if literal(d).is_some() => {
                    let value = &d.words[1];
                    let addr = if let Ok(p) = value.parse::<u16>() {
                        format!("0.0.0.0:{p}")
                    } else {
                        value.clone()
                    };
                    if addr.parse::<std::net::SocketAddr>().is_ok()
                        && self.env.get("LISTEN").is_none_or(|v| v == &addr)
                    {
                        self.env.insert("LISTEN".into(), addr);
                        self.ok(d, &ctx);
                    } else {
                        self.unsupported(d, &ctx);
                    }
                }
                "root"
                    if !hosts.is_empty()
                        && literal(d).is_some_and(|v| Path::new(v).is_absolute()) =>
                {
                    let mut duplicate = false;
                    for h in &hosts {
                        if self.seen_sites.insert(h.clone()) {
                            self.sites.push(json!({"host":h,"root":d.words[1]}));
                        } else {
                            duplicate = true;
                        }
                    }
                    if duplicate {
                        self.note(
                            d,
                            &ctx,
                            "unsupported",
                            "duplicate_definition",
                            "Duplicate site host omitted from candidate; reconcile conflicting server blocks",
                        );
                    } else {
                        self.ok(d, &ctx);
                    }
                }
                "location"
                    if block(d, "location", 1)
                        && !hosts.is_empty()
                        && d.words[1].starts_with('/')
                        && d.words[1].ends_with('/')
                        && !d.words[1].contains(['$', '\\', '?', '#']) =>
                {
                    let loc = bound_label(&format!("{ctx}/location[{}]", d.words[1]));
                    self.note(
                        d,
                        &loc,
                        "behavior_differs",
                        "location_matching",
                        "Verify normalized paths, slash redirects and location precedence",
                    );
                    for action in &d.children {
                        if action.words[0] == "proxy_pass"
                            && let Some(url) = literal(action)
                            && let Some((scheme, authority)) = url.split_once("://")
                            && matches!(scheme, "http" | "https")
                            && !authority.contains(['@', '/', '$', '\\', '%'])
                        {
                            let peers = match self.groups.get(authority) {
                                Some(peers) if peers.is_empty() => {
                                    self.unsupported(action, &loc);
                                    continue;
                                }
                                Some(peers) => peers.clone(),
                                None if endpoint(authority) => vec![authority.to_owned()],
                                None => {
                                    self.unsupported(action, &loc);
                                    continue;
                                }
                            };
                            let upstreams: Vec<_> =
                                peers.iter().map(|v| format!("{scheme}://{v}")).collect();
                            let mut duplicate = false;
                            for h in &hosts {
                                if self.seen_routes.insert((h.clone(), d.words[1].clone())) {
                                    self.routes.push(
                                        json!({"host":h,"prefix":d.words[1],"upstreams":upstreams}),
                                    );
                                } else {
                                    duplicate = true;
                                }
                            }
                            if duplicate {
                                self.note(
                                    action,
                                    &loc,
                                    "unsupported",
                                    "duplicate_definition",
                                    "Duplicate virtual host and prefix omitted from candidate; reconcile conflicting locations",
                                );
                            } else {
                                self.note(action,&loc,"behavior_differs","proxy_defaults","Path preserved; compare upstream Host, TLS verification, retry, buffering and timeout behavior");
                            }
                        } else {
                            self.unsupported(action, &loc);
                        }
                    }
                }
                _ => self.unsupported(d, &ctx),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn included_normalizes_parent_dir_components() {
        let text = "\
# configuration file /etc/nginx/conf.d/main.conf:
http { include ../sites-enabled/*.conf; }
# configuration file /etc/nginx/sites-enabled/app.conf:
server { server_name app.example; }
";
        let dump = Dump::read(text, "nginx.conf").unwrap();
        let included = dump.included("../sites-enabled/*.conf");
        assert_eq!(included, vec!["/etc/nginx/sites-enabled/app.conf"]);
    }

    #[test]
    fn underscore_upstream_and_duplicate_site_route_handling() {
        let text = "\
http {
  upstream api_v1 { server 127.0.0.1:9001; }
  server {
    server_name app.example APP.EXAMPLE;
    root /var/www/one;
    location /api/ { proxy_pass http://api_v1; }
  }
  server {
    server_name app.example;
    root /var/www/two;
    location /api/ { proxy_pass http://api_v1; }
  }
}
";
        let dump = Dump::read(text, "nginx.conf").unwrap();
        let mut report = Report::default();
        let nodes = &dump.files[&dump.root];
        report.http(&nodes[0].children);
        assert_eq!(report.sites.len(), 1);
        assert_eq!(report.routes.len(), 1);
        assert_eq!(
            report.routes[0]["upstreams"],
            json!(["http://127.0.0.1:9001"])
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d["code"] == "duplicate_definition")
        );
    }

    #[test]
    fn deeply_nested_includes_hit_global_depth_limit() {
        let mut text = String::new();
        text.push_str("# configuration file /etc/nginx/a.conf:\n");
        for _ in 0..40 {
            text.push_str("a { ");
        }
        text.push_str("include b.conf;");
        for _ in 0..40 {
            text.push_str(" }");
        }
        text.push_str("\n# configuration file /etc/nginx/b.conf:\n");
        for _ in 0..40 {
            text.push_str("b { ");
        }
        text.push_str("x 1;");
        for _ in 0..40 {
            text.push_str(" }");
        }
        let dump = Dump::read(&text, "a.conf").unwrap();
        let mut report = Report::default();
        let mut seen = BTreeSet::from([dump.root.clone()]);
        let mut budget = 100_000usize;
        let mut nodes = dump.files[&dump.root].clone();
        let err = expand(
            &mut nodes,
            &dump,
            &mut vec![dump.root.clone()],
            &mut seen,
            &mut budget,
            0,
            &mut report,
        );
        assert!(err.is_err());
    }
}
