"""Check file freshness and conditional responses during concurrent static requests."""
from concurrent.futures import ThreadPoolExecutor
from http.client import HTTPConnection
import os
from pathlib import Path
from tempfile import TemporaryDirectory

from routing import RILL, launch


def main():
    with TemporaryDirectory() as directory:
        root = Path(directory)
        docs = root / "docs"
        docs.mkdir()
        index = docs / "index.html"
        fixed_time = 1700000000
        sibling = docs / "other.bin"
        sibling.write_bytes(b"x" * 4096)
        os.utime(sibling, (fixed_time, fixed_time))
        settings = dict(os.environ, DOCUMENT_ROOT=directory)
        with launch([RILL], settings) as port, ThreadPoolExecutor(16) as clients:
            def request(case):
                method, path, headers, status, expected = case
                connection = HTTPConnection("127.0.0.1", port, timeout=5)
                try:
                    connection.request(method, path, headers=headers)
                    res = connection.getresponse()
                    data = res.read()
                    assert res.status == status, (method, path, res.status)
                    if expected is not None:
                        assert data == expected, (method, path, len(data))
                    if status in (200, 304):
                        assert res.getheader("ETag") == etag
                    if method == "HEAD" and status == 200:
                        assert int(res.getheader("Content-Length")) == len(body)
                finally:
                    connection.close()

            for label in (b"a", b"b", b"c"):
                body = label * 4096
                replacement = root / "replacement"
                replacement.write_bytes(body)
                # Equal length and mtime intentionally produce the same ETag.
                os.utime(replacement, (fixed_time, fixed_time))
                replacement.replace(index)
                etag = f'"{fixed_time:x}-{len(body):x}"'
                cases = [
                    ("GET", "/docs/", {}, 200, body),
                    ("GET", "/docs/index.html", {}, 200, body),
                    ("GET", "/docs/other.bin", {}, 200, b"x" * 4096),
                    ("HEAD", "/docs/", {}, 200, b""),
                    ("GET", "/docs/", {"If-None-Match": etag}, 304, b""),
                    ("GET", "/docs/", {"If-Match": '"other"'}, 412, None),
                    ("GET", "/docs/index.html", {"Range": "bytes=2-5"}, 206, body[2:6]),
                ]
                list(clients.map(request, cases * 16))
            index.unlink()
            list(clients.map(request, [("GET", "/docs/", {}, 404, None)] * 16))
    print("Concurrent GET, HEAD, validators and ranges stayed current across index replacements and removal.")


if __name__ == "__main__":
    main()
