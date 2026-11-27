#!/usr/bin/env python3
"""Measure Rill HTTP/1.1 throughput, latency and memory on loopback."""
import argparse
from contextlib import ExitStack, contextmanager
from http.client import HTTPConnection
import os
from pathlib import Path
import re
import shutil
import socket
import statistics
import sys
import subprocess
import tempfile
import time

REPO = Path(__file__).resolve().parent.parent
PATTERN = b"0123456789abcdef"
BODY = PATTERN * 256
ROUTES = {"static": "/payload.bin", "proxy": "/api/payload.bin?bench=1"}
LUA_CHECKS = """
local threads = {}
validated = 0
invalid = 0

function setup(thread)
    table.insert(threads, thread)
end

function response(status, headers, body)
    validated = validated + 1
    if status ~= 200 or body ~= expected then
        invalid = invalid + 1
    end
end

function done(summary, latency, requests)
    local checked, wrong, errors = 0, 0, 0
    for _, thread in ipairs(threads) do
        checked = checked + thread:get("validated")
        wrong = wrong + thread:get("invalid")
    end
    for _, count in pairs(summary.errors) do
        errors = errors + count
    end
    print(string.format("RESULT %d %d %d %d %.2f %.0f %.3f",
        summary.requests, checked, wrong, errors,
        summary.requests * 1000000 / summary.duration, latency:percentile(95),
        summary.duration / 1000000))
end
"""


def validation_script(size):
    repeats, tail = divmod(size, len(PATTERN))
    expected = f'string.rep("{PATTERN.decode()}", {repeats}) .. "{PATTERN[:tail].decode()}"'
    return f"local expected = {expected}\n" + LUA_CHECKS


LUA = validation_script(len(BODY))

# Build request strings before the measured loop. The 64 fixtures have identical
# bytes but distinct paths and inodes, so the same validator applies to all.
ROTATE_FILES = """
local requests = {}
local cursor = 0
function init(args)
    for i = 1, 64 do
        requests[i] = wrk.format("GET", "/files/" .. i .. file_suffix)
    end
end
function request()
    cursor = cursor % #requests + 1
    return requests[cursor]
end
"""


class DurationOverrun(RuntimeError):
    pass


@contextmanager
def process(command, **kwargs):
    proc = subprocess.Popen(command, **kwargs)
    try:
        yield proc
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def check(port, route, proc, expected=BODY):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"server on {port} exited ({proc.returncode})")
        conn = HTTPConnection("127.0.0.1", port, timeout=1)
        try:
            conn.request("GET", route)
            response = conn.getresponse()
            if response.status != 200 or response.read() != expected:
                raise RuntimeError(f"unexpected response from {port}{route}: {response.status}")
            return
        except ConnectionRefusedError:
            time.sleep(0.05)
        finally:
            conn.close()
    raise RuntimeError(f"server on {port} failed to start")


def rss_kib(pid):
    with open(f"/proc/{pid}/smaps_rollup") as stats:
        return next(int(line.split()[1]) for line in stats if line.startswith("Rss:"))


