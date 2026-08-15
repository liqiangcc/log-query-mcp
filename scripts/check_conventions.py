#!/usr/bin/env python3
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re
import sys

DEFAULT_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class Violation:
    rule: str
    path: Path | None
    message: str


def collect_violations(root: Path) -> tuple[list[Violation], int]:
    root = root.resolve()
    http_root = root / "tests" / "http"
    expected_initialize = http_root / "mcp" / "initialize.http"
    violations: list[Violation] = []

    def add(rule: str, path: Path | None, message: str) -> None:
        violations.append(Violation(rule=rule, path=path, message=message))

    # Project-local rule: the production-safe MCP initialization case is a required asset.
    if not expected_initialize.is_file():
        add("LQM_HTTP001", expected_initialize, "required MCP initialize HTTP case is missing")

    http_files = sorted(http_root.rglob("*.http")) if http_root.exists() else []
    seen_names: dict[str, Path] = {}

    for path in http_files:
        text = path.read_text(encoding="utf-8")

        name_match = re.search(r"^#\s*@name\s+(\S+)\s*$", text, re.MULTILINE)
        if not name_match:
            add("LQM_HTTP002", path, "missing '# @name <name>' metadata")
        else:
            name = name_match.group(1)
            previous = seen_names.get(name)
            if previous:
                add(
                    "LQM_HTTP002",
                    path,
                    f"duplicate HTTP case name '{name}' also used by {previous.relative_to(root)}",
                )
            else:
                seen_names[name] = path

        tags_match = re.search(r"^#\s*@tags\s+(.+?)\s*$", text, re.MULTILINE)
        tags = set(tags_match.group(1).split()) if tags_match else set()
        if "production-safe" in tags and "destructive" in tags:
            add("LQM_HTTP003", path, "production-safe case must not also be tagged destructive")

        request_line = next(
            (
                line.strip()
                for line in text.splitlines()
                if re.match(r"^(GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD)\s+", line.strip())
            ),
            None,
        )
        if request_line and "{{$processEnv BASE_URL}}" not in request_line:
            add("LQM_HTTP004", path, "HTTP request must use the BASE_URL environment variable")

        if not re.search(r"^\?\?\s+status\s*==\s*\d{3}\s*$", text, re.MULTILINE):
            add("LQM_HTTP005", path, "HTTP case must assert an explicit status code")

    return violations, len(http_files)


def format_violation(root: Path, violation: Violation) -> str:
    location = ""
    if violation.path is not None:
        try:
            relative = violation.path.resolve().relative_to(root.resolve())
        except ValueError:
            relative = violation.path
        location = f" {relative}"
    return f"[{violation.rule}]{location}: {violation.message}"


def run(root: Path = DEFAULT_ROOT) -> int:
    violations, checked_count = collect_violations(root)

    for violation in violations:
        print(format_violation(root, violation), file=sys.stderr)

    if violations:
        print(f"[conventions] FAIL ({len(violations)} violation(s))", file=sys.stderr)
        return 1

    print(f"[conventions] PASS ({checked_count} HTTP case(s) checked)")
    return 0


if __name__ == "__main__":
    raise SystemExit(run())
