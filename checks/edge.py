"""Check three TLS protocols against the same static file. Requires curl HTTP2 and aioquic."""
import argparse
import asyncio
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import pathlib
import threading
import subprocess

from curl_cffi import CurlHttpVersion, CurlOpt, requests

from aioquic.asyncio import QuicConnectionProtocol, connect
from aioquic.h3.connection import H3Connection
from aioquic.h3.events import HeadersReceived, DataReceived
from aioquic.quic.configuration import QuicConfiguration


class Client(QuicConnectionProtocol):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.http = H3Connection(self._quic)
        self.results = {}

    async def request(self, port, path, data=None):
        stream = self._quic.get_next_available_stream_id()
        future = asyncio.get_running_loop().create_future()
        self.results[stream] = future, {}, bytearray()
        headers = [(b":method", b"GET" if data is None else b"POST"),
                   (b":scheme", b"https"), (b":authority", f"localhost:{port}".encode()),
                   (b":path", path.encode())]
        if data is not None:
            headers.append((b"content-length", str(len(data)).encode()))
        self.http.send_headers(stream, headers, end_stream=data is None)
        if data is not None:
            self.http.send_data(stream, data, end_stream=True)
        self.transmit()
        return await asyncio.wait_for(future, timeout=10)

    def quic_event_received(self, event):
        for item in self.http.handle_event(event):
            entry = self.results.get(item.stream_id)
            if entry is None:
                continue
            future, headers, body = entry
            if isinstance(item, HeadersReceived):
                headers.update(item.headers)
            elif isinstance(item, DataReceived):
                body.extend(item.data)
            if item.stream_ended and not future.done():
                future.set_result((headers, bytes(body)))



class Echo(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_HEAD(self):
        self.send_response(200)
        self.send_header("content-length", "42")
        self.end_headers()

    def do_GET(self):
        self.send_response(204)
        self.end_headers()

    def do_POST(self):
        data = self.rfile.read(int(self.headers["content-length"]))
        self.send_response(200)
        self.send_header("content-length", str(len(data)))
        self.send_header("x-upstream-version", self.request_version)
        self.send_header("x-upstream-target", self.path)
        self.send_header("connection", "x-one-hop")
        self.send_header("x-one-hop", "removed")
        self.end_headers()
        self.wfile.write(data)


def check_framing(args, expected, proxy=False):
    origin = f"https://localhost:{args.port}"
    for requested, negotiated in [
        (CurlHttpVersion.V2TLS, CurlHttpVersion.V2_0),
        (CurlHttpVersion.V3ONLY, CurlHttpVersion.V3),
    ]:
        with requests.Session(http_version=requested, verify=str(args.ca), trust_env=False,
                curl_options={CurlOpt.RESOLVE: [f"localhost:{args.port}:127.0.0.1"]}) as client:
            def request(method, path, status, headers=None):
                res = client.request(method, origin + path, headers=headers, timeout=5)
                assert res.http_version == negotiated and res.status_code == status
                assert "alt-svc" in res.headers and "date" in res.headers
                return res

            if proxy:
                head = request("HEAD", "/api/echo", 200)
                assert head.content == b"" and head.headers["content-length"] == "42"
                empty = request("GET", "/api/echo", 204)
                assert empty.content == b"" and "content-length" not in empty.headers
                continue
            head = request("HEAD", args.path, 200)
            assert head.content == b"" and int(head.headers["content-length"]) == len(expected)
            missing = "/japka-edge-check-missing-file"
            head = request("HEAD", missing, 404)
            error = request("GET", missing, 404)
            assert head.content == b"" and int(head.headers["content-length"]) == len(error.content) > 0
            cond = request("GET", args.path, 304, {"If-None-Match": "*"})
            assert cond.content == b"" and "content-length" not in cond.headers
            if expected:
                part = request("GET", args.path, 206, {"Range": "bytes=0-0"})
                assert part.content == expected[:1]
                assert part.headers["content-range"] == f"bytes 0-0/{len(expected)}"


async def check(args):
    expected = args.file.read_bytes()
    check_framing(args, expected)
    print("HTTP/2 and HTTP/3 static HEAD, 404, 304 and range checks passed")
    url = f"https://localhost:{args.port}{args.path}"
    for flag, version in [("--http1.1", b"1.1"), ("--http2", b"2")]:
        out = subprocess.check_output(["curl", "--silent", "--show-error", "--fail",
            "--noproxy", "*", "--resolve", f"localhost:{args.port}:127.0.0.1",
            "--max-time", "10", "--cacert", str(args.ca), flag,
            "--write-out", r"\n%{response_code} %{http_version}", url])
        assert out == expected + b"\n200 " + version, f"HTTP/{version.decode()} mismatch"
        print(f"HTTP/{version.decode()} 200, {len(expected)} bytes")

    config = QuicConfiguration(is_client=True, alpn_protocols=["h3"], server_name="localhost")
    config.load_verify_locations(str(args.ca))
    async with asyncio.timeout(15):
        async with connect("127.0.0.1", args.port, configuration=config, create_protocol=Client) as client:
            responses = await asyncio.gather(client.request(args.port, args.path), client.request(args.port, args.path))
            for headers, data in responses:
                assert headers[b":status"] == b"200" and data == expected
                assert b"h3=" in headers[b"alt-svc"]
            if args.echo_port:
                data = bytes(range(256)) * 1024
                server = ThreadingHTTPServer(("127.0.0.1", args.echo_port), Echo)
                thread = threading.Thread(target=server.serve_forever, daemon=True)
                thread.start()
                try:
                    await asyncio.to_thread(check_framing, args, expected, True)
                    headers, reply = await client.request(args.port, "/api/echo?test=h3", data)
                    assert headers[b":status"] == b"200" and reply == data
                    assert headers[b"x-upstream-version"] == b"HTTP/1.1"
                    assert headers[b"x-upstream-target"] == b"/api/echo?test=h3"
                    assert b"connection" not in headers and b"x-one-hop" not in headers
                    print(f"HTTP/3 POST proxied to HTTP/1.1 and echoed {len(reply)} bytes")
                finally:
                    server.shutdown()
                    thread.join()
                    server.server_close()
            print(f"HTTP/3 200, {len(expected)} bytes on each of two concurrent QUIC streams")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--ca", type=pathlib.Path, required=True)
    parser.add_argument("--path", default="/index.html")
    parser.add_argument("--file", type=pathlib.Path, default=pathlib.Path("public/index.html"))
    parser.add_argument("--echo-port", type=int, help="Start a loopback echo backend on the port configured in UPSTREAMS")
    asyncio.run(check(parser.parse_args()))