def run_sample(args, script, port, route, proc, seconds):
    command = [
        "taskset", "-c", ",".join(map(str, args.client_cpus)), args.wrk,
        "-t", str(len(args.client_cpus)), "-c", str(args.connections),
        "-d", f"{seconds}s", "--timeout", "2s", "-s", str(script),
        f"http://127.0.0.1:{port}{route}",
    ]
    rss = 0
    with process(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as client:
        deadline = time.monotonic() + seconds + 10
        while client.poll() is None:
            if time.monotonic() >= deadline:
                raise RuntimeError("wrk exceeded its run deadline")
            if proc.poll() is not None:
                raise RuntimeError(f"server on {port} exited ({proc.returncode})")
            rss = max(rss, rss_kib(proc.pid))
            time.sleep(0.2)
        output, _ = client.communicate()
    match = re.search(r"^RESULT (\d+) (\d+) (\d+) (\d+) ([\d.]+) (\d+) ([\d.]+)$", output, re.M)
    if client.returncode or not match:
        raise RuntimeError(f"wrk failed:\n{output}")
    total, checked, wrong, errors = map(int, match.groups()[:4])
    if not rss or not total or total != checked or wrong:
        raise RuntimeError(f"invalid benchmark sample:\n{output}")
    rate, p95_us, elapsed = map(float, match.groups()[4:])
    if elapsed > seconds + 1:
        raise DurationOverrun(f"wrk ran for {elapsed:.3f}s (requested {seconds}s), "
                              f"with {errors} transport errors")
    if errors:
        raise RuntimeError(f"invalid benchmark sample:\n{output}")
    return rate, p95_us / 1000, rss / 1024, total


def measure(args, script, workload, route, name, port, proc):
    for attempt in range(1, 4):
        try:
            run_sample(args, script, port, route, proc, args.warmup)
            return run_sample(args, script, port, route, proc, args.seconds)
        except DurationOverrun as error:
            print(f"Rejected {workload} {name} attempt {attempt}: {error}", file=sys.stderr)
            if attempt == 3:
                raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rill", default=os.getenv("RILL", REPO / "target/release/rill"))
    comparison = parser.add_mutually_exclusive_group()
    comparison.add_argument("--baseline-rill", help="optional earlier Rill binary to benchmark alongside this release")
    comparison.add_argument("--asset-cache-ab", action="store_true", help="compare the Rill asset cache off and on")
    comparison.add_argument("--sendfile-ab", action="store_true", help="compare Rill buffered and direct file delivery")
    parser.add_argument("--workload", choices=ROUTES,
                        help="run one workload in fresh frontends (default: static followed by proxy)")
    parser.add_argument("--wrk", default=os.getenv("WRK", "wrk"))
    parser.add_argument("--static-layout", choices=["file", "index", "many"], default="file",
                        help="serve a named file, a directory index, or 64 rotating files")
    parser.add_argument("--server-cpu", type=int, required=True)
    parser.add_argument("--backend-cpu", type=int, required=True)
    parser.add_argument("--client-cpus", type=int, nargs="+", required=True)
    parser.add_argument("--payload-bytes", type=int, default=len(BODY),
                        help="validated body size in bytes (default: 4096)")
    parser.add_argument("--seconds", type=int, default=5)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--connections", type=int, default=64)
    args = parser.parse_args()
    cpus = [args.server_cpu, args.backend_cpu, *args.client_cpus]
    if len(cpus) != len(set(cpus)) or not set(cpus) <= os.sched_getaffinity(0):
        parser.error("choose distinct CPUs permitted by this process's affinity")
    if min(args.seconds, args.warmup, args.repeat) < 1:
        parser.error("seconds, warmup, and repeat must all be positive")
    if args.connections < 1 or args.connections > 256 or args.connections % len(args.client_cpus):
        parser.error("connections must be a positive multiple of client CPU count, at most 256")
    if args.payload_bytes < 0:
        parser.error("payload size must be nonnegative")
    if args.asset_cache_ab or args.sendfile_ab:
        if args.workload == "proxy" or args.static_layout == "index":
            parser.error("cache and sendfile comparisons require static named files (file or many layout)")
        args.workload = "static"
    binaries = ["wrk", "rill"]
    if args.baseline_rill:
        binaries.append("baseline_rill")
    for binary in binaries:
        path = shutil.which(getattr(args, binary))
        if not path:
            parser.error(f"{binary} binary missing (set --{binary.replace('_', '-')})")
        setattr(args, binary, str(Path(path).resolve()))

    with tempfile.TemporaryDirectory(prefix="rill-bench-") as temp, ExitStack() as running:
        root = Path(temp)
        (root / "api").mkdir()
        repeats, tail = divmod(args.payload_bytes, len(PATTERN))
        body = PATTERN * repeats + PATTERN[:tail]
        file_suffix = ".8f31.bin" if args.asset_cache_ab else ".bin"
        static_route = f"/payload{file_suffix}"
        (root / static_route[1:]).write_bytes(body)
        (root / "api/payload.bin").write_bytes(body)
        script = root / "validate.lua"
        script.write_text(validation_script(len(body)))
        static_script = script
        if args.static_layout == "index":
            (root / "index.html").write_bytes(body)
            static_route = "/"
        elif args.static_layout == "many":
            (root / "files").mkdir()
            for i in range(1, 65):
                (root / f"files/{i}{file_suffix}").write_bytes(body)
            static_route = f"/files/1{file_suffix}"
            static_script = root / "rotate.lua"
            static_script.write_text(validation_script(len(body))
                                     + f'local file_suffix = "{file_suffix}"\n' + ROTATE_FILES)
        def start_rill(binary, cpu, port, upstreams=None, **settings):
            env = dict(os.environ, TOKIO_WORKER_THREADS="1", DOCUMENT_ROOT=temp,
                       LISTEN=f"127.0.0.1:{port}", **settings)
            env.pop("RILL_ROUTES", None)
            env.pop("UPSTREAMS", None)
            if upstreams:
                env["UPSTREAMS"] = upstreams
            return running.enter_context(process(["taskset", "-c", str(cpu), binary], env=env))

        with ExitStack() as reserved:
            sockets = [reserved.enter_context(socket.socket()) for _ in range(3)]
            for listener in sockets:
                listener.bind(("127.0.0.1", 0))
            backend_port, port, other_port = [s.getsockname()[1] for s in sockets]

        upstream = f"127.0.0.1:{backend_port}"
        if args.workload in (None, "proxy"):
            backend = start_rill(args.rill, args.backend_cpu, backend_port)
            check(backend_port, ROUTES["proxy"], backend, body)
        if args.asset_cache_ab:
            variants = [("rill-uncached", args.rill, {"RILL_ASSET_CACHE_ENTRIES": "0"}),
                        ("rill-cached", args.rill, {"RILL_ASSET_CACHE_ENTRIES": "128"})]
        elif args.sendfile_ab:
            variants = [("rill-buffered", args.rill, {"RILL_SENDFILE": "0"}),
                        ("rill-sendfile", args.rill, {"RILL_SENDFILE": "1"})]
        else:
            variants = [("rill", args.rill, {})]
            if args.baseline_rill:
                variants.append(("rill-baseline", args.baseline_rill, {}))
        servers = [(name, p, start_rill(binary, args.server_cpu, p, upstream, **settings))
                   for (name, binary, settings), p in zip(variants, (port, other_port))]
        print(f"server CPU {args.server_cpu}, backend CPU {args.backend_cpu}, "
              f"client CPUs {args.client_cpus}, {args.connections} connections, "
              f"{args.warmup}s warmup + {args.seconds}s measured", flush=True)
        print("workload server run requests/s p95_ms peak_rss_mib verified_responses", flush=True)
        print(f"payload {len(body)} bytes, static layout {args.static_layout}", flush=True)
        if args.asset_cache_ab:
            print(f"fingerprinted fixture {static_route}, Rill asset cache 128 entries", flush=True)
        for workload, route in ROUTES.items():
            if args.workload and workload != args.workload:
                continue
            request_script = static_script if workload == "static" else script
            if workload == "static":
                route = static_route
            results = {name: [] for name, _, _ in servers}
            for name, port, proc in servers:
                check(port, route, proc, body)
            for index in range(args.repeat):
                offset = index % len(servers)
                order = servers[offset:] + servers[:offset]
                for name, port, proc in order:
                    rate, p95, rss, total = measure(args, request_script, workload, route, name, port, proc)
                    results[name].append((rate, p95, rss))
                    print(f"{workload} {name} {index + 1} {rate:.2f} {p95:.3f} "
                          f"{rss:.2f} {total}", flush=True)
            for name, rows in results.items():
                rate, p95, rss = map(statistics.median, zip(*rows))
                print(f"{workload} {name} median {rate:.2f} {p95:.3f} {rss:.2f} -", flush=True)


if __name__ == "__main__":
    main()
