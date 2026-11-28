"""Verify other routes keep making progress while large static clients read slowly."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import socket
import tempfile
from threading import Thread

from routing import JAPKA, launch


class Origin(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", "6")
        self.end_headers()
        self.wfile.write(b"origin")

    def log_message(self, *args):
        pass


def main():
    with tempfile.TemporaryDirectory() as tmp, ExitStack() as stack:
        root = Path(tmp)
        index = b"current index"
        large = bytes(range(256)) * (8 * 1024 * 1024 // 256)
        (root / "index.html").write_bytes(index)
        (root / "large.bin").write_bytes(large)
        origin = stack.enter_context(ThreadingHTTPServer(("127.0.0.1", 0), Origin))
        thread = Thread(target=origin.serve_forever, daemon=True)
        thread.start()
        stack.callback(thread.join)
        stack.callback(origin.shutdown)
        env = dict(os.environ, DOCUMENT_ROOT=tmp, UPSTREAMS=f"localhost:{origin.server_port}")
        port = stack.enter_context(launch([JAPKA], env))

        def connection():
            return HTTPConnection("127.0.0.1", port, timeout=10)

        # Keep sockets open after the response headers. Small receive windows
        # exercise downstream backpressure while independent requests complete.
        held = []
        for _ in range(8):
            client = connection()
            client.connect()
            client.sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 65536)
            stack.callback(client.close)
            client.request("GET", "/large.bin")
            response = client.getresponse()
            assert response.status == 200 and int(response.getheader("Content-Length")) == len(large)
            held.append(response)

        cases = [
            ("GET", "/", {}, 200, index),
            ("HEAD", "/index.html", {}, 200, b""),
            ("GET", "/large.bin", {"Range": "bytes=100-167"}, 206, large[100:168]),
            ("GET", "/api/check", {}, 200, b"origin"),
        ]

        def verify(case):
            method, path, headers, status, body = case
            client = connection()
            try:
                client.request(method, path, headers=headers)
                response = client.getresponse()
                assert response.status == status and response.read() == body, (method, path)
            finally:
                client.close()

        def finish(response):
            assert response.read() == large

        with ThreadPoolExecutor(16) as pool:
            list(pool.map(verify, cases * 8))
            list(pool.map(finish, held))
        print("32 mixed requests completed with eight 8 MiB streams paused; every stream matched.")


if __name__ == "__main__":
    main()
