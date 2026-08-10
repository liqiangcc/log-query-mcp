#!/usr/bin/env python3
"""Maintain a redacted run-level manifest for M7 real-target acceptance.

The manifest is orchestration evidence, not a replacement for the richer stdio
or systemd HTTP evidence files. Its purpose is to guarantee that every real
acceptance attempt remains traceable even when a gate fails before its component
can create component-specific evidence.

No Secret values, plaintext logical hosts, ProxyCommand argv, log content,
match_ref values, raw stderr, or marker plaintext are stored.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

GIT_COMMIT_RE = re.compile(r"^[0-9a-f]{40,64}$")
VALID_GATES = ("A", "B", "C", "D")


class ManifestError(RuntimeError):
    pass


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as exc:
        raise ManifestError("required file could not be hashed") from exc
    return digest.hexdigest()


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def read_buildinfo(path: Path) -> dict[str, str]:
    if not path.is_file():
        raise ManifestError("BUILDINFO is missing")
    allowed = {"version", "target", "git_commit", "git_ref", "built_at_utc", "rustc"}
    result: dict[str, str] = {}
    try:
        for line in path.read_text(encoding="utf-8").splitlines():
            key, sep, value = line.partition("=")
            if sep and key in allowed:
                result[key] = value
    except OSError as exc:
        raise ManifestError("BUILDINFO is unreadable") from exc
    commit = result.get("git_commit")
    if not isinstance(commit, str) or GIT_COMMIT_RE.fullmatch(commit) is None:
        raise ManifestError("BUILDINFO git_commit does not identify a packaged candidate")
    return result


def write_manifest(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    try:
        path.chmod(0o600)
    except OSError:
        pass


def load_manifest(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ManifestError("run manifest is unreadable") from exc
    if not isinstance(value, dict) or value.get("schema") != "log-query-mcp-m7-real-target-run-v1":
        raise ManifestError("run manifest schema is invalid")
    return value


def start(args: argparse.Namespace) -> None:
    path = Path(args.manifest)
    if path.exists():
        raise ManifestError("run manifest already exists")
    config = Path(args.config)
    stdio_bin = Path(args.stdio_bin)
    http_bin = Path(args.http_bin)
    if not config.is_file():
        raise ManifestError("config is not a regular file")
    if not stdio_bin.is_file():
        raise ManifestError("stdio binary is not a regular file")
    if not http_bin.is_file():
        raise ManifestError("HTTP binary is not a regular file")
    buildinfo = read_buildinfo(Path(args.buildinfo))
    value: dict[str, Any] = {
        "schema": "log-query-mcp-m7-real-target-run-v1",
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "status": "RUNNING",
        "source_id": args.source_id,
        "config_sha256": sha256_file(config),
        "keyword_sha256": sha256_text(args.keyword),
        "stdio_binary_sha256": sha256_file(stdio_bin),
        "http_binary_sha256": sha256_file(http_bin),
        "buildinfo": buildinfo,
        "completed_gates": [],
    }
    write_manifest(path, value)


def gate_pass(args: argparse.Namespace) -> None:
    value = load_manifest(Path(args.manifest))
    if value.get("status") != "RUNNING":
        raise ManifestError("only a RUNNING manifest can record a gate PASS")
    completed = value.get("completed_gates")
    if not isinstance(completed, list):
        raise ManifestError("completed_gates is invalid")
    gate = args.gate
    expected_index = len(completed)
    if expected_index >= len(VALID_GATES) or VALID_GATES[expected_index] != gate:
        raise ManifestError("gate PASS is out of order")
    completed.append(gate)
    value["last_gate_completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    write_manifest(Path(args.manifest), value)


def fail(args: argparse.Namespace) -> None:
    value = load_manifest(Path(args.manifest))
    if value.get("status") != "RUNNING":
        raise ManifestError("only a RUNNING manifest can fail")
    value["status"] = "FAIL"
    value["failed_gate"] = args.gate
    value["gate_exit_code"] = args.exit_code
    value["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
    write_manifest(Path(args.manifest), value)


def finish_pass(args: argparse.Namespace) -> None:
    value = load_manifest(Path(args.manifest))
    if value.get("status") != "RUNNING":
        raise ManifestError("only a RUNNING manifest can complete")
    if value.get("completed_gates") != list(VALID_GATES):
        raise ManifestError("all four gates must pass before the run can PASS")
    value["status"] = "PASS"
    value["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
    write_manifest(Path(args.manifest), value)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Maintain redacted M7 real-target run manifest")
    sub = parser.add_subparsers(dest="command", required=True)

    start_parser = sub.add_parser("start")
    start_parser.add_argument("--manifest", required=True)
    start_parser.add_argument("--config", required=True)
    start_parser.add_argument("--source-id", required=True)
    start_parser.add_argument("--keyword", required=True)
    start_parser.add_argument("--buildinfo", required=True)
    start_parser.add_argument("--stdio-bin", required=True)
    start_parser.add_argument("--http-bin", required=True)

    gate_parser = sub.add_parser("gate-pass")
    gate_parser.add_argument("--manifest", required=True)
    gate_parser.add_argument("--gate", required=True, choices=VALID_GATES)

    fail_parser = sub.add_parser("fail")
    fail_parser.add_argument("--manifest", required=True)
    fail_parser.add_argument("--gate", required=True, choices=VALID_GATES)
    fail_parser.add_argument("--exit-code", required=True, type=int)

    pass_parser = sub.add_parser("pass")
    pass_parser.add_argument("--manifest", required=True)

    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.command == "start":
            start(args)
        elif args.command == "gate-pass":
            gate_pass(args)
        elif args.command == "fail":
            fail(args)
        elif args.command == "pass":
            finish_pass(args)
        else:
            raise ManifestError("unknown command")
        return 0
    except ManifestError as exc:
        print(f"m7_real_target_manifest: FAIL: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())