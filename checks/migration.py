"""Verify the versioned migration and configuration-test CLI contracts."""
import json
import os
import socket
import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory

from routing import RILL


def run(args, expected, *, text=None, env=None):
    result = subprocess.run([RILL, *args], input=text, text=True, capture_output=True,
                            env=env, timeout=10)
    assert result.returncode == expected, (result.returncode, result.stdout, result.stderr)
    report = json.loads(result.stdout)
    assert report["schema_version"] == 1
    return report


def main():
    with TemporaryDirectory() as tmp, socket.socket() as listener:
        root = Path(tmp)
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        port = listener.getsockname()[1]
        dump = f"""# configuration file /etc/nginx/nginx.conf:
events {{}}
http {{ include conf.d/*.conf; }}
# configuration file /etc/nginx/conf.d/app.conf:
upstream app {{ server 127.0.0.1:9001; server 127.0.0.1:9002; }}
server {{
  listen 127.0.0.1:{port};
  server_name app.example;
  root "{root}";
  location /api/ {{ proxy_pass http://app; auth_request /login; }}
  location /strip/ {{ proxy_pass http://app/; }}
}}
"""
        file = root / "dump"
        file.write_text(dump)
        args = ["migrate", "nginx"]
        report = run(args, 2, text=dump)
        assert report == run([*args, str(file)], 2)
        assert report["status"] == "review_required"
        candidate = report["candidate"]
        assert candidate["config"]["routes"] == [
            {"host": "app.example", "prefix": "/api/",
             "upstreams": ["http://127.0.0.1:9001", "http://127.0.0.1:9002"]}]
        auth = next(d for d in report["diagnostics"] if d["code"] == "authentication_required")
        assert any(d["code"] == "proxy_uri_replacement" for d in report["diagnostics"])
        assert (auth["file"], auth["line"]) == ("/etc/nginx/conf.d/app.conf", 6)
        assert "/api/" in auth["context"]
        invalid = run(args, 1, text="http {")
        assert invalid["diagnostics"][0]["code"] == "invalid_syntax"
        assert run(args, 2, text="include /missing;")["diagnostics"][0]["code"] == "unresolved_include"
        path = root / "rill.json"
        path.write_text(json.dumps(candidate["config"]))
        env = dict(os.environ, DOCUMENT_ROOT=tmp, RILL_CONFIG=str(path), **candidate["environment"])
        assert run(["config", "test"], 0, env=env)["status"] == "valid"
        assert run(["config", "test"], 1, env=dict(env, RILL_STREAM_BALANCE="bad"))["status"] == "invalid"
        assert run(["config", "test"], 1, env=dict(env, RILL_FIXED_UPSTREAM="bad"))["status"] == "invalid"
        path.write_text('{"unknown":1}')
        assert run(["config", "test"], 1, env=env)["status"] == "invalid"
        print("Migration source locations, candidate, exit codes and preflight on an occupied port passed.")


if __name__ == "__main__":
    main()
