"""Exercise ordinary HTTP/1 traffic crossing the adaptive read/write buffer sizes."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import socket
import tempfile
from threading import Thread

from routing import launch, RILL

BULK = bytes(range(256)) * 8192
LONG_HEADER = "v" * (32 * 1024)
TARGET = "/api/echo%20body?key=one&key=two"

class Origin(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        assert self.path == TARGET
        assert self.headers["Host"] == "backend"
        assert self.headers["X-Long"] == LONG_HEADER
        assert self.headers.get("X-First-Hop") is None
        assert self.headers.get("X-Second-Hop") is None
        payload = self.rfile.read(int(self.headers["Content-Length"]))
        assert payload == BULK
        self.send_response(200)
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("X-Long", LONG_HEADER)
        self.send_header("Connection", "x-response-hop")
        self.send_header("X-Response-Hop", "private")
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        pass


def read_head(reader):
    assert reader.readline().startswith(b"HTTP/1.1 200 ")
    headers = {}
    while True:
        line = reader.readline()
        if line == b"\r\n":
            return headers
        if not line:
            raise AssertionError("incomplete HTTP response")
        name, value = line.split(b":", 1)
        headers[name.lower()] = value.strip()


def main():
    with tempfile.TemporaryDirectory() as directory, ThreadingHTTPServer(("127.0.0.1", 0), Origin) as origin:
        root = Path(directory)
        (root / "index.html").write_bytes(b"ready")
        worker = Thread(target=origin.serve_forever, daemon=True)
        worker.start()
        try:
            settings = dict(os.environ, DOCUMENT_ROOT=directory, UPSTREAMS=f"127.0.0.1:{origin.server_port}")
            with launch([RILL], settings) as port:
                with socket.create_connection(("127.0.0.1", port), timeout=10) as client:
                    head = (f"POST {TARGET} HTTP/1.1\r\nHost: site.example\r\n"
                            f"X-Long: {LONG_HEADER}\r\nContent-Length: {len(BULK)}\r\n"
                            "Connection: X-First-Hop\r\nConnection: X-Second-Hop\r\n"
                            "X-First-Hop: remove\r\nX-Second-Hop: remove\r\n\r\n").encode()
                    for offset in range(0, len(head), 347):
                        client.sendall(head[offset:offset+347])
                    client.sendall(BULK)
                    client.sendall(b"HEAD /index.html HTTP/1.1\r\nHost: site.example\r\n\r\n"
                                   b"GET /index.html HTTP/1.1\r\nHost: site.example\r\nConnection: close\r\n\r\n")
                    with client.makefile("rb") as reader:
                        headers = read_head(reader)
                        assert int(headers[b"content-length"]) == len(BULK)
                        assert headers[b"x-long"] == LONG_HEADER.encode() and b"x-response-hop" not in headers
                        assert reader.read(len(BULK)) == BULK
                        assert int(read_head(reader)[b"content-length"]) == 5
                        assert int(read_head(reader)[b"content-length"]) == 5
                        assert reader.read() == b"ready"
        finally:
            origin.shutdown()
            worker.join()
    print("32 KiB request/response fields, 2 MiB proxy body and pipelined HEAD/GET passed.")


if __name__ == "__main__":
    main()
