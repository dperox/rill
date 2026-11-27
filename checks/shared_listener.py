"""Exercise static files, a streamed proxy upload and subsequent files on one connection."""
from contextlib import ExitStack
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import socket
from tempfile import TemporaryDirectory
from threading import Thread

from routing import launch, RILL
from sendfile import LARGE, request, read_head, read_exact_body


class Echo(BaseHTTPRequestHandler):
    def do_POST(self):
        data = self.rfile.read(int(self.headers["Content-Length"]))
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def main():
    with ExitStack() as stack:
        root = Path(stack.enter_context(TemporaryDirectory()))
        (root / "large.bin").write_bytes(LARGE)
        origin = stack.enter_context(ThreadingHTTPServer(("127.0.0.1", 0), Echo))
        thread = Thread(target=origin.serve_forever, daemon=True)
        thread.start()
        stack.callback(thread.join)
        stack.callback(origin.shutdown)
        port = stack.enter_context(launch([RILL], dict(os.environ, DOCUMENT_ROOT=str(root),
            UPSTREAMS=f"127.0.0.1:{origin.server_port}", RILL_SENDFILE="1")))
        with socket.create_connection(("127.0.0.1", port), timeout=10) as sock:
            with sock.makefile("rb") as reader:
                upload = b"proxied input" * 8192
                sock.sendall(request("GET", "/large.bin")
                             + request("POST", "/api/echo", {"Content-Length": str(len(upload))})
                             + upload + request("GET", "/large.bin", {"Connection": "close"}))
                for body in (LARGE, upload, LARGE):
                    read_exact_body(reader, read_head(reader), body)
                assert reader.read(1) == b""
    print("One connection served a file, proxied an upload and served the next file.")


if __name__ == "__main__":
    main()
