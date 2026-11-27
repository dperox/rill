"""Verify security, protocol validation, CLI preflight and graceful shutdown regressions."""
from contextlib import ExitStack
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import signal
import socket
import ssl
import subprocess
from tempfile import TemporaryDirectory
from threading import Thread
import time

from reload import LOOPBACK, RILL, free_port, listening, publish, stop, wait_until


class EchoHeaders(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    calls = 0

    def do_GET(self):
        self.server.calls += 1
        body = json.dumps({
            "calls": self.server.calls,
            "path": self.path,
            "x_hop": self.headers.get("X-Hop"),
            "content_length": self.headers.get("Content-Length"),
            "authorization": self.headers.get("Authorization"),
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "public, max-age=60")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        data = self.rfile.read(length) if length else b""
        body = json.dumps({
            "len": len(data),
            "content_length": self.headers.get("Content-Length"),
            "x_hop": self.headers.get("X-Hop"),
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


def raw_exchange(port, payload):
    with socket.create_connection((LOOPBACK, port), timeout=5) as sock:
        sock.sendall(payload)
        sock.shutdown(socket.SHUT_WR)
        chunks = []
        while chunk := sock.recv(4096):
            chunks.append(chunk)
        return b"".join(chunks)


def status_of(raw):
    first = raw.split(b"\r\n", 1)[0]
    return int(first.split()[1])


def clean_env(**extra):
    scrub = (
        "LISTEN", "PORT", "TLS_LISTEN", "TLS_CERT", "TLS_KEY",
        "RILL_CONFIG", "RILL_ROUTES", "UPSTREAMS", "DOCUMENT_ROOT",
        "ALLOW_IPS", "DENY_IPS", "RATE_LIMIT", "CACHE_TTL", "CACHE_COALESCE",
        "COMPRESS", "RILL_ACCESS_LOG", "RILL_SENDFILE", "RILL_STATIC_LISTEN",
        "RILL_HTTPS_REDIRECT", "RILL_UPSTREAM_CA", "RILL_UPSTREAM_CERT",
        "RILL_UPSTREAM_KEY", "RILL_TCP_LISTENERS", "RILL_UDP_LISTENERS",
    )
    env = {k: v for k, v in os.environ.items() if k not in scrub}
    env.update(extra)
    return env


def check_cli_and_preflight(work):
    ver = subprocess.run([RILL, "--version"], capture_output=True, text=True, timeout=5)
    assert ver.returncode == 0 and ver.stdout.startswith("rill "), (ver.returncode, ver.stdout)
    for help_args in (["--help"], ["-h"], ["migrate", "nginx", "--help"], ["config", "test", "--help"]):
        res = subprocess.run([RILL, *help_args], capture_output=True, text=True, timeout=5)
        assert res.returncode == 0 and "Usage:" in res.stdout, (help_args, res.returncode, res.stdout)

    # Invalid routes[].host (port or wildcard) must be rejected by config test.
    for bad_host in ("docs.example:8080", "*.example.com", ""):
        env = clean_env(
            DOCUMENT_ROOT=str(work),
            RILL_ROUTES=json.dumps([{"host": bad_host, "prefix": "/api/", "upstreams": ["127.0.0.1:9001"]}]),
        )
        res = subprocess.run([RILL, "config", "test"], capture_output=True, text=True, env=env, timeout=5)
        assert res.returncode == 1, (bad_host, res.returncode, res.stdout)

    # Duplicate listener address must be rejected by config test.
    port = free_port()
    cert, key = work / "c.pem", work / "c.key"
    subprocess.run(
        ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
         "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost",
         "-keyout", str(key), "-out", str(cert)],
        check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )
    env = clean_env(
        DOCUMENT_ROOT=str(work),
        LISTEN=f"{LOOPBACK}:{port}",
        TLS_LISTEN=f"{LOOPBACK}:{port}",
        TLS_CERT=str(cert),
        TLS_KEY=str(key),
    )
    res = subprocess.run([RILL, "config", "test"], capture_output=True, text=True, env=env, timeout=5)
    assert res.returncode == 1, (res.returncode, res.stdout)

    # Invalid RILL_UPSTREAM_CA must fail config test even without https:// routes.
    env = clean_env(
        DOCUMENT_ROOT=str(work),
        RILL_UPSTREAM_CA=str(work / "nonexistent.pem"),
    )
    res = subprocess.run([RILL, "config", "test"], capture_output=True, text=True, env=env, timeout=5)
    assert res.returncode == 1, (res.returncode, res.stdout)


def check_static_and_protocol(work, backend):
    root = work / "site"
    root.mkdir()
    outside = work / "outside.txt"
    outside.write_text("TOP SECRET")
    (root / "ok.txt").write_text("public ok")
    (root / "escape.txt").symlink_to(outside)
    (root / "inside.txt").symlink_to(root / "ok.txt")
    fifo = root / "named.pipe"
    os.mkfifo(fifo)

    port = free_port()
    log = work / "static.log"
    env = clean_env(
        LISTEN=f"{LOOPBACK}:{port}",
        DOCUMENT_ROOT=str(root),
        UPSTREAMS=f"{LOOPBACK}:{backend.server_port}",
        RILL_SENDFILE="1",
    )
    with log.open("wb") as out:
        proc = subprocess.Popen([RILL], env=env, stdout=out, stderr=subprocess.STDOUT)
    try:
        wait_until(proc, log, "static security server", lambda: listening(port))

        # Symlink inside root works; symlink escaping root and FIFO return 404 without hanging.
        for path, headers, expected in [
            ("/ok.txt", {}, 200),
            ("/inside.txt", {}, 200),
            ("/escape.txt", {}, 404),
            ("/%65scape.txt", {}, 404),
            ("/escape.txt", {"Range": "bytes=0-3"}, 404),
            ("/named.pipe", {}, 404),
            ("/named.pipe", {"Range": "bytes=0-3"}, 404),
        ]:
            conn = HTTPConnection(LOOPBACK, port, timeout=3)
            try:
                conn.request("GET", path, headers=headers)
                resp = conn.getresponse()
                body = resp.read()
                assert resp.status == expected, (path, headers, resp.status, body)
                assert b"TOP SECRET" not in body
            finally:
                conn.close()

        # Duplicate Host, userinfo in Host, and proxy dot-dot traversal must be rejected with 400.
        dup_host = raw_exchange(port, b"GET /ok.txt HTTP/1.1\r\nHost: a.test\r\nHost: b.test\r\nConnection: close\r\n\r\n")
        assert status_of(dup_host) == 400, dup_host
        userinfo_host = raw_exchange(port, b"GET /ok.txt HTTP/1.1\r\nHost: user@a.test\r\nConnection: close\r\n\r\n")
        assert status_of(userinfo_host) == 400, userinfo_host
        traversal = raw_exchange(port, b"GET /api/sub/../secret HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        assert status_of(traversal) == 400, traversal
        escape_via_prefix = raw_exchange(port, b"GET /api/../escape.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        assert status_of(escape_via_prefix) == 404, escape_via_prefix

        # Connection: content-length must NOT strip Content-Length while still stripping X-Hop.
        raw_post = raw_exchange(
            port,
            b"POST /api/check HTTP/1.1\r\n"
            b"Host: localhost\r\n"
            b"Connection: content-length, x-hop, close\r\n"
            b"X-Hop: strip-me\r\n"
            b"Content-Length: 5\r\n\r\n"
            b"hello",
        )
        assert status_of(raw_post) == 200, raw_post
        parsed = json.loads(raw_post.split(b"\r\n\r\n", 1)[1])
        assert parsed == {"len": 5, "content_length": "5", "x_hop": None}, parsed

        # SIGTERM must exit cleanly with status 0.
        proc.send_signal(signal.SIGTERM)
        assert proc.wait(timeout=5) == 0, proc.returncode
    finally:
        if proc.poll() is None:
            stop(proc)


def check_ipv4_mapped_acl(work):
    root = work / "acl"
    root.mkdir()
    (root / "index.html").write_text("ok")

    port = free_port()
    log = work / "deny.log"
    env = clean_env(LISTEN=f"{LOOPBACK}:{port}", DOCUMENT_ROOT=str(root), DENY_IPS="::ffff:127.0.0.1")
    with log.open("wb") as out:
        proc = subprocess.Popen([RILL], env=env, stdout=out, stderr=subprocess.STDOUT)
    try:
        wait_until(proc, log, "deny server", lambda: listening(port))
        conn = HTTPConnection(LOOPBACK, port, timeout=3)
        conn.request("GET", "/")
        assert conn.getresponse().status == 403
        conn.close()
    finally:
        stop(proc)

    port = free_port()
    log = work / "allow.log"
    env = clean_env(LISTEN=f"{LOOPBACK}:{port}", DOCUMENT_ROOT=str(root), ALLOW_IPS="::ffff:127.0.0.0/104")
    with log.open("wb") as out:
        proc = subprocess.Popen([RILL], env=env, stdout=out, stderr=subprocess.STDOUT)
    try:
        wait_until(proc, log, "allow server", lambda: listening(port))
        conn = HTTPConnection(LOOPBACK, port, timeout=3)
        conn.request("GET", "/")
        assert conn.getresponse().status == 200
        conn.close()
    finally:
        stop(proc)


def check_sni_and_redirect_cache(work, backend):
    tls_dir = work / "tls"
    tls_dir.mkdir()
    for name, dns in (("default", "default.test"), ("site", "site.test")):
        subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
             "-subj", f"/CN={dns}", "-addext", f"subjectAltName=DNS:{dns}",
             "-keyout", str(tls_dir / f"{name}.key"), "-out", str(tls_dir / f"{name}.pem")],
            check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
    plain, secure = free_port(), free_port()
    while secure == plain:
        secure = free_port()
    cfg = tls_dir / "rill.json"
    publish(cfg, {
        "tls": {
            "cert": "default.pem",
            "key": "default.key",
            "hosts": [{"host": "site.test", "cert": "site.pem", "key": "site.key"}],
        },
        "routes": [{"prefix": "/api/", "upstreams": [f"{LOOPBACK}:{backend.server_port}"]}],
    })
    log = tls_dir / "rill.log"
    env = clean_env(
        LISTEN=f"{LOOPBACK}:{plain}",
        TLS_LISTEN=f"{LOOPBACK}:{secure}",
        RILL_CONFIG=str(cfg),
        RILL_HTTPS_REDIRECT=f"site.test:{secure}",
        CACHE_TTL="60",
        DOCUMENT_ROOT=str(tls_dir),
    )
    trust = ssl.create_default_context()
    trust.load_verify_locations(str(tls_dir / "default.pem"))
    trust.load_verify_locations(str(tls_dir / "site.pem"))

    with log.open("wb") as out:
        proc = subprocess.Popen([RILL], env=env, stdout=out, stderr=subprocess.STDOUT)
    try:
        wait_until(proc, log, "tls server", lambda: listening(plain) and listening(secure))

        def tls_get(sni, host, path):
            conn = HTTPConnection(LOOPBACK, secure, timeout=3)
            try:
                conn.connect()
                conn.sock = trust.wrap_socket(conn.sock, server_hostname=sni)
                conn.request("GET", path, headers={"Host": host})
                resp = conn.getresponse()
                return resp.status, resp.read()
            finally:
                conn.close()

        # Domain fronting: SNI=default.test requesting named host site.test must return 421.
        assert tls_get("default.test", "site.test", "/api/cached")[0] == 421

        # Populate HTTPS cache for /api/cached.
        status, first = tls_get("site.test", "site.test", "/api/cached")
        assert status == 200
        status, second = tls_get("site.test", "site.test", "/api/cached")
        assert status == 200 and second == first

        # Plaintext POST receives 308 redirect and must NOT invalidate HTTPS cache.
        conn = HTTPConnection(LOOPBACK, plain, timeout=3)
        conn.request("POST", "/api/cached", headers={"Host": "site.test"})
        assert conn.getresponse().status == 308
        conn.close()

        status, third = tls_get("site.test", "site.test", "/api/cached")
        assert status == 200 and third == first, (first, third)
    finally:
        stop(proc)


def main():
    with TemporaryDirectory() as tmp, ExitStack() as stack:
        work = Path(tmp)
        backend = stack.enter_context(ThreadingHTTPServer((LOOPBACK, 0), EchoHeaders))
        backend.calls = 0
        Thread(target=backend.serve_forever, daemon=True).start()
        stack.callback(backend.shutdown)

        check_cli_and_preflight(work)
        check_static_and_protocol(work, backend)
        check_ipv4_mapped_acl(work)
        check_sni_and_redirect_cache(work, backend)
    print("Security, protocol validation, IPv4-mapped ACL, SNI, redirect cache and SIGTERM checks passed.")


if __name__ == "__main__":
    main()
