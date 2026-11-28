"""Verify static HTTP/1.1 keep-alive capacity over loopback on Linux."""

import argparse
import asyncio
from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import resource
import socket
import subprocess
import tempfile
import time

REPO = Path(__file__).resolve().parents[1]
FIXTURE = bytes(range(256)) * 16
REQUEST = b"GET /payload.bin HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n"
CLEAR_ENV = {
    "ALLOW_IPS", "DENY_IPS", "RATE_LIMIT", "COMPRESS", "CACHE_TTL", "LISTEN",
    "UPSTREAMS", "JAPKA_ROUTES", "RILL_ROUTES", "SSL_CERT", "SSL_KEY", "TLS_CERT", "TLS_KEY",
    "TLS_LISTEN", "HTTP3_LISTEN", "TCP_LISTEN", "UDP_LISTEN",
}


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--connections", type=int, default=4096)
    parser.add_argument("--batch", type=int, default=128)
    parser.add_argument("--idle", type=float, default=5)
    parser.add_argument("--server-cpu", type=int, default=16)
    parser.add_argument("--client-cpus", type=int, nargs="+", default=[18, 20])
    parser.add_argument("--japka", "--rill", type=Path, default=REPO / "target/release/japka")
    result = parser.parse_args()
    if result.connections < 1 or result.batch < 1 or result.idle <= 0:
        parser.error("connections, batch and idle must be positive")
    available = os.sched_getaffinity(0)
    if (result.server_cpu in result.client_cpus
            or not {result.server_cpu, *result.client_cpus} <= available):
        parser.error("select distinct available server and client CPUs")
    return result


def ensure_fds(count):
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    if soft == resource.RLIM_INFINITY or soft >= count:
        return
    if hard != resource.RLIM_INFINITY and hard < count:
        raise RuntimeError(f"test needs at least {count} descriptors, hard limit is {hard}")
    resource.setrlimit(resource.RLIMIT_NOFILE, (count, hard))


@contextmanager
def server(args, state):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    env = {key: value for key, value in os.environ.items() if key not in CLEAR_ENV}
    env.update(DOCUMENT_ROOT=str(state), LISTEN=f"127.0.0.1:{port}", TOKIO_WORKER_THREADS="1")
    command = [str(args.japka.resolve())]
    with subprocess.Popen(["taskset", "-c", str(args.server_cpu), *command],
                          cwd=state, env=env, stderr=subprocess.PIPE) as process:
        try:
            deadline = time.monotonic() + 5
            while True:
                if process.poll() is not None:
                    raise RuntimeError(process.stderr.read().decode())
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                        break
                except ConnectionRefusedError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.01)
            yield process, port
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()


async def fetch(reader, writer):
    writer.write(REQUEST)
    await writer.drain()
    response = (await reader.readuntil(b"\r\n\r\n")).split(b"\r\n")
    if response[0].split()[1] != b"200":
        raise AssertionError(response[0])
    headers = dict((k.lower(), v.strip()) for k, v in
                   (line.split(b":", 1) for line in response[1:] if line))
    if headers.get(b"connection", b"").lower() == b"close":
        raise AssertionError("server sent Connection: close")
    size = int(headers[b"content-length"])
    if size != len(FIXTURE) or await reader.readexactly(size) != FIXTURE:
        raise AssertionError("static fixture did not match")


def process_metrics(pid):
    proc = Path(f"/proc/{pid}")
    stat = (proc / "stat").read_text().rsplit(")", 1)[1].split()
    cpu_ticks = sum(int(stat[index]) for index in (11, 12))
    rss_kib = next(int(line.split()[1]) for line in
                   (proc / "smaps_rollup").read_text().splitlines() if line.startswith("Rss:"))
    socket_fds = sum(
        os.readlink(fd).startswith("socket:") for fd in (proc / "fd").iterdir()
    )
    return cpu_ticks, rss_kib, socket_fds


async def exercise(process, port, args):
    clients = []

    async def open_and_fetch():
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        clients.append((reader, writer))
        await fetch(reader, writer)

    # Give the front end time to process EOF on the readiness probe.
    await asyncio.sleep(0.05)
    baseline_sockets = process_metrics(process.pid)[2]
    try:
        for start in range(0, args.connections, args.batch):
            count = min(args.batch, args.connections - start)
            async with asyncio.timeout(10):
                async with asyncio.TaskGroup() as tasks:
                    for _ in range(count):
                        tasks.create_task(open_and_fetch())

        before_idle = time.monotonic()
        ticks_before, first_rss, first_sockets = process_metrics(process.pid)
        await asyncio.sleep(args.idle)
        ticks_after, last_rss, last_sockets = process_metrics(process.pid)
        seconds = time.monotonic() - before_idle
        sockets = min(first_sockets, last_sockets) - baseline_sockets
        if sockets < args.connections:
            raise AssertionError(f"server only held {sockets} additional socket descriptors")

        for start in range(0, args.connections, args.batch):
            async with asyncio.timeout(10):
                async with asyncio.TaskGroup() as tasks:
                    for reader, writer in clients[start:start + args.batch]:
                        tasks.create_task(fetch(reader, writer))

        cpu_ms = (ticks_after - ticks_before) * 1000 / os.sysconf("SC_CLK_TCK")
        print(f"{args.connections} persistent clients, {2 * args.connections} verified responses, "
              f"{sockets} additional socket FDs, {seconds:.2f} s idle, "
              f"RSS {first_rss / 1024:.2f}/{last_rss / 1024:.2f} MiB, "
              f"idle CPU {cpu_ms:.0f} ms ({cpu_ms / seconds / 10:.2f}% of one CPU)")
    finally:
        for _, writer in clients:
            writer.close()
        await asyncio.gather(*(writer.wait_closed() for _, writer in clients), return_exceptions=True)


def main():
    args = arguments()
    ensure_fds(args.connections + 512)
    os.sched_setaffinity(0, args.client_cpus)
    with tempfile.TemporaryDirectory(prefix="japka-connections-") as directory:
        state = Path(directory)
        (state / "payload.bin").write_bytes(FIXTURE)
        digest = hashlib.sha256(args.japka.read_bytes()).hexdigest()[:12]
        print(f"japka {digest}", flush=True)
        with server(args, state) as (process, port):
            asyncio.run(exercise(process, port, args))


if __name__ == "__main__":
    main()
