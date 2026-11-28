"""Exercise settings discovery, route review and approved deployment on loopback."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread

from routing import JAPKA
from reload import free_port, make_certificate

ROOT = Path(__file__).resolve().parents[1]
BINARY = str(Path(JAPKA).resolve())
APPLY = ROOT / "deploy/apply-config.py"


def cli(env, *args, status=0):
    result = subprocess.run([BINARY, "config", *args], env=env, capture_output=True, timeout=15)
    assert result.returncode == status, (result.stdout, result.stderr)
    return json.loads(result.stdout)


def get(port, path):
    conn = HTTPConnection("127.0.0.1", port, timeout=2)
    try:
        conn.request("GET", path)
        response = conn.getresponse()
        return response.status, response.read()
    finally:
        conn.close()


class Origin(BaseHTTPRequestHandler):
    def do_GET(self):
        data = b"backend response"
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def check_proxy_settings(work, env, catalog):
    expected_fields = {"JAPKA_HTTP_BALANCE": "proxy.balance",
                       "JAPKA_HEALTH_INTERVAL_SECS": "proxy.health_interval_secs",
                       "JAPKA_HEALTH_PATH": "proxy.health_path"}
    assert {entry["name"]: entry["file_field"] for entry in catalog["environment"]
            if entry["name"] in expected_fields} == expected_fields
    base = work / "inherited.json"
    next_file = work / "proxy-candidate.json"
    base.write_text("{}")
    settings = dict(env, JAPKA_CONFIG=str(base), RILL_HTTP_BALANCE="least_connections",
                    RILL_HEALTH_INTERVAL_SECS="7", RILL_HEALTH_PATH="/ready")
    inherited = cli(settings, "show")["effective"]["proxy"]
    assert inherited == {k: {"value": v, "source": "environment"} for k, v in
                         {"balance": "least_connections", "health_interval_secs": 7, "health_path": "/ready"}.items()}
    next_file.write_text('{"proxy":{"balance":"round_robin","health_path":null}}')
    review = cli(settings, "plan", base, next_file)
    changes = {c["key"]: c for c in review["proxy"]["changes"]}
    assert changes["health_path"]["after"] == {"source": "file", "value": None}
    assert changes["balance"]["before"] == inherited["balance"]
    assert changes["balance"]["after"] == {"source": "file", "value": "round_robin"}
    assert review["proxy"]["unchanged"] == 1
    assert any('least_connections' in line and 'round_robin' in line for line in review["review"])
    assert review["action"] == ("reload" if os.name == "posix" else "restart")
    # A file setting can explicitly disable probes even if its inherited values are invalid.
    next_file.write_text('{"proxy":{"health_interval_secs":0,"health_path":null,"balance":"round_robin"}}')
    overridden = dict(settings, JAPKA_CONFIG=str(next_file), JAPKA_HEALTH_INTERVAL_SECS="bad",
                      JAPKA_HEALTH_PATH="bad", JAPKA_HTTP_BALANCE="bad")
    disabled = cli(overridden, "show")["effective"]["proxy"]
    assert disabled["health_interval_secs"] == {"value": 0, "source": "file"}
    # File and environment values are validated together after resolving each field independently.
    for candidate, path in [({"health_interval_secs":0}, "health_interval_secs"),
                            ({"health_interval_secs":-1}, "health_interval_secs"),
                            ({"health_path":"ready"}, "health_path"),
                            ({"balance":"random"}, "balance")]:
        next_file.write_text(json.dumps({"proxy": candidate}))
        error = cli(settings, "plan", base, next_file, status=1)["diagnostics"][0]
        assert error["path"] == "proxy." + path, error
    next_file.write_text('{"proxy":{"balance":"least_connections"}}')
    changed = cli(settings, "plan", base, next_file)["proxy"]["changes"]
    assert len(changed) == 1 and changed[0]["before"]["source"] == "environment" and changed[0]["after"]["source"] == "file"


def check(work, backend):
    base = work / "active.json"
    next_file = work / "candidate.json"
    plan_file = work / "reviewed.json"
    port = free_port()
    inherited = {"host": "LOCALHOST", "prefix": "/old/", "upstreams": [backend]}
    env = {k: v for k, v in os.environ.items() if not k.startswith(("JAPKA_", "RILL_"))}
    # Ignore the caller's other Japka settings to give the fixture its own native deployment.
    catalog = cli(env, "list", "--json")
    names = {entry["name"] for entry in catalog["environment"]}
    env = {k: v for k, v in env.items() if k not in names}
    (work / "public").mkdir()
    env.update(DOCUMENT_ROOT=str(work / "public"), JAPKA_CONFIG=str(base), RILL_ROUTES=json.dumps([inherited]),
               LISTEN=f"127.0.0.1:{port}", TOKIO_WORKER_THREADS="1")
    schema = cli(env, "schema")
    assert catalog["file"]["schema"] == schema and schema["additionalProperties"] is False
    assert set(schema["properties"]) == {"routes", "sites", "tls", "proxy"}
    check_proxy_settings(work, env, catalog)
    source = "\n".join(p.read_text() for p in (ROOT / "src").rglob("*.rs") if "config_review" not in p.parts)
    referenced = set(re.findall(r'"(JAPKA_[A-Z0-9_]+)"', source))
    assert referenced == {name for name in names if name.startswith("JAPKA_")}
    env_reads = re.findall(r'(?:env::var(?:_os)?|env_utf8|variable|number|networks)\("([A-Z][A-Z0-9_]+)"\)', source)
    assert set(env_reads) | referenced == names
    assert all(s["accepts"] and s["description"] and s["change_requires"] == "restart"
               for s in catalog["environment"])
    base.write_text('{"routes": null}')
    shown = cli(env, "show")["effective"]
    assert shown["routes_source"] == "environment" and shown["routes"][0]["host"] == "localhost"
    # Agents can seed an explicit list directly from the effective route definitions.
    explicit = shown["routes"]
    added = {"prefix": "/api/", "upstreams": [backend]}
    next_file.write_text(json.dumps({"routes": [added, *explicit]}))
    review = cli(env, "plan", base, next_file, "--json")
    assert any(line.startswith('Add route host "*" prefix "/api/"') for line in review["review"])
    assert "Unchanged route entries: 1" in review["review"]
    assert review["routes"]["unchanged"] == 1
    assert [(c["change"], c["key"]["prefix"]) for c in review["routes"]["changes"]] == [("added", "/api/")]
    assert review["sources"]["routes"] == {"before":"environment","after":"file"}
    plan_file.write_text(json.dumps(review))
    other = work / "other.json"
    other.write_text('{"routes":[]}')
    removal = cli(env, "plan", base, other)["routes"]["changes"]
    assert len(removal) == 1 and removal[0]["key"]["prefix"] == "/old/" and removal[0]["change"] == "removed"
    other.write_text('{"routes":[{"prefix":"/api/","upstreams":[17]}]}')
    error = cli(env, "plan", base, other, status=1)["diagnostics"][0]
    assert error["path"] == "routes[0].upstreams[0]" and "candidate" in error["message"]
    other.write_text('{}{}')
    assert cli(dict(env, JAPKA_CONFIG=str(other)), "test", status=1)["status"] == "invalid"
    other.write_text('{"routes":[{"prefix":"/x/","upstreams":[]}]}')
    assert cli(dict(env, JAPKA_CONFIG=str(other)), "test", status=1)["diagnostics"][0]["path"] == "routes[0].upstreams"
    reordered = [{"prefix": "/pool/", "upstreams": [backend, "localhost:12345"]}]
    prior = work / "prior.json"
    prior.write_text(json.dumps({"routes": reordered}))
    reordered[0]["upstreams"].reverse()
    other.write_text(json.dumps({"routes": reordered, "sites": [{"host": "TEST.LOCAL", "root": "public"}]}))
    changed = cli(env, "plan", prior, other)
    assert changed["routes"]["changes"][0]["change"] == "modified"
    site = changed["sites"]["changes"][0]
    assert site["key"] == "test.local" and Path(site["after"]) == work / "public"

    if os.name != "posix":
        assert review["action"] == "restart"
        return

    make_certificate(work, "local")
    tls_env = dict(env, TLS_LISTEN="127.0.0.1:0", TLS_CERT=str(work / "local.pem"), TLS_KEY=str(work / "local.key"))
    other.write_text(json.dumps({"tls": {"cert": "local.pem", "key": "local.key", "hosts": [
        {"host": "localhost", "cert":"local.pem", "key":"local.key"}]}}))
    tls_plan = cli(tls_env, "plan", base, other)
    assert tls_plan["sources"]["tls"] == {"before": "environment", "after": "file"}
    assert tls_plan["tls"]["unchanged"] == 1 and tls_plan["tls"]["changes"][0]["key"] == "localhost"
    assert "PRIVATE KEY" not in json.dumps(tls_plan)

    logfile = work / "service.log"
    with logfile.open("wb") as log:
        proc = subprocess.Popen([BINARY], env=env, stdout=subprocess.DEVNULL, stderr=log)
    try:
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline and proc.poll() is None:
            try:
                if get(port, "/api/")[0] == 404:
                    break
            except OSError:
                pass
            time.sleep(.05)
        else:
            raise AssertionError(logfile.read_text())

        def apply(probe, success, approved=None):
            result = subprocess.run([sys.executable, APPLY, "--binary", BINARY, "--approved-plan", plan_file,
                                     "--approved-sha256", approved or hashlib.sha256(plan_file.read_bytes()).hexdigest(),
                                     "--candidate", next_file, "--pid", str(proc.pid), "--log", logfile,
                                     "--verify", sys.executable, __file__, "--probe", str(port), probe],
                                    env=env, capture_output=True, timeout=30)
            assert (result.returncode == 0) == success, (result.stdout, result.stderr)
            return result.stderr

        assert b"approved digest" in apply("/api/", False, "0" * 64)
        approved_bytes = next_file.read_bytes()
        next_file.write_bytes(approved_bytes + b" ")
        apply("/api/", False)
        assert get(port, "/api/")[0] == 404
        next_file.write_bytes(approved_bytes)
        base.write_bytes(b"{}")
        apply("/api/", False)
        assert base.read_bytes() == b"{}"
        base.write_bytes(b'{"routes": null}')
        assert b"Restored the reviewed baseline" in apply("/missing/", False)
        assert hashlib.sha256(base.read_bytes()).hexdigest() == review["baseline_sha256"]
        assert get(port, "/api/")[0] == 404
        # Simulate interruption after rename and before SIGHUP. Retrying recovers and applies.
        os.link(base, work / ".active.json.rollback")
        other.write_bytes(approved_bytes)
        os.replace(other, base)
        apply("/api/", True)
        apply("/api/", True)
        assert base.read_bytes() == approved_bytes and not (work / ".active.json.rollback").exists()
        assert get(port, "/api/test") == (200, b"backend response")
        client = HTTPConnection("127.0.0.1", port, timeout=2)
        client.request("GET", "/old/test", headers={"Host":"localhost"})
        assert client.getresponse().read() == b"backend response"
        client.close()
    finally:
        proc.terminate()
        proc.wait(timeout=8)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", nargs=2)
    args = parser.parse_args()
    if args.probe:
        port, path = args.probe
        sys.exit(0 if get(int(port), path) == (200, b"backend response") else 1)
    with tempfile.TemporaryDirectory() as directory, ThreadingHTTPServer(("127.0.0.1", 0), Origin) as origin:
        server = Thread(target=origin.serve_forever, daemon=True)
        server.start()
        try:
            check(Path(directory).resolve(), f"127.0.0.1:{origin.server_port}")
        finally:
            origin.shutdown()
            server.join()
    print("Configuration discovery, plan, approval, reload and rollback passed")
