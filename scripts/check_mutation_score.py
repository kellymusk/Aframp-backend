#!/usr/bin/env python3
"""
check_mutation_score.py — parse cargo-mutants output and fail if the mutation
score falls below a required threshold.

Usage:
    python3 scripts/check_mutation_score.py <mutants_log_file> <threshold_percent>

Arguments:
    mutants_log_file    Path to the file containing cargo-mutants stdout output.
    threshold_percent   Minimum acceptable mutation score (0–100). The script
                        exits with code 1 if the score is below this value.

Example (in CI):
    cargo mutants ... 2>&1 | tee mutants.log
    python3 scripts/check_mutation_score.py mutants.log 85

Exit codes:
    0   Mutation score meets or exceeds the threshold.
    1   Mutation score is below the threshold, or the log could not be parsed.
"""

import re
import sys


def parse_summary(log_path: str) -> tuple[int, int]:
    """
    Parse a cargo-mutants summary line of the form:

        N mutants tested: X caught, Y missed, Z timeout

    Returns (caught, missed). Timeout mutants are excluded from the score
    calculation because they are an environment issue, not a test quality issue.

    Raises ValueError if no summary line is found.
    """
    pattern = re.compile(
        r"(\d+) mutants? tested.*?(\d+) caught.*?(\d+) missed",
        re.IGNORECASE,
    )
    with open(log_path, encoding="utf-8") as fh:
        for line in fh:
            m = pattern.search(line)
            if m:
                caught = int(m.group(2))
                missed = int(m.group(3))
                return caught, missed
    raise ValueError(
        f"No cargo-mutants summary line found in {log_path!r}.\n"
        "Expected a line like: '217 mutants tested: 198 caught, 12 missed, 7 timeout'"
    )


def main() -> int:
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <mutants_log_file> <threshold_percent>", file=sys.stderr)
        return 1

    log_path = sys.argv[1]
    try:
        threshold = float(sys.argv[2])
    except ValueError:
        print(f"Error: threshold must be a number, got {sys.argv[2]!r}", file=sys.stderr)
        return 1

    try:
        caught, missed = parse_summary(log_path)
    except (ValueError, FileNotFoundError) as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1

    total = caught + missed
    if total == 0:
        print("No mutants found — nothing to score.", file=sys.stderr)
        return 1

    score = (caught / total) * 100
    print(f"Mutation score: {score:.1f}% ({caught} caught / {total} total, {missed} missed)")

    if score < threshold:
        print(
            f"FAIL: score {score:.1f}% is below the required threshold of {threshold:.1f}%.",
            file=sys.stderr,
        )
        return 1

    print(f"PASS: score {score:.1f}% meets the threshold of {threshold:.1f}%.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
