"""Compare Japka and NGINX on macOS and in Apple containers from the same Mac."""
import argparse
from contextlib import ExitStack, contextmanager
import csv
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from uuid import uuid4

REPO = Path(__file__).resolve().parent.parent
PATTERN = b"0123456789abcdef"
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


def wait_for_port(port, seconds=10):
    deadline = time.monotonic() + seconds
    while True:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
            return
        except OSError:
            if time.monotonic() > deadline:
                raise
            time.sleep(0.05)


def nginx_config(listen, upstream, document_root):
    return f"""
daemon off;
master_process off;
worker_processes 1;
pid nginx.pid;
error_log stderr warn;
events {{ worker_connections 1024; }}
http {{
    access_log off;
    sendfile on;
    tcp_nodelay on;
    client_body_temp_path client_temp;
    proxy_temp_path proxy_temp;
    fastcgi_temp_path fastcgi_temp;
    uwsgi_temp_path uwsgi_temp;
    scgi_temp_path scgi_temp;
    upstream origin {{ server {upstream}; keepalive 128; }}
    server {{
        listen {listen};
        root "{document_root}";
        location /api/ {{
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_pass http://origin;
        }}
    }}
}}
"""


def start_container(running, site, image, port, frontend):
    name = f"japka-bench-{uuid4().hex[:10]}"
    # Read warm guest files and proxy over guest loopback for both servers.
    startup = """
set -eu
cp -R /fixtures /tmp/site
LISTEN=127.0.0.1:9000 DOCUMENT_ROOT=/tmp/site/origin japka &
"""
    if frontend == "nginx":
        startup += 'exec nginx -p /tmp/ -c /tmp/site/nginx.conf -e stderr'
    else:
        startup += 'export UPSTREAMS=127.0.0.1:9000 DOCUMENT_ROOT=/tmp/site/front; '
        startup += 'exec japka'
    subprocess.run(
        ["container", "run", "-d", "--rm", "--name", name, "--cpus", "2", "--memory", "1G",
         "-p", f"127.0.0.1:{port}:8080", "-v", f"{site}:/fixtures:ro",
         "-e", "LISTEN=0.0.0.0:8080", "-e", "TOKIO_WORKER_THREADS=1", "-e", "JAPKA_BLOCKING_THREADS=1",
         "--entrypoint", "/bin/sh", image, "-c", startup],
        check=True, stdout=sys.stderr,
    )
    running.callback(subprocess.run, ["container", "stop", name], check=True, stdout=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--japka", type=Path, default=REPO / "target/release/japka")
    parser.add_argument("--image", default="japka-nginx:mac-bench")
    parser.add_argument("--wrk", default="wrk")
    parser.add_argument("--nginx", default="nginx", help="native NGINX executable")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--native-only", action="store_true", help="skip the Apple container guests")
    args = parser.parse_args()
    if args.rounds < 1:
        parser.error("--rounds must be positive")
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
            sockets = [reservations.enter_context(socket.socket()) for _ in range(5)]
            for sock in sockets:
                sock.bind(("127.0.0.1", 0))
            native, origin, published, native_nginx, published_nginx = [sock.getsockname()[1] for sock in sockets]

        binary = str(args.japka.resolve())
        workers = {"TOKIO_WORKER_THREADS": "1", "JAPKA_BLOCKING_THREADS": "1"}
        for port, folder, settings in [(origin, "origin", {}), (native, "front", {"UPSTREAMS": f"127.0.0.1:{origin}"})]:
            running.enter_context(process(
                [binary], env=dict(PATH=os.environ["PATH"], LISTEN=f"127.0.0.1:{port}",
                                   DOCUMENT_ROOT=str(site / folder), **workers, **settings),
            ))
        config = root / "nginx.conf"
        config.write_text(nginx_config(f"127.0.0.1:{native_nginx}", f"127.0.0.1:{origin}", site / "front"))
        running.enter_context(process([args.nginx, "-p", f"{root}/", "-c", str(config), "-e", "stderr"]))
        (site / "nginx.conf").write_text(nginx_config("0.0.0.0:8080", "127.0.0.1:9000", "/tmp/site/front"))
        servers = [("japka-native", native), ("nginx-native", native_nginx)]
        if not args.native_only:
            start_container(running, site, args.image, published, "japka")
            start_container(running, site, args.image, published_nginx, "nginx")
            servers += [("japka-container", published), ("nginx-container", published_nginx)]
        for _, port in servers:
            wait_for_port(port)
        writer = csv.writer(sys.stdout, lineterminator="\n")
        writer.writerow(["workload", "bytes", "server", "round", "requests_per_s", "p95_ms", "checked"])
        for size in sizes:
            script = root / f"{size}.lua"
            for workload, path in [("static", f"/{size}.bin"), ("proxy", f"/api/{size}.bin?bench=1")]:
                for round_number in range(1, args.rounds + 1):
                    order = servers if round_number % 2 else servers[::-1]
                    for server, port in order:
                        sample(args.wrk, script, port, path, 2)
                        row = sample(args.wrk, script, port, path, 5)
                        writer.writerow([workload, size, server, round_number, *row])
                        sys.stdout.flush()


if __name__ == "__main__":
    main()
