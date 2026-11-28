"""Run against a native Windows Japka build via the JAPKA environment variable."""
import gzip
import os
import socket
import struct
from contextlib import ExitStack
from pathlib import Path
from tempfile import TemporaryDirectory

from routing import JAPKA, launch
from sendfile import LARGE, NEXT, SMALL, fetch, pipeline, ranges_and_validators, read_head, request


def saturated(port):
    # Both downloads have unread bodies larger than the TCP send window.
    # The third large GET must finish before either stalled reader is released.
    with ExitStack() as stack:
        for _ in range(2):
            sock = stack.enter_context(socket.socket())
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 8192)
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("HH", 1, 0))
            sock.settimeout(10)
            sock.connect(("127.0.0.1", port))
            sock.sendall(request("GET", "/slow.bin"))
            reader = stack.enter_context(sock.makefile("rb"))
            assert int(read_head(reader)[b"content-length"]) == 64 * 1024**2
            assert reader.read(1) == b"\0"
        assert fetch(port, "GET", "/large.txt")[2] == LARGE
    # Disconnect both readers mid-transfer, then reuse a pipelined connection.
    pipeline(port)


def main():
    if os.name != "nt":
        raise SystemExit("Run using Windows Python and a native japka.exe")
    with TemporaryDirectory() as directory:
        root = Path(directory)
        for name, data in (("small", SMALL), ("large", LARGE), ("next", NEXT)):
            (root / f"{name}.txt").write_bytes(data)
        with (root / "slow.bin").open("wb") as slow:
            slow.truncate(64 * 1024**2)
        env = dict(os.environ, DOCUMENT_ROOT=directory, TOKIO_WORKER_THREADS="1",
                   JAPKA_BLOCKING_THREADS="1")
        env.pop("JAPKA_SENDFILE", None)
        env.pop("RILL_SENDFILE", None)
        with launch([JAPKA], dict(env, COMPRESS="0")) as port:
            pipeline(port)
            ranges_and_validators(port)
            saturated(port)
        with launch([JAPKA], dict(env, COMPRESS="0", JAPKA_SENDFILE="0")) as port:
            pipeline(port)
            ranges_and_validators(port)
        with launch([JAPKA], dict(env, COMPRESS="1")) as port:
            status, headers, body = fetch(port, "GET", "/large.txt", {"Accept-Encoding": "gzip"})
            assert status == 200 and headers.get("content-encoding") == "gzip"
            assert gzip.decompress(body) == LARGE
    print("Windows file delivery passed pipeline, HEAD, validators, ranges, slow readers, resets and gzip checks.")


if __name__ == "__main__":
    main()
