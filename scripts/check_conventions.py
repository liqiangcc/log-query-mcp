#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
HTTP_ROOT = ROOT / "tests" / "http"
EXPECTED_INITIALIZE = HTTP_ROOT / "mcp" / "initialize.http"


def fail(rule: str, path: Path | None, message: str) -> None:
    location = f" {path.relative_to(ROOT)}" if path else ""
    print(f"[{rule}]{location}: {message}", file=sys.stderr)


def main() -> int:
    failures = 0

    # Project-local rule: the production-safe MCP initialization case is a required asset.
    if not EXPECTED_INITIALIZE.is_file():
        fail("LQM_HTTP001", EXPECTED_INITIALIZE, "required MCP initialize HTTP case is missing")
        failures += 1

    http_files = sorted(HTTP_ROOT.rglob("*.http")) if HTTP_ROOT.exists() else []
    seen_names: dict[str, Path] = {}

    for path in http_files:
        text = path.read_text(encoding="utf-8")

        name_match = re.search(r"^#\s*@name\s+(\S+)\s*$", text, re.MULTILINE)
        if not name_match:
            fail("LQM_HTTP002", path, "missing '# @name <name>' metadata")
            failures += 1
        else:
            name = name_match.group(1)
            previous = seen_names.get(name)
            if previous:
                fail(
                    "LQM_HTTP002",
                    path,
                    f"duplicate HTTP case name '{name}' also used by {previous.relative_to(ROOT)}",
                )
                failures += 1
            else:
                seen_names[name] = path

        tags_match = re.search(r"^#\s*@tags\s+(.+?)\s*$", text, re.MULTILINE)
        tags = set(tags_match.group(1).split()) if tags_match else set()
        if "production-safe" in tags and "destructive" in tags:
            fail("LQM_HTTP003", path, "production-safe case must not also be tagged destructive")
            failures += 1

        request_line = next(
            (
                line.strip()
                for line in text.splitlines()
                if re.match(r"^(GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD)\s+", line.strip())
            ),
            None,
        )
        if request_line and "{{$processEnv BASE_URL}}" not in request_line:
            fail("LQM_HTTP004", path, "HTTP request must use the BASE_URL environment variable")
            failures += 1

        if not re.search(r"^\?\?\s+status\s*==\s*\d{3}\s*$", text, re.MULTILINE):
            fail("LQM_HTTP005", path, "HTTP case must assert an explicit status code")
            failures += 1

    if failures:
        print(f"[conventions] FAIL ({failures} violation(s))", file=sys.stderr)
        return 1

    print(f"[conventions] PASS ({len(http_files)} HTTP case(s) checked)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
