"""Verify Rill's small and large file responses across the 64 KiB cutoff."""
import os
from pathlib import Path
from http.client import HTTPConnection
from tempfile import TemporaryDirectory

from routing import RILL, launch
import email.utils

def response(port, method, path):
    client = HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        client.request(method, path)
        r = client.getresponse()
        return r.status, tuple(r.getheader(h) for h in
            ("Content-Length", "Content-Type", "Accept-Ranges", "ETag", "Last-Modified")), r.read()
    finally:
        client.close()

with TemporaryDirectory() as tmp:
    root = Path(tmp)
    env = dict(os.environ, DOCUMENT_ROOT=tmp)
    with launch([RILL], env) as port:
        for size in (0, 65535, 65536, 65537, 1024 * 1024):
            path = root / "data.bin"
            data = (bytes(range(256)) * ((size + 255) // 256))[:size]
            path.write_bytes(data)
            mtime = int(path.stat().st_mtime)
            expected_headers = (str(size), "application/octet-stream", "bytes",
                                f'"{mtime:x}-{size:x}"', email.utils.formatdate(mtime, usegmt=True))
            for method in ("GET", "HEAD"):
                actual = response(port, method, "/data.bin")
                assert actual == (200, expected_headers, data if method == "GET" else b""), (size, method, actual[:2])
    print("10 GET/HEAD checks passed at file sizes 0 through 1 MiB.")
