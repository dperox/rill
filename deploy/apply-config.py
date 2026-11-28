#!/usr/bin/env python3
"""Apply a reviewed reload plan to a single Unix service. Run from a trusted deploy job."""
import argparse
import fcntl
import hashlib
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path


def sha(data):
    return hashlib.sha256(data).hexdigest()


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


@contextmanager
def staged(target, data):
    # Stage in the target directory so relative paths and atomic rename match service behavior.
    with tempfile.NamedTemporaryFile(dir=target.parent, prefix=".japka-", delete=False) as f:
        path = Path(f.name)
    try:
        info = target.stat()
        with path.open("wb") as f:
            f.write(data)
            f.flush()
            staged_info = os.fstat(f.fileno())
            if (info.st_uid, info.st_gid) != (staged_info.st_uid, staged_info.st_gid):
                os.fchown(f.fileno(), info.st_uid, info.st_gid)
            shutil.copystat(target, path)
            os.fsync(f.fileno())
        yield path
    finally:
        path.unlink(missing_ok=True)


def last_active(log, identity):
    last = None
    while True:
        offset = log.tell()
        line = log.readline()
        if not line.endswith(b"\n"):
            log.seek(offset)
            break
        if line.startswith(b"Japka configuration active "):
            record = json.loads(line[len(b"Japka configuration active "):])
            if record.get("pid") == identity["pid"]:
                require(all(record.get(k) == v for k, v in identity.items()),
                        "Service target, binary version, working directory or environment differs from plan")
                last = record["file_sha256"]
    return last


def reload_and_wait(log, identity, digest):
    log.seek(0, os.SEEK_END)
    os.kill(identity["pid"], signal.SIGHUP)
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if last_active(log, identity) == digest:
            return
        time.sleep(0.05)
    raise RuntimeError("Service did not acknowledge the expected configuration within 10 seconds")


def restore(target, backup, log, identity, plan):
    require(sha(backup.read_bytes()) == plan["baseline_sha256"], "Rollback file differs from reviewed baseline")
    require(sha(target.read_bytes()) in (plan["baseline_sha256"], plan["candidate_sha256"]),
            "Target was changed by another writer; rollback file retained")
    with staged(target, backup.read_bytes()) as original:
        os.replace(original, target)
    reload_and_wait(log, identity, plan["baseline_sha256"])
    backup.unlink()


def apply(args):
    require(args.pid > 1, "Supply a single service PID greater than 1")
    require(args.verify, "Supply the trusted health check after --verify")
    reviewed = args.approved_plan.read_bytes()
    require(sha(reviewed) == args.approved_sha256, "Plan differs from the approved digest")
    plan = json.loads(reviewed)
    require(plan.get("command") == "config plan" and plan.get("schema_version") == 1
            and plan.get("status") == "valid" and plan.get("action") in ("none", "reload"),
            "Expected a valid Unix file plan")
    target = Path(os.environ["JAPKA_CONFIG"]).absolute()
    require(str(target) == plan["target"] and stat.S_ISREG(target.lstat().st_mode),
            "JAPKA_CONFIG must be the reviewed regular file")
    identity = {k: plan[k] for k in ("target", "version", "working_directory", "environment_sha256")}
    identity["pid"] = args.pid
    backup = target.with_name("." + target.name + ".rollback")
    lock = target.with_name("." + target.name + ".lock")
    with lock.open("a") as guard, args.log.open("rb") as log:
        fcntl.flock(guard, fcntl.LOCK_EX)
        os.kill(args.pid, 0)
        active = last_active(log, identity)
        if backup.exists():
            require(active in (plan["baseline_sha256"], plan["candidate_sha256"]),
                    "Pending rollout has a different active generation")
            restore(target, backup, log, identity, plan)
            active = plan["baseline_sha256"]
        candidate = args.candidate.read_bytes()
        require(sha(candidate) == plan["candidate_sha256"], "Candidate changed after review")
        if plan["action"] == "reload" and active == plan["candidate_sha256"] and sha(target.read_bytes()) == active:
            subprocess.run(args.verify, check=True, timeout=30)
            return
        require(active == plan["baseline_sha256"], "Service is not serving the reviewed baseline")
        with staged(target, candidate) as path:
            result = subprocess.run([args.binary, "config", "plan", target, path], capture_output=True, timeout=60)
            require(result.returncode == 0 and json.loads(result.stdout) == plan,
                    "Deployment inputs changed; generate a new plan for review")
            if plan["action"] == "none":
                subprocess.run(args.verify, check=True, timeout=30)
                return
            os.link(target, backup)
            try:
                require(sha(backup.read_bytes()) == plan["baseline_sha256"], "Baseline changed after review")
                os.replace(path, target)
                reload_and_wait(log, identity, plan["candidate_sha256"])
                subprocess.run(args.verify, check=True, timeout=30)
            except BaseException:
                restore(target, backup, log, identity, plan)
                print("Restored the reviewed baseline", file=sys.stderr)
                raise
            backup.unlink()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="japka")
    parser.add_argument("--approved-plan", type=Path, required=True)
    parser.add_argument("--approved-sha256", required=True, help="plan digest from the trusted approval record")
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--verify", nargs=argparse.REMAINDER, required=True,
                        help="trusted health-check executable and arguments (last option)")
    try:
        apply(parser.parse_args())
        print("Verified the reviewed configuration")
    except (KeyError, ValueError, OSError, RuntimeError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
