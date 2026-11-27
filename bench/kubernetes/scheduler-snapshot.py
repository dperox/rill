#!/usr/bin/env python3
"""Write Linux scheduler counters for each Rill server thread in the kind lab."""
import csv
from concurrent.futures import ThreadPoolExecutor
import json
import subprocess
import sys


def kubectl(*args):
    return subprocess.check_output(["kubectl", "-n", "rill-bench", *args], text=True)


def threads(pod):
    script = 'for t in /proc/1/task/*; do cat "$t/stat" "$t/schedstat"; done'
    lines = kubectl("exec", pod, "--", "sh", "-ec", script).splitlines()
    rows = []
    for stat, schedstat in zip(lines[::2], lines[1::2], strict=True):
        tid, fields = stat.split(" (", 1)
        name, _ = fields.rsplit(") ", 1)
        runtime, wait, slices = map(int, schedstat.split())
        rows.append((pod, tid, name, runtime, wait, slices))
    return rows


def main():
    if kubectl("config", "current-context").strip() != "kind-rill-benchmark":
        sys.exit("Select the isolated kind-rill-benchmark kubeconfig.")
    response = kubectl("get", "pods", "-l", "app=rill", "-o", "json")
    pods = [
        pod["metadata"]["name"] for pod in json.loads(response)["items"]
        if pod["status"]["phase"] == "Running" and "deletionTimestamp" not in pod["metadata"]
    ]
    if len(pods) != 3:
        sys.exit("Expected three running Rill pods.")
    output = csv.writer(sys.stdout, lineterminator="\n")
    output.writerow(("pod", "tid", "thread", "runtime_ns", "wait_ns", "timeslices"))
    with ThreadPoolExecutor(len(pods)) as pool:
        for rows in pool.map(threads, pods):
            output.writerows(rows)


if __name__ == "__main__":
    main()
