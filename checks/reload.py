"""Check transactional SIGHUP reloads. Requires Unix signals and the openssl CLI."""
import argparse
import asyncio
import json
import os
import signal
import socket
import ssl
import subprocess
import time
from contextlib import ExitStack
from http.client import HTTPConnection, HTTPSConnection
from http.server import ThreadingHTTPServer
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Event, Thread

from routing import Origin, JAPKA
from sendfile import SLOW, read_exact_body, read_head, request, slow_socket

LOOPBACK = "127.0.0.1"


class KeepAliveOrigin(Origin):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if self.path == "/ready":
            self.send_response(self.server.ready_status)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if self.path != "/api/mutable":
            return super().do_GET()
        self.server.reads += 1
        body = f"{self.server.label}:{self.server.version}".encode()
        self.send_response(200)
        self.send_header("Cache-Control", "public, max-age=120")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        assert self.path == "/api/mutable"
        self.server.write_started.set()
        if not self.server.finish_write.wait(10):
            self.send_error(504)
            return
        self.server.version += 1
        self.send_response(204)
        self.end_headers()


def free_port():
    with socket.socket() as sock:
        sock.bind((LOOPBACK, 0))
        return sock.getsockname()[1]


def make_certificate(work, label):
    subprocess.run(
        ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
         "-subj", f"/CN=localhost/O={label}", "-addext", "subjectAltName=DNS:localhost",
         "-keyout", f"{label}.key", "-out", f"{label}.pem"],
        cwd=work, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )


def candidate(origin, certificate, key=None):
    return {
        "routes": [{"prefix": "/api/", "upstreams": [f"{LOOPBACK}:{origin.server_port}"]}],
        "tls": {"cert": f"{certificate}.pem", "key": f"{key or certificate}.key"},
    }


def publish(path, config):
    replacement = path.with_suffix(".tmp")
    replacement.write_text(json.dumps(config))
    replacement.replace(path)


def wait_until(proc, logfile, description, predicate):
    end = time.monotonic() + 10
    while proc.poll() is None and time.monotonic() < end:
        if predicate():
            return
        time.sleep(.02)
    raise AssertionError(f"{description} (exit status {proc.poll()})\n{logfile.read_text()}")


def reload(proc, logfile, expected):
    offset = logfile.stat().st_size
    proc.send_signal(signal.SIGHUP)
    wait_until(proc, logfile, expected, lambda: expected in logfile.read_bytes()[offset:])


def listening(port):
    try:
        with socket.create_connection((LOOPBACK, port), timeout=.1):
            return True
    except OSError:
        return False


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def get(client, path, label):
    client.request("GET", path)
    read_reply(client, path, label)


def read_reply(client, path, label):
    response = client.getresponse()
    body = response.read()
    assert (response.status, body) == (200, f"{label} {path}".encode()), (response.status, body)


def https(port, trust, expected, stack):
    client = HTTPSConnection("localhost", port, context=trust, timeout=10)
    stack.callback(client.close)
    client.connect()
    assert client.sock.getpeercert(binary_form=True) == expected, "unexpected certificate"
    return client


async def check_http3(port, ca, label):
    from aioquic.asyncio import connect
    from aioquic.quic.configuration import QuicConfiguration

    from edge import Client

    config = QuicConfiguration(is_client=True, alpn_protocols=["h3"], server_name="localhost")
    config.load_verify_locations(str(ca))
    async with asyncio.timeout(10):
        async with connect(LOOPBACK, port, configuration=config, create_protocol=Client) as client:
            headers, body = await client.request(port, "/api/quic")
            assert (headers[b":status"], body) == (b"200", f"{label} /api/quic".encode())


def check_generation(http_port, tls_port, trust, certificate, origin, stack, quic_ca):
    http = HTTPConnection(LOOPBACK, http_port, timeout=10)
    stack.callback(http.close)
    get(http, "/api/plain", origin)
    get(https(tls_port, trust, certificate, stack), "/api/secure", origin)
    if quic_ca is not None:
        asyncio.run(check_http3(tls_port, quic_ca, origin))


