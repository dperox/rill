"""Check file freshness and conditional responses during concurrent static requests."""
from concurrent.futures import ThreadPoolExecutor
from email.utils import formatdate
from http.client import HTTPConnection
import os
from pathlib import Path
from tempfile import TemporaryDirectory

from routing import JAPKA, launch


def check_conditional_ranges(settings, root, fixed_time):
    data = bytes(range(256))
    path = root / "bytes.bin"
    path.write_bytes(data)
    os.utime(path, (fixed_time, fixed_time))
    etag = f'"{fixed_time:x}-{len(data):x}"'
    current = formatdate(fixed_time, usegmt=True)
    older = formatdate(fixed_time - 1, usegmt=True)
    partial = ("Range", "bytes=0-3")
    unsatisfiable = ("Range", "bytes=256-")
    cases = [
        ("GET", (partial, ("If-Range", etag)), 206, data[:4], "bytes 0-3/256"),
        ("GET", (partial, ("If-Range", current)), 206, data[:4], "bytes 0-3/256"),
        ("GET", (partial, ("If-Range", '"old"')), 200, data, None),
        ("GET", (partial, ("If-Range", "W/" + etag)), 200, data, None),
        ("GET", (unsatisfiable, ("If-Range", older)), 200, data, None),
        ("HEAD", (partial, ("If-Range", older)), 200, b"", None),
        ("GET", (unsatisfiable, ("If-Match", '"old"')), 412, None, None),
        ("GET", (unsatisfiable, ("If-None-Match", "*"), ("If-Range", older)), 304, b"", None),
    ]
    for header, value in (
        ("If-Match", etag),
        ("If-None-Match", etag),
        ("If-Modified-Since", current),
        ("If-Unmodified-Since", current),
        ("If-Range", etag),
    ):
        cases.append(("GET", (partial, (header, value), (header.lower(), value)), 400, None, None))

    for sendfile in ("0", "1"):
        with launch([JAPKA], dict(settings, JAPKA_SENDFILE=sendfile)) as port:
            for method, headers, status, body, content_range in cases:
                connection = HTTPConnection("127.0.0.1", port, timeout=5)
                try:
                    connection.putrequest(method, "/bytes.bin")
                    for name, value in headers:
                        connection.putheader(name, value)
                    connection.endheaders()
                    response = connection.getresponse()
                    actual = response.read()
                    context = (sendfile, method, headers)
                    assert response.status == status, (context, response.status)
                    assert response.getheader("Content-Range") == content_range, context
                    if body is not None:
                        assert actual == body, (context, len(actual))
                    if method == "HEAD":
                        assert response.getheader("Content-Length") == str(len(data)), context
                finally:
                    connection.close()


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
        with launch([JAPKA], settings) as port, ThreadPoolExecutor(16) as clients:
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
        check_conditional_ranges(settings, root, fixed_time)
    print("Concurrent GET, HEAD, validators and ranges stayed current across index replacements and removal.")
    print("Conditional ranges, precondition precedence and duplicate headers passed with sendfile enabled and disabled.")


if __name__ == "__main__":
    main()
