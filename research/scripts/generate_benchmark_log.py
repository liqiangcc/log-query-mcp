#!/usr/bin/env python3
"""Generate a deterministic text log for scanner benchmarks."""

from __future__ import annotations

import argparse
from pathlib import Path

MIB = 1024 * 1024


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--size-mib", type=int, default=1024)
    parser.add_argument("--keyword", default="BENCHMARK_MATCH")
    parser.add_argument("--match-every", type=int, default=100_000)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.size_mib <= 0:
        raise SystemExit("--size-mib must be greater than zero")
    if args.match_every <= 0:
        raise SystemExit("--match-every must be greater than zero")
    if not args.keyword or "\n" in args.keyword or "\r" in args.keyword:
        raise SystemExit("--keyword must be a non-empty single-line string")

    target_bytes = args.size_mib * MIB
    args.output.parent.mkdir(parents=True, exist_ok=True)

    line_number = 0
    written = 0
    with args.output.open("wb") as output:
        while written < target_bytes:
            line_number += 1
            marker = args.keyword if line_number % args.match_every == 0 else "ok"
            line = (
                f"2026-06-19T14:20:03.125+09:00 INFO "
                f"requestId=req-{line_number:012d} status={marker} "
                f"message=deterministic-benchmark-line\n"
            ).encode("utf-8")
            remaining = target_bytes - written
            chunk = line[:remaining]
            output.write(chunk)
            written += len(chunk)

    print(
        f"generated={args.output} bytes={written} "
        f"lines={line_number} keyword={args.keyword!r}"
    )


if __name__ == "__main__":
    main()
