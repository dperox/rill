"""Replay the committed HTTP regression cases against Japka."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Thread
import argparse
import json
import os
import subprocess

argparse.ArgumentParser(description=__doc__).parse_args()

repo = Path(__file__).resolve().parents[1]
os.chdir(repo)
tool = os.environ.get("RUST_ANYTHING", str(Path.home() / "projects/rust-anything-tool/target/debug/rust-anything"))

class Backend(BaseHTTPRequestHandler):
    def do_GET(self):
        data = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        body = json.dumps([self.server.label, self.command, self.path, self.headers.get("X-Test"), data.hex()]).encode()
        self.send_response(418 if self.path.endswith("/teapot") else 200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    do_POST = do_GET
    do_PUT = do_GET
    do_DELETE = do_GET

    def log_message(self, *args):
        pass

with TemporaryDirectory() as directory:
    state = Path(directory)
    root = state / "public"
    root.mkdir()
    (root / "index.html").write_bytes(b"<p>Japka fixture</p>\n")
    (root / "bytes.bin").write_bytes(bytes(range(256)))
    (root / "empty.bin").touch()
    (root / "hello world.txt").write_bytes(b"hello world\n")
    (root / "guide").mkdir()
    (root / "guide/index.html").write_bytes(b"<p>Guide</p>\n")
    for entry in root.rglob("*"):
        os.utime(entry, (1700000000, 1700000000))
    servers = [ThreadingHTTPServer(("127.0.0.1", 0), Backend) for _ in range(2)]
    threads = []
    for server, label in zip(servers, ("A", "B")):
        server.label = label
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        threads.append(thread)
    try:
        env = dict(os.environ, TEST_STATE=directory, DOCUMENT_ROOT=str(root),
                   UPSTREAMS=",".join(f"127.0.0.1:{s.server_port}" for s in servers))
        subprocess.run([tool, "app", "replay", ".", "--build", os.getenv("JAPKA", os.getenv("RILL", "target/debug/japka"))],
                       env=env, check=True)
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()
        for thread in threads:
            thread.join()
