"""Verify the opt-in immutable asset cache against a Linux Japka release."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from http.client import HTTPSConnection
import os
from pathlib import Path
import socket
import ssl
import subprocess
import sys
from tempfile import TemporaryDirectory
import time

from routing import JAPKA, launch
from sendfile import fetch, read_exact_body, read_head, request, slow_socket

SETTING = "JAPKA_ASSET_CACHE_ENTRIES"
SMALL = b"const answer = 42;\n"
LARGE = bytes(range(256)) * (4 * 1024 * 1024 // 256)
REPLACEMENT = b"replacement content for the same path\n"
INITIAL_MTIME = 1_700_000_000
ASSET_HEADERS = ("content-type", "content-length", "accept-ranges", "etag", "last-modified")
EXEC_WITH_PID = (
    "import os, pathlib, sys; pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); "
    "os.execv(sys.argv[2], [sys.argv[2]])"
)


def write_asset(root, name, body, changed=False):
    asset = root / name
    asset.parent.mkdir(parents=True, exist_ok=True)
    pending = asset.with_name(f".{asset.name}.new")
    pending.write_bytes(body)
    modified = INITIAL_MTIME + int(changed)
    os.utime(pending, (modified, modified))
    pending.replace(asset)


def environment(root, cap, **settings):
    env = dict(os.environ, DOCUMENT_ROOT=str(root), TOKIO_WORKER_THREADS="1",
               JAPKA_SENDFILE="1", COMPRESS="0")
    env.pop(SETTING, None)
    if cap is not None:
        env[SETTING] = str(cap)
    return dict(env, **settings)


@contextmanager
def serve(root, cap, **settings):
    pid_path = root / ".japka-pid"
    command = [sys.executable, "-c", EXEC_WITH_PID, str(pid_path), JAPKA]
    with launch(command, environment(root, cap, **settings)) as port:
        yield port, int(pid_path.read_text())


def expected_get(port, path, expected):
    status, headers, body = fetch(port, "GET", path)
    assert status == 200 and body == expected, (path, status, len(body), len(expected))
    assert headers["content-length"] == str(len(expected)), (path, headers)
    return headers


def metadata(headers):
    return tuple(headers[key] for key in ASSET_HEADERS)


def fixture_fds(pid, root, names):
    paths = {str(root / name): name for name in names}
    now = []
    for fd in Path(f"/proc/{pid}/fd").iterdir():
        try:
            target = os.readlink(fd).removesuffix(" (deleted)")
        except FileNotFoundError:
            continue
        if target in paths:
            now.append(paths[target])
    return sorted(now)


def wait_for_fds(pid, root, names, count):
    deadline = time.monotonic() + 2
    while True:
        current = fixture_fds(pid, root, names)
        if len(current) == count:
            return current
        assert time.monotonic() < deadline, (names, count, current)
        time.sleep(.01)


def positional_downloads(port, path):
    # Read both headers before consuming either body to keep downloads overlapping.
    with slow_socket(port) as first, slow_socket(port) as second:
        for client in (first, second):
            client.sendall(request("GET", path))
        with first.makefile("rb") as a, second.makefile("rb") as b:
            headers_a = read_head(a)
            headers_b = read_head(b)
            with ThreadPoolExecutor(max_workers=2) as readers:
                tasks = [readers.submit(read_exact_body, reader, headers, LARGE)
                         for reader, headers in ((a, headers_a), (b, headers_b))]
                for task in tasks:
                    task.result()


def enabled(root):
    small_name, large_name, mutable_name = "app.8f31.js", "download.A84F.bin", "live.js"
    names = (small_name, large_name, mutable_name)
    for name, body in zip(names, (SMALL, LARGE, SMALL)):
        write_asset(root, name, body)
    with serve(root, 2, JAPKA_BLOCKING_THREADS="1") as (port, pid):
        small_path = f"/{small_name}"
        large_path = f"/{large_name}"
        small = expected_get(port, small_path, SMALL)
        large = expected_get(port, large_path, LARGE)
        expected_get(port, f"/{mutable_name}", SMALL)
        assert wait_for_fds(pid, root, names, 1) == [large_name]
        for name in names:
            write_asset(root, name, REPLACEMENT, changed=True)
        assert metadata(expected_get(port, small_path, SMALL)) == metadata(small)
        expected_get(port, f"/{mutable_name}", REPLACEMENT)
        status, headers, body = fetch(port, "HEAD", large_path)
        assert status == 200 and body == b""
        assert metadata(headers) == metadata(large)
        for method, validator, source in (
            ("HEAD", "If-None-Match", "etag"),
            ("GET", "If-Modified-Since", "last-modified"),
        ):
            status, conditional, body = fetch(port, method, large_path, {validator: large[source]})
            assert status == 304 and body == b""
            assert (conditional["etag"], conditional["last-modified"]) == (
                large["etag"], large["last-modified"])
        status, ranged, body = fetch(port, "GET", large_path, {"Range": "bytes=0-2"})
        assert status == 206 and body == REPLACEMENT[:3]
        assert ranged["content-range"] == f"bytes 0-2/{len(REPLACEMENT)}"
        positional_downloads(port, large_path)
        upper = "upper." + "b" * 64 + ".js"
        write_asset(root, upper, SMALL)
        expected_get(port, f"/{upper}", SMALL)
        write_asset(root, upper, REPLACEMENT, changed=True)
        expected_get(port, f"/{upper}", SMALL)
    print("Enabled cache retained small bytes and the large descriptor, preserved HEAD validators, "
          "served overlapping downloads, and bypassed the cache for ranges and mutable names.")


def capacity(root):
    names = ("first.1111.bin", "second.2222.bin", "third.3333.bin")
    with serve(root, 2) as (port, pid):
        for index, name in enumerate(names, 1):
            write_asset(root, name, LARGE)
            expected_get(port, f"/{name}", LARGE)
            kept = wait_for_fds(pid, root, names, min(index, 2))
            assert len(set(kept)) == min(index, 2), kept
        evicted = (set(names) - set(kept)).pop()
        write_asset(root, evicted, REPLACEMENT, changed=True)
        expected_get(port, f"/{evicted}", REPLACEMENT)
    print("Three large asset lookups at cap two retained exactly two distinct descriptors.")


def not_admitted(root):
    with serve(root, 2) as (port, _):
        cases = (
            ("short.abc.js", "GET", None, {}, 200),
            ("long." + "d" * 65 + ".js", "GET", None, {}, 200),
            ("nonhex.8f3z.js", "GET", None, {}, 200),
            ("range.12a3.js", "GET", None, {"Range": "bytes=0-2"}, 206),
            ("head.123a.js", "HEAD", None, {}, 200),
            ("conditional.abc1.js", "GET", None, {"If-None-Match": "*"}, 304),
            ("failed.abc2.js", "GET", None, {"If-Match": '"wrong"'}, 412),
            ("encoded.abc3.js", "GET", "/%65ncoded.abc3.js", {}, 200),
            ("nested/slash.abc4.js", "GET", "/nested//slash.abc4.js", {}, None),
            ("nested/dot.abc5.js", "GET", "/nested/./dot.abc5.js", {}, None),
            ("parent.abc6.js", "GET", "/nested/../parent.abc6.js", {}, None),
        )
        for name, method, alias, headers, expected in cases:
            path = f"/{name}"
            write_asset(root, name, SMALL)
            status, _, body = fetch(port, method, alias or path, headers)
            if expected is not None:
                assert status == expected, (path, status, expected)
            if status == 200 and method == "GET":
                assert body == SMALL, (path, status, body)
            write_asset(root, name, REPLACEMENT, changed=True)
            if alias and status == 200:
                expected_get(port, alias, REPLACEMENT)
            expected_get(port, path, REPLACEMENT)
        missing = "created.89af.js"
        status, _, _ = fetch(port, "GET", f"/{missing}")
        assert status == 404
        write_asset(root, missing, SMALL)
        expected_get(port, f"/{missing}", SMALL)
    print("Ineligible filenames, aliases, HEAD, partial/conditional responses, and misses did not populate the cache.")


def disabled(root):
    name = "off.8f31.js"
    for cap, settings in (
        (None, {}),
        (0, {}),
        (2, {"JAPKA_SENDFILE": "0"}),
        (2, {"COMPRESS": "1"}),
    ):
        write_asset(root, name, SMALL)
        with serve(root, cap, **settings) as (port, _):
            expected_get(port, f"/{name}", SMALL)
            write_asset(root, name, REPLACEMENT, changed=True)
            expected_get(port, f"/{name}", REPLACEMENT)
    for invalid in ("-1", "invalid"):
        proc = subprocess.run([JAPKA], env=environment(root, invalid, LISTEN="127.0.0.1:0"),
                              capture_output=True, text=True, timeout=5)
        assert proc.returncode != 0 and SETTING in proc.stderr, (invalid, proc.returncode, proc.stderr)
    print("Default and explicit zero are off, buffered mode and compression bypass the cache, "
          "and invalid caps fail startup.")


def tls_bypass(root):
    name = "secure.81ab.js"
    write_asset(root, name, SMALL)
    cert, key = root / "cert.pem", root / "key.pem"
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                    "-subj", "/CN=localhost", "-days", "1", "-keyout", str(key),
                    "-out", str(cert), "-addext", "subjectAltName=IP:127.0.0.1"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   check=True)
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        tls_port = reservation.getsockname()[1]
    with serve(root, 2, TLS_LISTEN=f"127.0.0.1:{tls_port}", TLS_CERT=str(cert), TLS_KEY=str(key)) as (port, _):
        expected_get(port, f"/{name}", SMALL)
        write_asset(root, name, REPLACEMENT, changed=True)
        context = ssl.create_default_context(cafile=str(cert))

        def tls_get(asset, expected):
            client = HTTPSConnection("127.0.0.1", tls_port, context=context, timeout=5)
            try:
                client.request("GET", f"/{asset}")
                response = client.getresponse()
                assert response.status == 200 and response.version == 11
                assert response.read() == expected
            finally:
                client.close()

        tls_get(name, REPLACEMENT)
        expected_get(port, f"/{name}", SMALL)
        tls_only = "tls.1122.js"
        write_asset(root, tls_only, SMALL)
        tls_get(tls_only, SMALL)
        write_asset(root, tls_only, REPLACEMENT, changed=True)
        expected_get(port, f"/{tls_only}", REPLACEMENT)
    print("TLS HTTP/1 observed the new file while the plaintext cache retained the original.")


if __name__ == "__main__":
    for check in (enabled, capacity, not_admitted, disabled, tls_bypass):
        with TemporaryDirectory() as directory:
            check(Path(directory).resolve())
