"""Verify Rill returns a gateway error when the selected upstream refuses a connection."""

from contextlib import contextmanager
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
from queue import Queue
import socket
import subprocess
from tempfile import TemporaryDirectory
from threading import Thread
import time


HOST = "127.0.0.1"
ROOT = Path(__file__).resolve().parents[1]
HEALTHY_BODY = b"healthy backend\n"


class HealthyBackend(BaseHTTPRequestHandler):
    def do_GET(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.server.requests.put((self.command, self.path, body))
        self.send_response(200)
        self.send_header("Content-Length", str(len(HEALTHY_BODY)))
        self.end_headers()
        self.wfile.write(HEALTHY_BODY)

    do_POST = do_GET

    def log_message(self, *args):
        pass


@contextmanager
def running(command, env, port):
    with subprocess.Popen(command, cwd=ROOT, env=env) as process:
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f"{command[0]} exited with status {process.returncode}")
                try:
                    with socket.create_connection((HOST, port), timeout=0.1):
                        break
                except ConnectionRefusedError:
                    time.sleep(0.02)
            else:
                raise RuntimeError(f"{command[0]} did not listen on {HOST}:{port}")
            yield
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()


def request(port, method, path, body=None):
    connection = HTTPConnection(HOST, port, timeout=5)
    try:
        connection.request(method, path, body=body)
        response = connection.getresponse()
        return response.status, response.getheader("Content-Type"), response.read()
    finally:
        connection.close()


def exercise(method, command, state, upstreams, healthy_requests):
    # Use a fresh process so the refused backend starts first for both methods.
    with socket.socket() as listener:
        listener.bind((HOST, 0))
        port = listener.getsockname()[1]
    env = dict(
        os.environ,
        TEST_STATE=str(state),
        DOCUMENT_ROOT=str(state),
        UPSTREAMS=upstreams,
        PORT=str(port),
        LISTEN=f"{HOST}:{port}",
    )
    failed_path = "/api/refused?check=1"
    healthy_path = "/api/healthy?check=1"
    body = b"post payload" if method == "POST" else None
    with running(command, env, port):
        failure = request(port, method, failed_path, body)
        status, _, control_body = request(port, method, healthy_path, body)
    observed = []
    while not healthy_requests.empty():
        observed.append(healthy_requests.get_nowait())
    if (status, control_body) != (200, HEALTHY_BODY):
        raise AssertionError(f"{command[0]} {method} healthy control returned {status}, {control_body!r}")
    if observed != [(method, healthy_path, body or b"")]:
        raise AssertionError(f"{command[0]} {method} healthy backend received {observed!r}")
    return failure


def main():
    rill = [str(Path(os.getenv("RILL", ROOT / "target/debug/rill")).resolve())]
    with TemporaryDirectory() as directory, socket.socket() as refused:
        state = Path(directory)
        # Keep the port reserved without calling listen, so TCP connections are refused.
        refused.bind((HOST, 0))
        with ThreadingHTTPServer((HOST, 0), HealthyBackend) as healthy:
            healthy.requests = Queue()
            worker = Thread(target=healthy.serve_forever, daemon=True)
            worker.start()
            try:
                upstreams = f"{HOST}:{refused.getsockname()[1]},{HOST}:{healthy.server_port}"
                for method in ("GET", "POST"):
                    status, media, body = exercise(method, rill, state, upstreams, healthy.requests)
                    assert (status, media) == (502, "text/html") and b"502 Bad Gateway" in body
                    print(f"{method} returned 502; the healthy backend received only the control request")
            finally:
                healthy.shutdown()
                worker.join()


if __name__ == "__main__":
    main()
