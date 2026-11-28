"""Verify HTTP/1 response framing and incremental delivery on a reused proxy connection."""
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from threading import Event, Thread

from routing import JAPKA, launch


class Origin(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if self.path == "/api/split":
            self.connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nfirst")
            if self.server.release.wait(10):
                self.connection.sendall(b"-last")
        elif self.path == "/api/chunked":
            self.connection.sendall(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
                b"3\r\none\r\n3\r\ntwo\r\n0\r\n\r\n"
            )
        elif self.path == "/api/empty":
            self.connection.sendall(b"HTTP/1.1 204 No Content\r\n\r\n")
        else:
            self.connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nfull")

    def do_HEAD(self):
        self.connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n")


def main():
    with ThreadingHTTPServer(("127.0.0.1", 0), Origin) as origin:
        origin.release = Event()
        worker = Thread(target=origin.serve_forever, daemon=True)
        worker.start()
        try:
            env = dict(os.environ, UPSTREAMS=f"127.0.0.1:{origin.server_port}")
            with launch([JAPKA], env) as port:
                client = HTTPConnection("127.0.0.1", port, timeout=5)
                try:
                    client.connect()
                    original_socket = client.sock
                    for method, path, status, body in [
                        ("GET", "full", 200, b"full"),
                        ("HEAD", "full", 200, b""),
                        ("GET", "empty", 204, b""),
                        ("GET", "split", 200, b"-last"),
                        ("GET", "chunked", 200, b"onetwo"),
                        ("GET", "full", 200, b"full"),
                    ]:
                        client.request(method, f"/api/{path}")
                        response = client.getresponse()
                        assert response.status == status
                        if path == "split":
                            # The origin only finishes once the client has read the prefix.
                            assert response.read(5) == b"first"
                            origin.release.set()
                        assert response.read() == body, (method, path)
                        assert client.sock is original_socket
                finally:
                    origin.release.set()
                    client.close()
        finally:
            origin.shutdown()
            worker.join()
    print("Complete, split, chunked, HEAD and 204 responses passed on one keep-alive socket.")


if __name__ == "__main__":
    main()
