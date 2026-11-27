from contextlib import ExitStack, contextmanager
from pathlib import Path
import os
import socket
import socketserver
import subprocess
import sys
import threading
import time
import unittest


HOST = "127.0.0.1"
MAIL = (
    (b"* OK Rill test IMAP\r\n", b"A001 NOOP\r\n", b"A001 OK NOOP completed\r\n"),
    (b"+OK Rill test POP3\r\n", b"NOOP\r\n", b"+OK\r\n"),
    (b"220 test Rill SMTP\r\n", b"EHLO rill.test\r\n", b"250-test\r\n250 PIPELINING\r\n"),
)


@contextmanager
def running_server(transport, greeting, reply=None, *, address=(HOST, 0)):
    if transport == socket.SOCK_STREAM:
        class Handler(socketserver.StreamRequestHandler):
            def handle(self):
                self.wfile.write(greeting)
                for line in self.rfile:
                    self.wfile.write(reply if reply is not None else line)
        server_type = socketserver.ThreadingTCPServer
    else:
        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                packet, sock = self.request
                sock.sendto(greeting + packet, self.client_address)
        server_type = socketserver.ThreadingUDPServer
    with server_type(address, Handler) as server:
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.02})
        thread.start()
        try:
            yield server.server_address[1]
        finally:
            server.shutdown()
            thread.join()


def available_port(transport):
    with socket.socket(socket.AF_INET, transport) as sock:
        sock.bind((HOST, 0))
        return sock.getsockname()[1]


def pool(port, upstreams):
    addresses = ",".join(f"{HOST}:{upstream}" for upstream in upstreams)
    return f"{HOST}:{port}={addresses}"


@contextmanager
def running_proxy(tcp="", udp="", balance="round_robin", wait_port=None):
    http_port = available_port(socket.SOCK_STREAM)
    env = {
        **os.environ,
        "LISTEN": f"{HOST}:{http_port}",
        "RILL_TCP_LISTENERS": tcp,
        "RILL_UDP_LISTENERS": udp,
        "RILL_STREAM_BALANCE": balance,
    }
    with subprocess.Popen([BINARY], env=env) as proxy:
        try:
            deadline = time.monotonic() + 5
            while True:
                if proxy.poll() is not None:
                    raise RuntimeError(f"Rill exited with status {proxy.returncode}")
                try:
                    with socket.create_connection((HOST, wait_port or http_port), 0.1):
                        break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.02)
            yield
        finally:
            if proxy.poll() is None:
                proxy.terminate()
                proxy.wait(timeout=5)


class StreamChecks(unittest.TestCase):
    def test_tcp_round_robin(self):
        with ExitStack() as stack:
            reserved_port = available_port(socket.SOCK_STREAM)
            backend_a = stack.enter_context(running_server(socket.SOCK_STREAM, b"A\r\n"))
            backend_b = stack.enter_context(running_server(socket.SOCK_STREAM, b"B\r\n"))
            stack.enter_context(running_proxy(tcp=pool(reserved_port, [backend_a, backend_b])))
            responses = []
            for _ in range(4):
                with socket.create_connection((HOST, reserved_port), 2) as client:
                    with client.makefile("rb") as incoming:
                        responses.append(incoming.readline())
                        client.sendall(b"ping\r\n")
                        client.shutdown(socket.SHUT_WR)
                        self.assertEqual(incoming.read(), b"ping\r\n")
            self.assertEqual(responses, [b"A\r\n", b"B\r\n", b"A\r\n", b"B\r\n"])

    def test_tcp_least_connections(self):
        with ExitStack() as stack:
            reserved_port = available_port(socket.SOCK_STREAM)
            backend_a = stack.enter_context(running_server(socket.SOCK_STREAM, b"A\r\n"))
            backend_b = stack.enter_context(running_server(socket.SOCK_STREAM, b"B\r\n"))
            stack.enter_context(running_proxy(
                tcp=pool(reserved_port, [backend_a, backend_b]), balance="least_connections",
            ))
            first = stack.enter_context(socket.create_connection((HOST, reserved_port), 2))
            first_in = stack.enter_context(first.makefile("rb"))
            self.assertEqual(first_in.readline(), b"A\r\n")
            for _ in range(3):
                with socket.create_connection((HOST, reserved_port), 2) as client:
                    with client.makefile("rb") as incoming:
                        self.assertEqual(incoming.readline(), b"B\r\n")
                        client.shutdown(socket.SHUT_WR)
                        self.assertEqual(incoming.read(), b"")

    def test_udp_round_robin_keeps_client_affinity(self):
        with ExitStack() as stack:
            backend_a = stack.enter_context(running_server(socket.SOCK_DGRAM, b"A:"))
            backend_b = stack.enter_context(running_server(socket.SOCK_DGRAM, b"B:"))
            port = available_port(socket.SOCK_DGRAM)
            stack.enter_context(running_proxy(udp=pool(port, [backend_a, backend_b])))
            clients = [
                stack.enter_context(socket.socket(socket.AF_INET, socket.SOCK_DGRAM))
                for _ in range(2)
            ]
            for client in clients:
                client.settimeout(2)
                client.connect((HOST, port))
            for payload in (b"one", b"", b"\x00\xff" * 2000):
                for client, prefix in zip(clients, (b"A:", b"B:"), strict=True):
                    client.send(payload)
                    self.assertEqual(client.recv(65536), prefix + payload)

    def test_mail_greetings_and_commands(self):
        with ExitStack() as stack:
            # Keep the reservations open until all three client ports are distinct.
            reservations = [socket.socket() for _ in MAIL]
            try:
                for reservation in reservations:
                    reservation.bind((HOST, 0))
                ports = [reservation.getsockname()[1] for reservation in reservations]
                listeners = []
                for port, (greeting, _, reply) in zip(ports, MAIL, strict=True):
                    upstream = stack.enter_context(
                        running_server(socket.SOCK_STREAM, greeting, reply),
                    )
                    listeners.append(pool(port, [upstream]))
            finally:
                for reservation in reservations:
                    reservation.close()
            stack.enter_context(running_proxy(tcp=";".join(listeners)))
            for port, (greeting, command, reply) in zip(ports, MAIL, strict=True):
                with socket.create_connection((HOST, port), 2) as client:
                    with client.makefile("rb") as incoming:
                        self.assertEqual(incoming.readline(), greeting)
                        client.sendall(command)
                        self.assertEqual(incoming.read(len(reply)), reply)


if __name__ == "__main__":
    BINARY = str(Path(sys.argv.pop(1) if len(sys.argv) > 1 else "target/debug/rill").resolve())
    unittest.main(verbosity=2)
