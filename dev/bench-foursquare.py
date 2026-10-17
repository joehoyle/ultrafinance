#!/usr/bin/env python3
"""Compare Foursquare reconciliation binaries on the same local catalog.

Prepares the input once, then runs rollback-only reconciliation previews
sequentially, alternating binaries to reduce ordering/cache bias. Requires a
local initialized PostgreSQL database; does not load .env or contact Foursquare.
"""

import argparse
import collections
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import time
from urllib.parse import urlsplit


STAGE = re.compile(r"^Import: (.*?) \d+/\d+ \(100%\) · ([\d.]+)s")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", action="append", required=True, type=Path)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--region", default="ca")
    parser.add_argument("--limit", type=int, default=20000)
    parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()
    database = os.environ.get("ULTRAFINANCE_DATABASE_URL", "")
    url = urlsplit(database)
    if url.scheme not in ("postgres", "postgresql") or url.hostname not in (
        "localhost", "127.0.0.1", "::1"
    ) or url.query not in ("", "sslmode=disable", "sslmode=require"):
        parser.error("set ULTRAFINANCE_DATABASE_URL to a local PostgreSQL URL (no host overrides)")
    if args.limit < 1 or args.repeats < 1:
        parser.error("limit and repeats must be positive")
    args.output.mkdir(parents=True, exist_ok=True)
    binaries = [str(p.resolve(strict=True)) for p in args.binary]
    common = [
        "sources", "import", "foursquare", "--input", str(args.input.resolve(strict=True)),
        "--region", args.region, "--limit", str(args.limit), "--verbose",
        "--output", str((args.output / "prepared").resolve()),
    ]
    # Exclude sorting and validation from the comparison. Both versions reuse
    # the same prepared CSV files, with no committed application checkpoint.
    with (args.output / "prepare.log").open("w") as log:
        subprocess.run([binaries[0], *common, "--dry-run"], stdout=log, stderr=log, check=True)
    results = []
    for repeat in range(args.repeats):
        order = list(enumerate(binaries))
        if repeat % 2:
            order.reverse()
        for index, binary in order:
            stem = args.output / f"binary-{index}-run-{repeat + 1}"
            started = time.perf_counter()
            with stem.with_suffix(".json").open("w") as output, stem.with_suffix(".log").open("w") as log:
                subprocess.run([binary, *common, "--dedupe-dry-run"], stdout=output, stderr=log, check=True)
            elapsed = time.perf_counter() - started
            report = json.loads(stem.with_suffix(".json").read_text())
            selected = report["selection"]["selected_records"]
            stages = collections.defaultdict(float)
            for line in stem.with_suffix(".log").read_text().splitlines():
                match = STAGE.match(line)
                if match:
                    stages[match[1]] += float(match[2])
            result = dict(binary=binary, repeat=repeat + 1, selected=selected,
                          dedupe_candidates=report["dedupe"]["candidates"],
                          seconds=elapsed, records_per_second=selected / elapsed,
                          stage_seconds=dict(stages))
            results.append(result)
            print(json.dumps(result), flush=True)
    if len({(r["selected"], r["dedupe_candidates"]) for r in results}) != 1:
        raise RuntimeError("comparison changed selected records or reconciliation counts; inspect run reports")
    summaries = []
    for binary in binaries:
        runs = [r for r in results if r["binary"] == binary]
        summaries.append(dict(binary=binary,
                              median_seconds=statistics.median(r["seconds"] for r in runs),
                              median_records_per_second=statistics.median(r["records_per_second"] for r in runs)))
    summary = dict(runs=results, summaries=summaries,
                   notes="Rollback previews; input preparation cached; per-stage timings rounded by CLI and nested, not additive. No concurrent builds/imports should run.")
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summaries, indent=2))


if __name__ == "__main__":
    main()
