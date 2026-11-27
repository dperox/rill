"""Check WebSocket proxying on plain HTTP and TLS. Requires Unix signals and openssl."""

import base64
from contextlib import ExitStack
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
import socket
import ssl
import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Thread

from upstreams import certificates
from reload import LOOPBACK, RILL, free_port, listening, make_certificate, publish, reload, stop, wait_until

KEY = "dGhlIHNhbXBsZSBub25jZQ=="
GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
ACCEPT = base64.b64encode(hashlib.sha1((KEY + GUID).encode()).digest()).decode()


def frame(payload, *, masked=False, opcode=1):
    assert len(payload) < 126
    flags = len(payload) | (0x80 if masked else 0)
    if not masked:
        return bytes((0x80 | opcode, flags)) + payload
    mask = b"rill"
    return bytes((0x80 | opcode, flags)) + mask + bytes(
        value ^ mask[index % 4] for index, value in enumerate(payload)
    )


def read_frame(reader, *, masked=False):
    first = reader.read(2)
    assert len(first) == 2, f"incomplete frame header {first!r}"
    opcode, flags = first
    assert bool(flags & 0x80) == masked
    size = flags & 0x7F
    assert size < 126
    mask = reader.read(4) if masked else b""
    payload = reader.read(size)
    assert len(payload) == size
    if masked:
        payload = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
    return opcode & 0x0F, payload


