"""Run the Japka HTTP/1.1 load suite against a native binary or Apple container."""

import argparse
from contextlib import ExitStack
import hashlib
import http.client
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from uuid import uuid4


TARGET = Path(__file__).resolve().parent / "target"
LOOPBACK = "127.0.0.1"
WORKERS = {"TOKIO_WORKER_THREADS": "1", "JAPKA_BLOCKING_THREADS": "1"}
PATTERN = b"0123456789abcdef"
CASES = {
    "static-4k": ("/4096.bin", 4096),
    "proxy-4k": ("/api/4096.bin?bench=1", 4096),
    "static-256k": ("/262144.bin", 262144),
    "proxy-256k": ("/api/262144.bin?bench=1", 262144),
}


def fixture(size):
    return PATTERN * (size // len(PATTERN))


def write_fixtures(site):
    # The host temp directory is private. The mounted subtree must be readable
    # by the image's unprivileged user, regardless of the host umask.
    for folder in (site, site / "front", site / "origin", site / "origin/api"):
        folder.mkdir()
        folder.chmod(0o755)
    for size in (4096, 262144):
        for folder in (site / "front", site / "origin/api"):
            path = folder / f"{size}.bin"
            path.write_bytes(fixture(size))
            path.chmod(0o644)


def free_ports(count):
    with ExitStack() as reservations:
        ports = []
        for _ in range(count):
            sock = reservations.enter_context(socket.socket())
            sock.bind((LOOPBACK, 0))
            ports.append(sock.getsockname()[1])
    return ports


def stop_process(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def start_native(running, binary, site):
    frontend, origin = free_ports(2)
    processes = []
    for folder, port, routing in (
        ("origin", origin, {}),
        ("front", frontend, {"UPSTREAMS": f"{LOOPBACK}:{origin}"}),
    ):
        server = subprocess.Popen(
            [binary], cwd=site,
            env=dict(WORKERS, LISTEN=f"{LOOPBACK}:{port}",
                     DOCUMENT_ROOT=str(site / folder), **routing),
        )
        running.callback(stop_process, server)
        processes.append((folder, server))
    return frontend, processes


def remove_container(container, name):
    result = subprocess.run(
        [container, "delete", "--force", name], capture_output=True, text=True, timeout=15,
    )
    error = (result.stderr or result.stdout).strip()
    if result.returncode and f"container with id {name} not found" not in error.lower():
        raise RuntimeError(f"container cleanup for {name} failed: {error or result.returncode}")


def container_digest(container, name):
    try:
        result = subprocess.run(
            [container, "inspect", name], capture_output=True, check=True, timeout=5,
        )
        return json.loads(result.stdout)[0]["configuration"]["image"]["descriptor"]["digest"]
    except (subprocess.SubprocessError, ValueError, KeyError, IndexError, TypeError):
        return None


def start_container(running, container, image, site):
    port, = free_ports(1)
    name = f"japka-perf-{uuid4().hex}"
    # Copy the fixtures into guest storage so reads and the upstream hop stay in
    # the guest. Match both native processes' config and worker counts.
    startup = """
site=$(mktemp -d)
cp -R /fixtures/front /fixtures/origin "$site"
LISTEN=127.0.0.1:9000 DOCUMENT_ROOT="$site/origin" /usr/local/bin/japka &
LISTEN=0.0.0.0:8080 DOCUMENT_ROOT="$site/front" UPSTREAMS=127.0.0.1:9000 exec /usr/local/bin/japka
"""
    process = None

    def cleanup():
        try:
            remove_container(container, name)
        finally:
            if process is not None:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                    # Startup may have created the container after the first delete.
                    remove_container(container, name)

    running.callback(cleanup)
    process = subprocess.Popen([
        container, "run", "--rm", "--name", name, "--cpus", "2", "--memory", "1G",
        "--publish", f"{LOOPBACK}:{port}:8080", "--volume", f"{site}:/fixtures:ro",
        "--entrypoint", "/usr/bin/env", image, "-i", "PATH=/usr/bin:/bin",
        *(f"{key}={value}" for key, value in WORKERS.items()), "/bin/sh", "-eu", "-c", startup,
    ])
    return port, [("container", process)], name


def verify_routes(port, processes):
    deadline = time.monotonic() + 30
    for path, size in CASES.values():
        expected = fixture(size)
        while True:
            for name, process in processes:
                exit_code = process.poll()
                if exit_code is not None:
                    raise RuntimeError(f"{name} exited with status {exit_code} before readiness")
            connection = http.client.HTTPConnection(LOOPBACK, port, timeout=0.5)
            try:
                connection.request("GET", path)
                response = connection.getresponse()
                body = response.read(size + 1)
                if response.status == 200 and body == expected:
                    break
                error = f"HTTP {response.status}, fixture {'matched' if body == expected else 'mismatch'}"
            except (OSError, http.client.HTTPException) as exc:
                error = str(exc)
            finally:
                connection.close()
            if time.monotonic() >= deadline:
                raise RuntimeError(f"GET {path} did not return HTTP 200 and the expected body ({error})")
            time.sleep(0.1)


def executable(parser, path):
    path = path.resolve()
    if not path.is_file() or not os.access(path, os.X_OK):
        parser.error(f"expected an executable at {path}")
    return path


def file_sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_case(args, case, port, output):
    env = {"JAPKA_PERF_CASE": case}
    users = str(1 if args.smoke else args.users)
    command = [
        args.client, "--host", f"http://{LOOPBACK}:{port}", "--users", users,
        "--increase-rate", users, "--timeout", "2", "--no-telnet", "--quiet", "--no-print-metrics",
    ]
    if not args.smoke and args.warmup:
        print(f"Warming up {case} for {args.warmup}s", flush=True)
        subprocess.run(
            [*command, "--run-time", f"{args.warmup}s"],
            env=env, check=True, timeout=args.warmup + 30,
        )
    print(f"Running {case}", flush=True)
    if args.smoke:
        duration = ["--iterations", "10"]
        timeout = 30
    else:
        duration = ["--run-time", f"{args.seconds}s"]
        timeout = args.seconds + 30
    report = output / f"{case}.json"
    summary = output / f"{case}.summary.json"
    subprocess.run(
        [*command, *duration, "--report-file", str(report)],
        env=dict(env, JAPKA_PERF_RESULT=str(summary)), check=True, timeout=timeout,
    )
    with report.open() as source:
        ended_by = json.load(source)["ended_by"]
    if ended_by != ("users_exited" if args.smoke else "completed"):
        raise RuntimeError(f"{case} ended early ({ended_by})")
    with summary.open() as source:
        validated = json.load(source)["validated_requests"]
    if args.smoke and validated != 10:
        raise RuntimeError(f"{case} validated {validated} responses, expected 10")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--japka", type=Path, help="path to the native Japka executable")
    target.add_argument("--image", help="Japka image for Apple container")
    parser.add_argument("--client", type=Path, default=TARGET / "release/japka-perf")
    parser.add_argument("--case", action="append", choices=CASES, help="repeat to run selected cases")
    parser.add_argument("--smoke", action="store_true", help="run ten requests per case with one user")
    parser.add_argument("--users", type=int, help="concurrent users per case (default: 64)")
    parser.add_argument("--warmup", type=int, help="warmup seconds per case (default: 2, 0 skips)")
    parser.add_argument("--seconds", type=int, help="measured seconds per case (default: 10)")
    parser.add_argument("--output", type=Path, help="new directory for the JSON reports")
    args = parser.parse_args()
    args.client = executable(parser, args.client)
    if args.japka:
        args.japka = executable(parser, args.japka)
    elif not (container := shutil.which("container")):
        parser.error("--image needs Apple's container command")
    if args.smoke and any(value is not None for value in (args.users, args.warmup, args.seconds)):
        parser.error("--smoke runs with fixed settings and cannot be combined with --users, --warmup, or --seconds")
    args.users = 64 if args.users is None else args.users
    args.warmup = 2 if args.warmup is None else args.warmup
    args.seconds = 10 if args.seconds is None else args.seconds
    if args.users < 1 or args.seconds < 1 or args.warmup < 0:
        parser.error("--users and --seconds must be positive and --warmup cannot be negative")

    if args.output is None:
        results = TARGET / "results"
        results.mkdir(parents=True, exist_ok=True)
        output = Path(tempfile.mkdtemp(prefix="run-", dir=results))
    else:
        output = args.output.absolute()
        output.mkdir(parents=True)
    print(f"Reports: {output}", flush=True)

    cases = list(dict.fromkeys(args.case or CASES))
    client_sha256 = file_sha256(args.client)
    with tempfile.TemporaryDirectory(prefix="japka-perf-") as directory, ExitStack() as running:
        site = Path(directory).resolve() / "fixtures"
        write_fixtures(site)
        if args.japka:
            server = {"name": args.japka.name, "sha256": file_sha256(args.japka)}
            port, processes = start_native(running, args.japka, site)
        else:
            port, processes, name = start_container(running, container, args.image, site)
        verify_routes(port, processes)
        if args.image:
            server = {"image": args.image, "digest": container_digest(container, name)}
        for case in cases:
            run_case(args, case, port, output)

    if file_sha256(args.client) != client_sha256:
        raise RuntimeError("load client changed during the run; the reports are incomplete")
    run = {
        "platform": "native" if args.japka else "apple-container",
        "smoke": args.smoke,
        "users": 1 if args.smoke else args.users,
        "seconds": None if args.smoke else args.seconds,
        "warmup": 0 if args.smoke else args.warmup,
        "cases": cases,
        "client_sha256": client_sha256,
        "server": server,
    }
    with (output / "run.json").open("x", encoding="utf-8") as manifest:
        json.dump(run, manifest, indent=2)
        manifest.write("\n")


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    try:
        main()
    except KeyboardInterrupt:
        raise SystemExit(130)
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        sys.exit(f"japka-perf: {error}")
