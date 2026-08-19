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
import tempfile
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
    temporary = path.with_name(f".{path.name}.tmp")
    payload = json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    try:
        temporary.write_text(payload, encoding="utf-8")
        try:
            temporary.chmod(0o600)
        except OSError:
            pass
        temporary.replace(path)
        try:
            path.chmod(0o600)
        except OSError:
            pass
    except OSError as exc:
        try:
            temporary.unlink(missing_ok=True)
        except OSError:
            pass
        raise ManifestError("run manifest could not be written atomically") from exc


def load_manifest(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ManifestError("run manifest is unreadable") from exc
    if not isinstance(value, dict) or value.get("schema") != "log-query-mcp-m7-real-target-run-v1":
        raise ManifestError("run manifest schema is invalid")
    return value


def completed_gates(value: dict[str, Any]) -> list[str]:
    completed = value.get("completed_gates")
    if not isinstance(completed, list) or not all(isinstance(item, str) for item in completed):
        raise ManifestError("completed_gates is invalid")
    if completed != list(VALID_GATES[: len(completed)]):
        raise ManifestError("completed_gates is out of order")
    return completed


def expected_next_gate(value: dict[str, Any]) -> str:
    completed = completed_gates(value)
    if len(completed) >= len(VALID_GATES):
        raise ManifestError("no gate remains to be recorded")
    return VALID_GATES[len(completed)]


def start(args: argparse.Namespace) -> None:
    path = Path(args.manifest)
    if path.exists():
        raise ManifestError("run manifest already exists")
    if not args.source_id:
        raise ManifestError("source_id must be non-empty")
    if not args.keyword:
        raise ManifestError("keyword must be non-empty")
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
    path = Path(args.manifest)
    value = load_manifest(path)
    if value.get("status") != "RUNNING":
        raise ManifestError("only a RUNNING manifest can record a gate PASS")
    if expected_next_gate(value) != args.gate:
        raise ManifestError("gate PASS is out of order")
    completed = completed_gates(value)
    completed.append(args.gate)
    value["last_gate_completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    write_manifest(path, value)


def fail(args: argparse.Namespace) -> None:
    path = Path(args.manifest)
    value = load_manifest(path)
    if value.get("status") != "RUNNING":
        raise ManifestError("only a RUNNING manifest can fail")
    if expected_next_gate(value) != args.gate:
        raise ManifestError("failed gate is out of order")
    if not 1 <= args.exit_code <= 255:
        raise ManifestError("failed gate exit_code must be between 1 and 255")
    value["status"] = "FAIL"
    value["failed_gate"] = args.gate
    value["gate_exit_code"] = args.exit_code
    value["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
    write_manifest(path, value)


def finish_pass(args: argparse.Namespace) -> None:
    path = Path(args.manifest)
    value = load_manifest(path)
    if value.get("status") != "RUNNING":
        raise ManifestError("only a RUNNING manifest can complete")
    if completed_gates(value) != list(VALID_GATES):
        raise ManifestError("all four gates must pass before the run can PASS")
    value["status"] = "PASS"
    value["finished_at_utc"] = datetime.now(timezone.utc).isoformat()
    write_manifest(path, value)


def expect_manifest_error(action: Any, message: str) -> None:
    try:
        action()
    except ManifestError:
        return
    raise ManifestError(message)


def self_test() -> None:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        config = root / "config.json"
        stdio_bin = root / "stdio"
        http_bin = root / "http"
        buildinfo = root / "BUILDINFO"
        config.write_text('{"version":2}\n', encoding="utf-8")
        stdio_bin.write_bytes(b"stdio-candidate")
        http_bin.write_bytes(b"http-candidate")
        buildinfo.write_text(
            "version=0.2.0\n"
            "target=x86_64-unknown-linux-gnu\n"
            f"git_commit={'c' * 40}\n"
            "git_ref=self-test\n"
            "built_at_utc=2026-08-10T00:00:00Z\n"
            "rustc=rustc-self-test\n",
            encoding="utf-8",
        )

        def start_args(path: Path) -> argparse.Namespace:
            return argparse.Namespace(
                manifest=str(path),
                config=str(config),
                source_id="proxy-source",
                keyword="synthetic-marker-plaintext",
                buildinfo=str(buildinfo),
                stdio_bin=str(stdio_bin),
                http_bin=str(http_bin),
            )

        pass_manifest = root / "pass.json"
        start(start_args(pass_manifest))
        for gate in VALID_GATES:
            gate_pass(argparse.Namespace(manifest=str(pass_manifest), gate=gate))
        finish_pass(argparse.Namespace(manifest=str(pass_manifest)))
        passed = load_manifest(pass_manifest)
        if passed.get("status") != "PASS" or passed.get("completed_gates") != list(VALID_GATES):
            raise ManifestError("self-test PASS lifecycle failed")
        serialized = pass_manifest.read_text(encoding="utf-8")
        if "synthetic-marker-plaintext" in serialized or '"keyword"' in serialized:
            raise ManifestError("self-test detected marker plaintext in run manifest")

        fail_manifest = root / "fail.json"
        start(start_args(fail_manifest))
        gate_pass(argparse.Namespace(manifest=str(fail_manifest), gate="A"))
        expect_manifest_error(
            lambda: gate_pass(argparse.Namespace(manifest=str(fail_manifest), gate="C")),
            "self-test failed to reject out-of-order gate PASS",
        )
        expect_manifest_error(
            lambda: fail(argparse.Namespace(manifest=str(fail_manifest), gate="B", exit_code=0)),
            "self-test failed to reject zero failure exit code",
        )
        fail(argparse.Namespace(manifest=str(fail_manifest), gate="B", exit_code=17))
        failed = load_manifest(fail_manifest)
        if (
            failed.get("status") != "FAIL"
            or failed.get("completed_gates") != ["A"]
            or failed.get("failed_gate") != "B"
            or failed.get("gate_exit_code") != 17
        ):
            raise ManifestError("self-test FAIL lifecycle failed")


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

    sub.add_parser("self-test")

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
        elif args.command == "self-test":
            self_test()
            print("m7_real_target_manifest: SELF_TEST_PASS (synthetic only; not real-target evidence)")
        else:
            raise ManifestError("unknown command")
        return 0
    except ManifestError as exc:
        print(f"m7_real_target_manifest: FAIL: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
