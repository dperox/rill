"""Compare median throughput and full-response p95 from matched Japka runs."""

import argparse
import json
import math
from pathlib import Path
from statistics import median


def load_run(directory):
    config = json.loads((directory / "run.json").read_text())
    if config["smoke"]:
        raise ValueError(f"{directory} is a smoke test; use timed runs for comparisons")
    conditions = {key: config[key] for key in
                  ("platform", "users", "seconds", "warmup", "cases", "client_sha256")}
    measurements = {}
    for case in conditions["cases"]:
        sample = json.loads((directory / f"{case}.summary.json").read_text())
        rate, p95 = sample["requests_per_second"], sample["p95_us"]
        if (sample["case"] != case or sample["validated_requests"] <= 0
                or not all(math.isfinite(x) and x > 0 for x in (rate, p95))):
            raise ValueError(f"{directory} has an invalid measurement for {case}")
        measurements[case] = (rate, p95)
    if not measurements:
        raise ValueError(f"{directory} contains no cases")
    return conditions, measurements


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", nargs="+", required=True, type=Path, metavar="DIR")
    parser.add_argument("--candidate", nargs="+", required=True, type=Path, metavar="DIR")
    parser.add_argument("--max-regression", type=float, metavar="PERCENT",
                        help="exit 1 if throughput falls or p95 rises by more than this percent")
    args = parser.parse_args()
    if len(args.baseline) != len(args.candidate):
        parser.error("provide one candidate run for each baseline run")
    if args.max_regression is not None and not (0 <= args.max_regression < 100):
        parser.error("--max-regression must be between 0 (inclusive) and 100 (exclusive)")

    directories = args.baseline + args.candidate
    if len({path.resolve() for path in directories}) != len(directories):
        parser.error("each run directory must be distinct")
    try:
        runs = [load_run(path) for path in directories]
        conditions = runs[0][0]
        for directory, (config, _) in zip(directories, runs):
            differences = [key for key in conditions if config[key] != conditions[key]]
            if differences:
                raise ValueError(f"{directory} differs in {', '.join(differences)}")
    except (OSError, ValueError, TypeError, KeyError) as error:
        parser.error(str(error))

    print("case          baseline req/s  candidate req/s    change   baseline p95 µs  candidate p95 µs    change")
    failed = False
    for case in conditions["cases"]:
        samples = [measurements[case] for _, measurements in runs]
        count = len(args.baseline)
        before_rate, before_p95 = map(median, zip(*samples[:count]))
        after_rate, after_p95 = map(median, zip(*samples[count:]))
        rate_change = 100 * (after_rate - before_rate) / before_rate
        p95_change = 100 * (after_p95 - before_p95) / before_p95
        print(f"{case:<13} {before_rate:14,.1f} {after_rate:16,.1f} {rate_change:+8.2f}%"
              f" {before_p95:16,.1f} {after_p95:17,.1f} {p95_change:+8.2f}%")
        if args.max_regression is not None and max(-rate_change, p95_change) > args.max_regression:
            failed = True
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
