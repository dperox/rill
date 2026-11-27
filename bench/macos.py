"""Benchmark native Rill and Apple's container runtime from the same Mac."""
import argparse
from contextlib import ExitStack
import csv
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
from uuid import uuid4

from compare import PATTERN, REPO, process, validation_script


def sample(wrk, script, port, path, seconds):
    result = subprocess.run(
        [wrk, "-t2", "-c64", f"-d{seconds}s", "--timeout", "2s", "-s", str(script),
         f"http://127.0.0.1:{port}{path}"],
        check=True, capture_output=True, text=True, timeout=seconds + 15,
    )
    line = next((line for line in result.stdout.splitlines() if line.startswith("RESULT ")), "")
    if not line:
        raise RuntimeError(f"missing wrk validation results: {result.stdout} {result.stderr}")
    _, requests, checked, wrong, errors, rate, p95_us, elapsed = line.split()
    if int(checked) == 0 or requests != checked or int(wrong) or int(errors) or float(elapsed) > seconds + 1:
        raise RuntimeError(f"invalid wrk sample: {line}")
    return rate, f"{float(p95_us) / 1000:.3f}", checked


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rill", type=Path, default=REPO / "target/release/rill")
    parser.add_argument("--image", default="rill:local")
    parser.add_argument("--wrk", default="wrk")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as directory, ExitStack() as running:
        root = Path(directory)
        site = root / "site"
        (site / "front").mkdir(parents=True)
        (site / "origin/api").mkdir(parents=True)
        sizes = (4096, 262144)
        for size in sizes:
            body = PATTERN * (size // len(PATTERN))
            for folder in (site / "front", site / "origin/api"):
                (folder / f"{size}.bin").write_bytes(body)
            (root / f"{size}.lua").write_text(validation_script(size))

        with ExitStack() as reservations:
            sockets = [reservations.enter_context(socket.socket()) for _ in range(3)]
            for sock in sockets:
                sock.bind(("127.0.0.1", 0))
            native, origin, published = [sock.getsockname()[1] for sock in sockets]

        workers = {"TOKIO_WORKER_THREADS": "1", "RILL_BLOCKING_THREADS": "1"}
        for port, folder, settings in [(origin, "origin", {}), (native, "front", {"UPSTREAMS": f"127.0.0.1:{origin}"})]:
            running.enter_context(process(
                [str(args.rill.resolve())],
                env=dict(PATH=os.environ["PATH"], LISTEN=f"127.0.0.1:{port}",
                         DOCUMENT_ROOT=str(site / folder), **workers, **settings),
            ))

        name = f"rill-bench-{uuid4().hex[:10]}"
        # Keep the measured files and the backend inside the guest during the run.
        startup = """
set -eu
cp -R /fixtures /tmp/site
LISTEN=127.0.0.1:9000 DOCUMENT_ROOT=/tmp/site/origin rill &
export UPSTREAMS=127.0.0.1:9000 DOCUMENT_ROOT=/tmp/site/front
exec rill
"""
        subprocess.run(
            ["container", "run", "-d", "--rm", "--name", name, "--cpus", "2", "--memory", "1G",
             "-p", f"127.0.0.1:{published}:8080", "-v", f"{site}:/fixtures:ro",
             "-e", "TOKIO_WORKER_THREADS=1", "-e", "RILL_BLOCKING_THREADS=1",
             "--entrypoint", "/bin/sh", args.image, "-c", startup],
            check=True, stdout=sys.stderr,
        )
        running.callback(subprocess.run, ["container", "stop", name], check=True, stdout=sys.stderr)
        writer = csv.writer(sys.stdout, lineterminator="\n")
        writer.writerow(["workload", "bytes", "server", "round", "requests_per_s", "p95_ms", "checked"])
        servers = [("native", native), ("apple-container", published)]
        for size in sizes:
            script = root / f"{size}.lua"
            for workload, path in [("static", f"/{size}.bin"), ("proxy", f"/api/{size}.bin?bench=1")]:
                for round_number in range(1, 4):
                    for server, port in (servers if round_number % 2 else servers[::-1]):
                        sample(args.wrk, script, port, path, 2)
                        row = sample(args.wrk, script, port, path, 5)
                        writer.writerow([workload, size, server, round_number, *row])
                        sys.stdout.flush()


if __name__ == "__main__":
    main()
