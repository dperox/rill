"""Verify HTTP/1 framing and static responses with sendfile disabled and enabled."""
import gzip
from http.client import HTTPConnection
import os
from pathlib import Path
import socket
import struct
from tempfile import TemporaryDirectory

from routing import JAPKA, launch

SMALL = b"small response"
NEXT = b"next response"
LARGE = bytes(range(256)) * 1024
SLOW = LARGE * 32
REPLACEMENT = LARGE[::-1]


def request(method, path, headers=None):
    fields = {"Host": "localhost", **(headers or {})}
    return (f"{method} {path} HTTP/1.1\r\n"
            + "".join(f"{field}: {value}\r\n" for field, value in fields.items())
            + "\r\n").encode()


def read_head(reader, status=200):
    line = reader.readline()
    assert line.startswith(f"HTTP/1.1 {status} ".encode()), line
    headers = {}
    while True:
        line = reader.readline()
        if line == b"\r\n":
            return headers
        assert line, "incomplete HTTP response headers"
        field, value = line.split(b":", 1)
        headers[field.lower()] = value.strip()


def read_exact_body(reader, headers, expected):
    assert b"transfer-encoding" not in headers, headers
    length = int(headers[b"content-length"])
    assert length == len(expected), (length, len(expected))
    data = reader.read(length)
    assert data == expected, (len(data), length)


def fetch(port, method, path, headers=None):
    client = HTTPConnection("127.0.0.1", port, timeout=10)
    try:
        client.request(method, path, headers=headers or {})
        reply = client.getresponse()
        return reply.status, {key.lower(): value for key, value in reply.getheaders()}, reply.read()
    finally:
        client.close()


def pipeline(port):
    with socket.create_connection(("127.0.0.1", port), timeout=10) as client:
        client.sendall(request("HEAD", "/large.txt")
                       + request("GET", "/small.txt")
                       + request("GET", "/large.txt")
                       + request("GET", "/next.txt", {"Connection": "close"}))
        with client.makefile("rb") as reader:
            head = read_head(reader)
            assert int(head[b"content-length"]) == len(LARGE), head
            read_exact_body(reader, read_head(reader), SMALL)
            large = read_head(reader)
            for field in (b"content-type", b"accept-ranges", b"etag", b"last-modified"):
                assert large[field] == head[field], (field, large[field], head[field])
            read_exact_body(reader, large, LARGE)
            read_exact_body(reader, read_head(reader), NEXT)
            assert reader.read(1) == b"", "unexpected data after final response"


def ranges_and_validators(port):
    status, metadata, body = fetch(port, "HEAD", "/large.txt")
    assert status == 200 and body == b""
    cases = [
        ({"Range": "bytes=123-456"}, 206, LARGE[123:457]),
        ({"Range": f"bytes={len(LARGE)}-"}, 416, None),
        ({"If-None-Match": metadata["etag"]}, 304, b""),
        ({"If-Modified-Since": metadata["last-modified"]}, 304, b""),
        ({"If-Match": '"different"'}, 412, None),
        ({"Range": "bytes=123-456", "If-None-Match": metadata["etag"]}, 304, b""),
    ]
    for headers, expected_status, expected_body in cases:
        status, reply, body = fetch(port, "GET", "/large.txt", headers)
        assert status == expected_status, (headers, status)
        if expected_body is not None:
            assert body == expected_body, (headers, len(body), len(expected_body))
        if status == 206:
            assert reply["content-range"] == f"bytes 123-456/{len(LARGE)}"
            assert int(reply["content-length"]) == len(expected_body)
        if status == 416:
            assert reply["content-range"] == f"bytes */{len(LARGE)}"
        if status == 304:
            assert reply["etag"] == metadata["etag"]


def slow_socket(port):
    client = socket.socket()
    try:
        client.settimeout(10)
        # Keep the advertised receive window small while the client pauses.
        client.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 32768)
        client.connect(("127.0.0.1", port))
    except OSError:
        client.close()
        raise
    return client


def slow_and_disconnected(port):
    with slow_socket(port) as client:
        client.sendall(request("GET", "/slow.txt")
                       + request("GET", "/next.txt", {"Connection": "close"}))
        with client.makefile("rb") as reader:
            headers = read_head(reader)
            status, _, body = fetch(port, "GET", "/small.txt")
            assert status == 200 and body == SMALL
            read_exact_body(reader, headers, SLOW)
            read_exact_body(reader, read_head(reader), NEXT)
            assert reader.read(1) == b"", "unexpected data after final response"

    with slow_socket(port) as client:
        client.sendall(request("GET", "/slow.txt"))
        with client.makefile("rb") as reader:
            assert int(read_head(reader)[b"content-length"]) == len(SLOW)
        # Reset while the body is still pending.
        client.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
    status, _, body = fetch(port, "GET", "/large.txt")
    assert status == 200 and body == LARGE


def file_replacement_and_truncation(port, root, sendfile):
    path = root / "changing.txt"
    path.write_bytes(SLOW)
    replacement = root / "replacement.txt"
    replacement.write_bytes(REPLACEMENT)
    with slow_socket(port) as client:
        client.sendall(request("GET", "/changing.txt")
                       + request("GET", "/changing.txt", {"Connection": "close"}))
        with client.makefile("rb") as reader:
            headers = read_head(reader)
            assert int(headers[b"content-length"]) == len(SLOW)
            replacement.replace(path)
            read_exact_body(reader, headers, SLOW)
            read_exact_body(reader, read_head(reader), REPLACEMENT)
            assert reader.read(1) == b"", "unexpected data after final response"

    if sendfile == "1":
        path.write_bytes(SLOW)
        with slow_socket(port) as client:
            client.sendall(request("GET", "/changing.txt")
                           + request("GET", "/next.txt", {"Connection": "close"}))
            with client.makefile("rb") as reader:
                assert int(read_head(reader)[b"content-length"]) == len(SLOW)
                first_byte = reader.read(1)
                assert first_byte == SLOW[:1]
                with path.open("r+b") as file:
                    file.truncate(0)
                partial = first_byte + reader.read()
                assert len(partial) < len(SLOW) and SLOW.startswith(partial), (
                    "truncation must close the connection before the successor", len(partial))


def main():
    with TemporaryDirectory() as directory:
        root = Path(directory)
        for name, body in (("small", SMALL), ("next", NEXT), ("large", LARGE), ("slow", SLOW)):
            (root / f"{name}.txt").write_bytes(body)
        env = dict(os.environ, DOCUMENT_ROOT=directory, TOKIO_WORKER_THREADS="1")
        for sendfile in ("0", "1"):
            with launch([JAPKA], dict(env, JAPKA_SENDFILE=sendfile, COMPRESS="0")) as port:
                pipeline(port)
                ranges_and_validators(port)
                slow_and_disconnected(port)
                file_replacement_and_truncation(port, root, sendfile)
            with launch([JAPKA], dict(env, JAPKA_SENDFILE=sendfile, COMPRESS="1")) as port:
                status, headers, body = fetch(port, "GET", "/large.txt", {"Accept-Encoding": "gzip"})
                assert status == 200 and headers.get("content-encoding") == "gzip", (status, headers)
                assert gzip.decompress(body) == LARGE
                status, headers, body = fetch(port, "GET", "/large.txt")
                assert status == 200 and "content-encoding" not in headers and body == LARGE
            print(f"JAPKA_SENDFILE={sendfile}: pipeline, ranges, validators, slow reads, "
                  "reset, replacement, and gzip passed.")
            if sendfile == "1":
                print("In-flight truncation closed the connection before the pipelined successor.")


if __name__ == "__main__":
    main()
