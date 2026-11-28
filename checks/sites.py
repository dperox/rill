"""Exercise virtual-host roots, reload rollback and JSON access records on real sockets."""
import json
import os
import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory

from reload import free_port, listening, publish, reload, stop, wait_until
from routing import JAPKA, fetch
from http.client import HTTPConnection


def check(work, sendfile):
    front = free_port()
    config = work / "config.json"
    logs = work / "stderr"
    asset = "app.abcdef123456.js"
    for label in ("one", "two", "default"):
        root = work / label
        root.mkdir(exist_ok=True)
        (root / asset).write_text(label)
    dead_peer = f"127.0.0.1:{free_port()}"
    candidate = {
        "sites": [{"host": "one.test", "root": "one"}, {"host": "two.test", "root": "two"}],
        "routes": [{"host": "one.test", "prefix": "/api/", "upstreams": [dead_peer]}],
    }
    publish(config, candidate)
    env = dict(os.environ, LISTEN=f"127.0.0.1:{front}", JAPKA_CONFIG=str(config),
               DOCUMENT_ROOT=str(work / "default"), JAPKA_ACCESS_LOG="1",
               JAPKA_SENDFILE=sendfile, JAPKA_ASSET_CACHE_ENTRIES="8")
    with logs.open("w") as log:
        proc = subprocess.Popen([JAPKA], env=env, stderr=log)
        try:
            wait_until(proc, logs, "listening", lambda: listening(front))
            for host, expected in (("ONE.test:123", "one"), ("two.test", "two"),
                                   ("unknown.test", "default"), ("one.test", "one")):
                assert fetch(front, "/" + asset, host) == expected
            conn = HTTPConnection("127.0.0.1", front, timeout=3)
            try:
                conn.request("GET", "/api/test?secret=hidden", headers={"Host": "one.test"})
                assert conn.getresponse().status == 502
            finally:
                conn.close()
            candidate["sites"][0]["root"] = "missing"
            publish(config, candidate)
            reload(proc, logs, b"reload rejected")
            assert fetch(front, "/" + asset, "one.test") == "one"
            candidate["sites"][0]["root"] = "two"
            publish(config, candidate)
            reload(proc, logs, b"configuration reloaded")
            assert fetch(front, "/" + asset, "one.test") == "two"
        finally:
            stop(proc)
    records = [json.loads(line) for line in logs.read_text().splitlines() if line.startswith("{")]
    assert len(records) == 7, records
    record = next(r for r in records if r["status"] == 502)
    assert record["upstream"] == dead_peer and record["path"] == "/api/test"
    assert record["client_ip"] == "127.0.0.1" and record["host"] == "one.test"
    assert record["method"] == "GET" and record["header_latency_us"] >= 0
    assert all(r["upstream"] is None for r in records if r["status"] == 200)
    assert "secret=hidden" not in logs.read_text()


if __name__ == "__main__":
    with TemporaryDirectory() as temp:
        for mode in ("0", "1"):
            check(Path(temp), mode)
    print("Per-host roots, isolation, reload rollback, and access logs passed in both sendfile modes.")