class Origin(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if self.headers.get("Upgrade") != "websocket":
            return self.reply(200, self.server.label)
        assert self.request_version == "HTTP/1.1"
        assert self.headers.get("Connection") == "upgrade"
        assert self.headers.get("X-Request-Hop") is None
        assert self.headers.get("Sec-WebSocket-Key") == KEY
        assert self.headers.get("Sec-WebSocket-Version") == "13"
        assert self.headers.get("Sec-WebSocket-Protocol") == "other, echo"
        assert self.headers.get("Origin") == "https://app.example"
        if self.path == "/api/reject":
            return self.reply(403, b"origin rejected upgrade")

        self.close_connection = True
        accept = "incorrect" if self.path == "/api/bad-accept" else ACCEPT
        protocol = "unexpected" if self.path == "/api/bad-protocol" else "echo"
        self.wfile.write(
            ("HTTP/1.1 101 Switching Protocols\r\n"
             "Connection: UpGrAdE, X-Response-Hop\r\n"
             "Upgrade: WebSocket\r\n"
             "X-Response-Hop: remove me\r\n"
             f"Sec-WebSocket-Accept: {accept}\r\n"
             f"Sec-WebSocket-Protocol: {protocol}\r\n\r\n").encode()
            + frame(self.server.label)
        )
        if accept != ACCEPT or protocol != "echo":
            return
        assert self.path.startswith("/api/socket?")
        while opcode_and_flags := self.rfile.peek(1):
            opcode, payload = read_frame(self.rfile, masked=True)
            if opcode == 8:
                self.wfile.write(frame(payload, opcode=8))
                return
            assert opcode == 1, opcode
            self.wfile.write(frame(self.server.label + b":" + payload))

    def reply(self, status, body):
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


def connect(port, stack, trust=None):
    sock = stack.enter_context(socket.create_connection((LOOPBACK, port), timeout=5))
    if trust:
        sock = stack.enter_context(trust.wrap_socket(sock, server_hostname="localhost"))
        assert sock.selected_alpn_protocol() == "http/1.1"
    return sock, stack.enter_context(sock.makefile("rb"))


def send_upgrade(sock, path, early=b"", key=KEY):
    sock.sendall(
        (f"GET {path} HTTP/1.1\r\n"
         "Host: localhost\r\n"
         "Connection: keep-alive\r\n"
         "Connection: UpGrAdE, X-Request-Hop\r\n"
         "X-Request-Hop: remove me\r\n"
         "Upgrade: WebSocket\r\n"
         "Origin: https://app.example\r\n"
         "Sec-WebSocket-Version: 13\r\n"
         f"Sec-WebSocket-Key: {key}\r\n"
         "Sec-WebSocket-Protocol: other, echo\r\n"
         "Accept-Encoding: gzip\r\n\r\n").encode() + early
    )


def read_head(reader):
    status_line = reader.readline()
    assert status_line.startswith(b"HTTP/1.1 "), status_line
    status = int(status_line.split()[1])
    headers = {}
    for line in iter(reader.readline, b"\r\n"):
        assert line, "connection closed before header terminator"
        name, value = line.rstrip().split(b":", 1)
        headers[name.lower()] = value.strip()
    return status, headers


def plain_request(port, path):
    with ExitStack() as stack:
        sock, reader = connect(port, stack)
        sock.sendall(f"GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n".encode())
        status, headers = read_head(reader)
        assert status == 200
        return reader.read(int(headers[b"content-length"]))


def active_config(*origins):
    return {
        "routes": [{
            "prefix": "/api/",
            "upstreams": [server.upstream_url for server in origins],
        }],
        "tls": {"cert": "edge.pem", "key": "edge.key"},
    }


def exercise(work, old, new, compress):
    with ExitStack() as stack:
        http_port, tls_port = free_port(), free_port()
        while tls_port == http_port:
            tls_port = free_port()
        config, log = work / "rill.json", work / "rill.log"
        publish(config, active_config(old, new))
        env = dict(os.environ, LISTEN=f"{LOOPBACK}:{http_port}",
                   TLS_LISTEN=f"{LOOPBACK}:{tls_port}", RILL_CONFIG=str(config),
                   RILL_UPSTREAM_CA=str(work / "ca.pem"),
                   DOCUMENT_ROOT=str(work), CACHE_TTL="30", CACHE_COALESCE="1",
                   RILL_HTTP_BALANCE="least_connections", RILL_SENDFILE="1",
                   COMPRESS=compress)
        with log.open("wb") as output:
            process = subprocess.Popen([RILL], env=env, stdout=output, stderr=subprocess.STDOUT)
        stack.callback(stop, process)
        wait_until(process, log, "Rill listeners did not become ready",
                   lambda: listening(http_port) and listening(tls_port))
        trust = ssl.create_default_context(cafile=str(work / "edge.pem"))
        trust.set_alpn_protocols(["http/1.1"])

        path = "/api/socket?cache=1"
        assert plain_request(http_port, path) == old.label
        sock, reader = connect(http_port, stack)
        send_upgrade(sock, path, early=frame(b"early client frame", masked=True))
        status, headers = read_head(reader)
        assert status == 101, (status, headers)
        assert headers[b"connection"] == b"upgrade"
        assert headers[b"upgrade"] == b"websocket"
        assert headers[b"sec-websocket-accept"] == ACCEPT.encode()
        assert headers[b"sec-websocket-protocol"] == b"echo"
        for name in (b"x-response-hop", b"content-encoding", b"content-length"):
            assert name not in headers, headers
        assert read_frame(reader) == (1, new.label)
        assert read_frame(reader) == (1, new.label + b":early client frame")
        assert plain_request(http_port, "/api/lease-first") == old.label
        assert plain_request(http_port, "/api/lease-second") == old.label

        publish(config, active_config(old))
        reload(process, log, b"Rill configuration reloaded")
        sock.sendall(frame(b"after reload", masked=True))
        assert read_frame(reader) == (1, new.label + b":after reload")

        secure, secure_reader = connect(tls_port, stack, trust)
        send_upgrade(secure, "/api/socket?secure=1")
        assert read_head(secure_reader)[0] == 101
        assert read_frame(secure_reader) == (1, old.label)
        secure.sendall(frame(b"encrypted downstream", masked=True))
        assert read_frame(secure_reader) == (1, old.label + b":encrypted downstream")

        for client, stream in ((sock, reader), (secure, secure_reader)):
            client.sendall(frame(b"", masked=True, opcode=8))
            assert read_frame(stream) == (8, b"")
            assert not stream.read(1)

        for path, expected in [("/api/reject", 403), ("/api/bad-accept", 502),
                               ("/api/bad-protocol", 502), ("/api/bad-key", 400)]:
            with ExitStack() as failure:
                client, stream = connect(http_port, failure)
                send_upgrade(client, path, key="invalid" if expected == 400 else KEY)
                status, headers = read_head(stream)
                assert status == expected, (path, status, headers)
                assert b"upgrade" not in headers
                if expected == 403:
                    assert stream.read(int(headers[b"content-length"])) == b"origin rejected upgrade"


def main():
    with TemporaryDirectory() as directory, ExitStack() as stack:
        work = Path(directory)
        make_certificate(work, "edge")
        certificates(work)
        upstream_tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        upstream_tls.load_cert_chain(work / "server.pem", work / "server.key")
        origins = []
        for label, secure in ((b"old", False), (b"new", True)):
            origin = stack.enter_context(ThreadingHTTPServer((LOOPBACK, 0), Origin))
            origin.label = label
            origin.upstream_url = f"http://{LOOPBACK}:{origin.server_port}"
            if secure:
                origin.socket = upstream_tls.wrap_socket(origin.socket, server_side=True)
                origin.upstream_url = f"https://localhost:{origin.server_port}"
            Thread(target=origin.serve_forever, daemon=True).start()
            stack.callback(origin.shutdown)
            origins.append(origin)
        for compress in ("0", "1"):
            exercise(work, *origins, compress)
    print("Plain and TLS WebSocket upgrades, buffering, cache bypass, leases and reload verified")


if __name__ == "__main__":
    main()