def check(work, origins, sendfile, http3):
    with ExitStack() as stack:
        old, new = origins
        old.started.clear()
        old.release.clear()
        config = work / "japka.json"
        log = work / "japka.log"
        publish(config, candidate(old, "old"))
        http_port = free_port()
        tls_port = free_port()
        while tls_port == http_port:
            tls_port = free_port()
        env = dict(os.environ, LISTEN=f"{LOOPBACK}:{http_port}",
                   TLS_LISTEN=f"{LOOPBACK}:{tls_port}", JAPKA_CONFIG=str(config),
                   DOCUMENT_ROOT=str(work / "public"), JAPKA_SENDFILE=sendfile,
                   COMPRESS="0", CACHE_TTL="120", TOKIO_WORKER_THREADS="1")
        with log.open("wb") as output:
            proc = subprocess.Popen([JAPKA], env=env, stdout=output, stderr=subprocess.STDOUT)
        stack.callback(stop, proc)
        wait_until(proc, log, "Japka listeners did not become ready",
                   lambda: listening(http_port) and listening(tls_port))

        trust = ssl.create_default_context()
        for label in ("old", "new"):
            trust.load_verify_locations(cafile=str(work / f"{label}.pem"))
        old_cert, new_cert = (
            ssl.PEM_cert_to_DER_cert((work / f"{label}.pem").read_text())
            for label in ("old", "new")
        )
        old_ca = work / "old.pem" if http3 else None
        new_ca = work / "new.pem" if http3 else None
        check_generation(http_port, tls_port, trust, old_cert, "old", stack, old_ca)

        # Warm the old route before switching to a different backend for this URL.
        client = HTTPConnection(LOOPBACK, http_port, timeout=10)
        stack.callback(client.close)

        def cached(label, version):
            client.request("GET", "/api/mutable")
            response = client.getresponse()
            assert (response.status, response.read()) == (200, f"{label}:{version}".encode())

        cached("old", old.version)
        slow = stack.enter_context(slow_socket(http_port))
        slow.sendall(request("GET", "/slow.txt"))
        reader = stack.enter_context(slow.makefile("rb"))
        slow_headers = read_head(reader)
        assert int(slow_headers[b"content-length"]) == len(SLOW)

        held = https(tls_port, trust, old_cert, stack)
        tls_socket = held.sock
        held.request("GET", "/api/hold")
        stack.callback(old.release.set)
        assert old.started.wait(3), "old upstream did not receive the held request"

        publish(config, candidate(new, "new"))
        reload(proc, log, b"Japka configuration reloaded")
        check_generation(http_port, tls_port, trust, new_cert, "new", stack, new_ca)
        cached("new", new.version)
        # Hold a write on this pool, reload the same route, then cache a read before
        # the write completes. Its completion must invalidate the new pool's cache.
        new.write_started.clear()
        new.finish_write.clear()
        writer = HTTPConnection(LOOPBACK, http_port, timeout=10)
        stack.callback(writer.close)
        stack.callback(new.finish_write.set)
        writer.request("POST", "/api/mutable")
        assert new.write_started.wait(3)
        reload(proc, log, b"Japka configuration reloaded")
        reads = new.reads
        cached("new", new.version)
        cached("new", new.version)
        assert new.reads == reads + 1
        version = new.version
        new.finish_write.set()
        reply = writer.getresponse()
        assert (reply.status, reply.read()) == (204, b"")
        cached("new", version + 1)
        assert new.reads == reads + 2
        old.release.set()
        read_reply(held, "/api/hold", "old")
        get(held, "/api/after-reload", "new")
        assert held.sock is tls_socket, "TLS keep-alive connection was replaced"
        assert held.sock.getpeercert(binary_form=True) == old_cert
        read_exact_body(reader, slow_headers, SLOW)

        # A valid route and a mismatched key must be rejected together.
        publish(config, candidate(old, "old", key="new"))
        reload(proc, log, b"Japka reload rejected:")
        check_generation(http_port, tls_port, trust, new_cert, "new", stack, new_ca)

        # A valid certificate and invalid routes must also be rejected together.
        invalid = candidate(old, "old")
        invalid["routes"][0]["upstreams"] = []
        publish(config, invalid)
        reload(proc, log, b"Japka reload rejected:")
        check_generation(http_port, tls_port, trust, new_cert, "new", stack, new_ca)

        publish(config, candidate(old, "old"))
        reload(proc, log, b"Japka configuration reloaded")
        check_generation(http_port, tls_port, trust, old_cert, "old", stack, old_ca)


