"""Exercise redirect destinations, SNI reload, and safe upstream failover."""
import argparse
import asyncio
import os
import socket
import ssl
import subprocess
from http.client import HTTPConnection
from http.server import ThreadingHTTPServer
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Thread

from reload import free_port, listening, publish, reload, stop, wait_until
from routing import Origin, JAPKA


def certificate(root, label, dns):
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                    "-subj", f"/CN={dns}/O={label}", "-addext", f"subjectAltName=DNS:{dns}",
                    "-keyout", str(root / f"{label}.key"), "-out", str(root / f"{label}.pem")],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    return {"cert": f"{label}.pem", "key": f"{label}.key"}


async def quic_handshake(port, cert):
    from aioquic.asyncio import connect
    from aioquic.quic.configuration import QuicConfiguration

    config = QuicConfiguration(is_client=True, alpn_protocols=["h3"], server_name="site.test")
    config.load_verify_locations(cafile=str(cert))
    async with connect("127.0.0.1", port, configuration=config):
        pass


def check(root, backend, refused, http3):
    plain, secure = free_port(), free_port()
    default = certificate(root, "default", "default.test")
    named = certificate(root, "site", "site.test")
    rotated = certificate(root, "rotated", "site.test")
    trust = ssl.create_default_context()
    for entry in (default, named, rotated):
        trust.load_verify_locations(str(root / entry["cert"]))
    peers = [f"127.0.0.1:{refused}", f"127.0.0.1:{backend.server_port}"]
    config = {"tls": dict(default, hosts=[dict(named, host="SITE.test")]),
              "routes": [{"prefix": p, "upstreams": peers, "failover": True}
                         for p in ("/api/", "/post/", "/body/")]}
    path, log = root / "config.json", root / "log"
    publish(path, config)
    env = dict(os.environ, LISTEN=f"127.0.0.1:{plain}", TLS_LISTEN=f"127.0.0.1:{secure}",
               JAPKA_CONFIG=str(path), JAPKA_HTTPS_REDIRECT=f"site.test:{secure}", DOCUMENT_ROOT=str(root))

    def tls_request(host, method="GET", target="/", body=None):
        conn = HTTPConnection("127.0.0.1", secure, timeout=3)
        try:
            conn.connect()
            conn.sock = trust.wrap_socket(conn.sock, server_hostname=host)
            cert = conn.sock.getpeercert(binary_form=True)
            conn.request(method, target, body=body, headers={"Host": host})
            response = conn.getresponse()
            return cert, response.status, response.read()
        finally:
            conn.close()

    with log.open("w") as stderr:
        proc = subprocess.Popen([JAPKA], env=env, stderr=stderr)
        try:
            wait_until(proc, log, "TLS listening", lambda: listening(secure))
            conn = HTTPConnection("127.0.0.1", plain, timeout=3)
            try:
                conn.request("POST", "/api/a%20b?q=x", body=b"data", headers={"Host": "untrusted.test"})
                response = conn.getresponse()
                assert response.status == 308
                assert response.getheader("Location") == f"https://site.test:{secure}/api/a%20b?q=x"
            finally:
                conn.close()
            first, status, data = tls_request("site.test", target="/api/orders?q=2")
            assert (status, data) == (200, b"healthy /api/orders?q=2")
            assert tls_request("default.test")[0] != first
            assert tls_request("site.test", "POST", "/post/write", b"data")[1] == 502
            assert tls_request("site.test", "GET", "/body/read", b"data")[1] == 502
            config["tls"]["hosts"] = [dict(rotated, host="site.test")]
            publish(path, config)
            reload(proc, log, b"configuration reloaded")
            if http3:
                asyncio.run(asyncio.wait_for(quic_handshake(secure, root / rotated["cert"]), 5))
            second = tls_request("site.test")[0]
            assert second != first
            config["tls"]["hosts"][0]["key"] = default["key"]
            publish(path, config)
            reload(proc, log, b"reload rejected")
            assert tls_request("site.test")[0] == second
        finally:
            stop(proc)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--http3", action="store_true", help="verify named QUIC certificate after reload using aioquic")
    args = parser.parse_args()
    with TemporaryDirectory() as tmp, socket.socket() as refused, ThreadingHTTPServer(("127.0.0.1", 0), Origin) as backend:
        # Reserve a non-listening port that also returns ECONNREFUSED on macOS.
        refused.connect(backend.server_address)
        backend.label = "healthy"
        Thread(target=backend.serve_forever, daemon=True).start()
        try:
            check(Path(tmp), backend, refused.getsockname()[1], args.http3)
        finally:
            backend.shutdown()
    print("HTTPS redirects, SNI selection and rotation, rollback, and connect-error failover passed.")
