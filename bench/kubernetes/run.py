#!/usr/bin/env python3
"""Measure validated HTTPS traffic through the Rill benchmark ingress."""

import argparse
import csv
import os
from pathlib import Path
import subprocess
import time

HERE = Path(__file__).resolve().parent
FIELDS = ("requests", "elapsed_s", "requests_per_s", "p95_ms", "p99_ms",
          "checked", "invalid", "static", "api", "download",
          "connect_errors", "read_errors", "write_errors", "timeouts", "status_errors")
PROFILES = (("static", 64), ("download", 64), ("mixed", 64), ("mixed", 256))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("state", type=Path, help="directory initialized by deploy.sh")
    parser.add_argument("--wrk", default="wrk")
    parser.add_argument("--cpus", default="16,18,20,22,24,26,28,30")
    parser.add_argument("--seconds", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--rollout", action="store_true")
    args = parser.parse_args()
    if min(args.seconds, args.warmup, args.rounds) < 1:
        parser.error("--seconds, --warmup, and --rounds must be positive")
    env = dict(os.environ, HOSTALIASES=str((args.state / "hosts").resolve()))
    threads = len(args.cpus.split(","))
    if args.rollout:
        context = subprocess.check_output(["kubectl", "config", "current-context"], text=True).strip()
        if not env.get("KUBECONFIG") or context != "kind-rill-benchmark":
            parser.error("--rollout requires the isolated kind-rill-benchmark kubeconfig")

    def run(profile, connections, seconds, *, rollout=False):
        cmd = ["taskset", "-c", args.cpus, args.wrk, f"-t{threads}", f"-c{connections}",
               f"-d{seconds}s", "--timeout", "5s", "-s", str(HERE / "mixed.lua"),
               "https://rill-bench:18443", "--", profile]
        start = time.monotonic()
        with subprocess.Popen(cmd, env=env, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True) as load:
            if rollout:
                time.sleep(8)
                kubectl = ["kubectl", "-n", "rill-bench", "rollout"]
                subprocess.run([*kubectl, "restart", "deploy/rill"], check=True)
                subprocess.run([*kubectl, "status", "deploy/rill", "--timeout=40s"], check=True)
                print(f"Rollout ready {time.monotonic() - start:.1f}s after load started", flush=True)
                if load.poll() is not None:
                    raise RuntimeError("load ended before rollout completed")
            output, _ = load.communicate(timeout=seconds + 15)
            print(output, end="", flush=True)
        results = [line.split()[1:] for line in output.splitlines() if line.startswith("RESULT ")]
        if load.returncode or len(results) != 1 or len(results[0]) != len(FIELDS):
            raise RuntimeError("wrk produced no complete result")
        row = dict(zip(FIELDS, results[0]))
        if int(row["requests"]) != int(row["checked"]) or float(row["elapsed_s"]) < seconds:
            raise RuntimeError("incomplete response validation or short run")
        if not rollout:
            if any(int(row[key]) for key in ("invalid", *FIELDS[-5:])):
                raise RuntimeError("steady-state run had response or transport errors")
            requests = int(row["requests"])
            if profile == "mixed":
                for route, fraction in (("static", .7), ("api", .2), ("download", .1)):
                    if abs(int(row[route]) / requests - fraction) > .01:
                        raise RuntimeError(f"unexpected mix for {route}")
            elif int(row[profile]) != requests:
                raise RuntimeError(f"unexpected response body for {profile}")
        return row

    profiles = (("mixed", 64),) if args.rollout else PROFILES
    path = args.state / ("rollout.csv" if args.rollout else "rounds.csv")
    with path.open("w", newline="") as file:
        rows = csv.DictWriter(file, ("profile", "connections", "round", *FIELDS), lineterminator="\n")
        rows.writeheader()
        for profile, connections in profiles:
            for round_number in range(1, (1 if args.rollout else args.rounds) + 1):
                print(f"{profile}, {connections} connections, round {round_number}", flush=True)
                run(profile, connections, args.warmup)
                result = run(profile, connections, args.seconds, rollout=args.rollout)
                rows.writerow(dict(profile=profile, connections=connections, round=round_number, **result))
                file.flush()


if __name__ == "__main__":
    main()