def check_proxy_reload(work, origins):
    old, new = origins
    config = work / "proxy.json"
    log = work / "proxy.log"
    port = free_port()
    publish(config, {})
    peers = ",".join(f"{LOOPBACK}:{origin.server_port}" for origin in origins)
    env = dict(PATH=os.environ["PATH"], DOCUMENT_ROOT=str(work / "public"), LISTEN=f"{LOOPBACK}:{port}",
               JAPKA_CONFIG=str(config), UPSTREAMS=peers, JAPKA_HEALTH_INTERVAL_SECS="1")
    old.ready_status, new.ready_status = 503, 200
    for origin in origins:
        origin.release.clear()
        origin.started.clear()
    with ExitStack() as stack:
        with log.open("wb") as out:
            proc = subprocess.Popen([JAPKA], env=env, stderr=out)
        stack.callback(stop, proc)
        wait_until(proc, log, "listener", lambda: listening(port))
        client = HTTPConnection(LOOPBACK, port, timeout=5)
        held = HTTPConnection(LOOPBACK, port, timeout=5)
        stack.callback(client.close)
        stack.callback(held.close)
        for origin in origins:
            stack.callback(origin.release.set)

        def labels():
            results = []
            for _ in range(4):
                client.request("GET", "/api/item")
                reply = client.getresponse()
                assert reply.status == 200
                results.append(reply.read().split()[0])
            return results

        assert labels() == [b"old", b"new"] * 2  # TCP probes accept both listening peers.
        held.request("GET", "/api/hold")
        assert old.started.wait(3)
        policy = {"balance": "least_connections", "health_interval_secs": 1, "health_path": "/ready"}
        publish(config, {"proxy": policy})
        reload(proc, log, b"Japka configuration reloaded")
        wait_until(proc, log, "503 peer excluded", lambda: labels() == [b"new"] * 4)
        old.release.set()
        read_reply(held, "/api/hold", "old")

        publish(config, {"proxy": dict(policy, balance="round_robin", health_interval_secs=0)})
        reload(proc, log, b"Japka reload rejected:")
        assert labels() == [b"new"] * 4
        old.ready_status = 200
        wait_until(proc, log, "HTTP probes recover the peer", lambda: set(labels()) == {b"old", b"new"})

        # With both peers healthy, least_connections must avoid a busy peer throughout its request.
        for origin in origins:
            origin.release.clear()
            origin.started.clear()
        held.request("GET", "/api/hold")
        wait_until(proc, log, "held upstream", lambda: any(o.started.is_set() for o in origins))
        busy, idle = (old, new) if old.started.is_set() else (new, old)
        assert labels() == [idle.label.encode()] * 4
        busy.release.set()
        read_reply(held, "/api/hold", busy.label)
        assert proc.poll() is None
        print("Same-process TCP-to-HTTP probe reload, least-connections routing and invalid update rejection passed.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--http3", action="store_true", help="Also check HTTP/3 using edge.py dependencies")
    args = parser.parse_args()
    with TemporaryDirectory() as directory, ExitStack() as stack:
        work = Path(directory)
        (work / "public").mkdir()
        (work / "public" / "slow.txt").write_bytes(SLOW)
        origins = []
        for label in ("old", "new"):
            make_certificate(work, label)
            origin = stack.enter_context(ThreadingHTTPServer((LOOPBACK, 0), KeepAliveOrigin))
            origin.ready_status = 200
            origin.label = label
            origin.started, origin.release = Event(), Event()
            origin.write_started, origin.finish_write = Event(), Event()
            origin.reads = origin.version = 0
            Thread(target=origin.serve_forever, daemon=True).start()
            stack.callback(origin.shutdown)
            origins.append(origin)
        check_proxy_reload(work, origins)
        for sendfile in ("0", "1"):
            check(work, origins, sendfile, args.http3)
            print(f"JAPKA_SENDFILE={sendfile}: route and certificate reload, invalid candidate rejection, "
                  "in-flight responses, and TLS keep-alive passed.")
            if args.http3:
                print("HTTP/3 handshakes and routing passed across accepted and rejected candidates.")


if __name__ == "__main__":
    main()
