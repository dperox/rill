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
    name = f"rill-bench-{uuid4().hex[:10]}"
    # Read warm guest files and proxy over guest loopback for both servers.
    startup = """
set -eu
cp -R /fixtures /tmp/site
LISTEN=127.0.0.1:9000 DOCUMENT_ROOT=/tmp/site/origin rill &
"""
    if frontend == "nginx":
        startup += 'exec nginx -p /tmp/ -c /tmp/site/nginx.conf -e stderr'
    else:
        startup += 'export UPSTREAMS=127.0.0.1:9000 DOCUMENT_ROOT=/tmp/site/front; exec rill'
    subprocess.run(
        ["container", "run", "-d", "--rm", "--name", name, "--cpus", "2", "--memory", "1G",
         "-p", f"127.0.0.1:{port}:8080", "-v", f"{site}:/fixtures:ro",
         "-e", "LISTEN=0.0.0.0:8080", "-e", "TOKIO_WORKER_THREADS=1", "-e", "RILL_BLOCKING_THREADS=1",
         "--entrypoint", "/bin/sh", image, "-c", startup],
        check=True, stdout=sys.stderr,
    )
    running.callback(subprocess.run, ["container", "stop", name], check=True, stdout=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rill", type=Path, default=REPO / "target/release/rill")
    parser.add_argument("--image", default="rill:local")
    parser.add_argument("--wrk", default="wrk")
    parser.add_argument("--nginx", help="also benchmark this native NGINX binary; --image must contain nginx and rill")
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
            sockets = [reservations.enter_context(socket.socket()) for _ in range(5)]
            for sock in sockets:
                sock.bind(("127.0.0.1", 0))
            native, origin, published, native_nginx, published_nginx = [sock.getsockname()[1] for sock in sockets]

        workers = {"TOKIO_WORKER_THREADS": "1", "RILL_BLOCKING_THREADS": "1"}
        for port, folder, settings in [(origin, "origin", {}), (native, "front", {"UPSTREAMS": f"127.0.0.1:{origin}"})]:
            running.enter_context(process(
                [str(args.rill.resolve())],
                env=dict(PATH=os.environ["PATH"], LISTEN=f"127.0.0.1:{port}",
                         DOCUMENT_ROOT=str(site / folder), **workers, **settings),
            ))

        start_container(running, site, args.image, published, "rill")
        servers = [("native", native), ("apple-container", published)]
        if args.nginx:
            config = root / "nginx.conf"
            config.write_text(nginx_config(f"127.0.0.1:{native_nginx}", f"127.0.0.1:{origin}", site / "front"))
            running.enter_context(process([args.nginx, "-p", f"{root}/", "-c", str(config), "-e", "stderr"]))
            (site / "nginx.conf").write_text(nginx_config("0.0.0.0:8080", "127.0.0.1:9000", "/tmp/site/front"))
            start_container(running, site, args.image, published_nginx, "nginx")
            servers = [("rill-native", native), ("nginx-native", native_nginx),
                       ("rill-container", published), ("nginx-container", published_nginx)]
        writer = csv.writer(sys.stdout, lineterminator="\n")
        writer.writerow(["workload", "bytes", "server", "round", "requests_per_s", "p95_ms", "checked"])
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
