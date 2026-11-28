"""Verify Japka returns a gateway error when the selected upstream refuses a connection."""

from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from queue import Queue
import socket
from tempfile import TemporaryDirectory
from threading import Thread

from routing import JAPKA, launch


HOST = "127.0.0.1"
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


def request(port, method, path, body=None):
    connection = HTTPConnection(HOST, port, timeout=5)
    try:
        connection.request(method, path, body=body)
        response = connection.getresponse()
        return response.status, response.getheader("Content-Type"), response.read()
    finally:
        connection.close()


def exercise(method, state, upstreams, healthy_requests):
    # Use a fresh process so the refused backend starts first for both methods.
    env = dict(os.environ, DOCUMENT_ROOT=state, UPSTREAMS=upstreams)
    failed_path = "/api/refused?check=1"
    healthy_path = "/api/healthy?check=1"
    body = b"post payload" if method == "POST" else None
    with launch([JAPKA], env) as port:
        failure = request(port, method, failed_path, body)
        status, _, control_body = request(port, method, healthy_path, body)
    observed = []
    while not healthy_requests.empty():
        observed.append(healthy_requests.get_nowait())
    if (status, control_body) != (200, HEALTHY_BODY):
        raise AssertionError(f"{JAPKA} {method} healthy control returned {status}, {control_body!r}")
    if observed != [(method, healthy_path, body or b"")]:
        raise AssertionError(f"{JAPKA} {method} healthy backend received {observed!r}")
    return failure


def main():
    with TemporaryDirectory() as directory, socket.socket() as refused:
        with ThreadingHTTPServer((HOST, 0), HealthyBackend) as healthy:
            # A connected client's local port stays reserved and refuses new connections.
            refused.connect(healthy.server_address)
            healthy.requests = Queue()
            worker = Thread(target=healthy.serve_forever, daemon=True)
            worker.start()
            try:
                upstreams = f"{HOST}:{refused.getsockname()[1]},{HOST}:{healthy.server_port}"
                for method in ("GET", "POST"):
                    status, media, body = exercise(method, directory, upstreams, healthy.requests)
                    assert (status, media) == (502, "text/html") and b"502 Bad Gateway" in body
                    print(f"{method} returned 502; the healthy backend received only the control request")
            finally:
                healthy.shutdown()
                worker.join()


if __name__ == "__main__":
    main()
