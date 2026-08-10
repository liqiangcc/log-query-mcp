#!/usr/bin/env python3
"""Offline verifier for the two real-target M7 acceptance evidence files.

This tool never contacts the network, starts MCP, reads Secrets, or changes the
system. It validates the redacted evidence contracts produced by:

- m7_wsl_acceptance.py (service-identity stdio gate)
- m7_wsl_http_acceptance.py (production systemd HTTP gate)

`--self-test` uses synthetic in-memory evidence only and is suitable for CI and
release-package validation. A self-test PASS is not real-target evidence.
"""

from __future__ import annotations

import argparse
import copy
import json
import re
import sys
from datetime import datetime
from pathlib import Path
from typing import Any

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
EXPECTED_TOOLS = ["get_log_context", "list_log_sources", "search_logs"]
ALLOWED_PROXY_ARG_SHAPE = {"{host}", "{port}", "<literal>"}
FORBIDDEN_KEYS = {
    "password",
    "passphrase",
    "secret",
    "secret_value",
    "private_key",
    "raw_stderr",
    "stderr",
    "log_content",
    "content",
    "match_ref",
    "host",
    "logical_host",
    "target_host",
    "keyword",
    "marker",
}


class EvidenceError(RuntimeError):
    pass


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Verify redacted M7 real-target evidence offline")
    parser.add_argument("--stdio-evidence", help="m7-wsl-acceptance-*.json")
    parser.add_argument("--http-evidence", help="m7-wsl-http-acceptance-*.json")
    parser.add_argument("--self-test", action="store_true", help="run synthetic verifier tests only")
    return parser.parse_args()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise EvidenceError(message)


def require_sha(value: Any, name: str) -> None:
    require(isinstance(value, str) and SHA256_RE.fullmatch(value) is not None, f"{name} must be SHA256")


def require_timestamp(value: Any, name: str) -> None:
    require(isinstance(value, str) and value, f"{name} must be present")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise EvidenceError(f"{name} must be ISO-8601") from exc
    require(parsed.tzinfo is not None, f"{name} must include timezone")


def require_proxy_arg_shape(value: Any, name: str) -> None:
    require(isinstance(value, list) and bool(value), f"{name} must be a non-empty array")
    require(all(isinstance(item, str) for item in value), f"{name} must contain only strings")
    require(all(item in ALLOWED_PROXY_ARG_SHAPE for item in value), f"{name} contains unredacted ProxyCommand argv")
    require("{host}" in value and "{port}" in value, f"{name} must retain host/port placeholders")


def reject_sensitive_keys(value: Any, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            normalized = str(key).casefold()
            if normalized in FORBIDDEN_KEYS:
                raise EvidenceError(f"forbidden evidence key at {path}: {key}")
            if normalized.endswith("_host") and not normalized.endswith("_host_sha256"):
                raise EvidenceError(f"plaintext host-shaped key at {path}: {key}")
            reject_sensitive_keys(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            reject_sensitive_keys(child, f"{path}[{index}]")


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise EvidenceError(f"evidence is unreadable: {path}") from exc
    require(isinstance(value, dict), f"evidence root must be an object: {path}")
    reject_sensitive_keys(value)
    return value


def verify_stdio(value: dict[str, Any]) -> None:
    require(value.get("schema") == "log-query-mcp-m7-wsl-acceptance-v1", "unexpected stdio evidence schema")
    require(value.get("status") == "PASS", "stdio acceptance status is not PASS")
    require_timestamp(value.get("started_at_utc"), "stdio.started_at_utc")
    require_timestamp(value.get("finished_at_utc"), "stdio.finished_at_utc")
    require_sha(value.get("config_sha256"), "stdio.config_sha256")
    require_sha(value.get("stdio_binary_sha256"), "stdio.stdio_binary_sha256")
    require_sha(value.get("target_host_sha256"), "stdio.target_host_sha256")
    require_sha(value.get("keyword_sha256"), "stdio.keyword_sha256")
    require_proxy_arg_shape(value.get("proxy_args_shape"), "stdio.proxy_args_shape")
    require(isinstance(value.get("source_id"), str) and bool(value["source_id"]), "stdio source_id missing")
    require(isinstance(value.get("connection_id"), str) and bool(value["connection_id"]), "stdio connection_id missing")
    require(value.get("direct_wsl_tcp_reachable") is False, "stdio evidence does not prove Direct TCP gap")
    require(value.get("direct_path_requirement_met") is True, "stdio Direct-path requirement is not met")
    require(value.get("initialize") == "PASS", "stdio initialize is not PASS")
    require(value.get("list_log_sources") == "PASS", "stdio list_log_sources is not PASS")
    require(value.get("search_logs") == "PASS", "stdio search_logs is not PASS")
    require(value.get("get_log_context") == "PASS", "stdio get_log_context is not PASS")
    require(value.get("proxy_ssh_sftp_path") == "PASS", "stdio Proxy SSH/SFTP path is not PASS")
    require(value.get("helper_cleanup") == "PASS", "stdio helper cleanup is not PASS")
    require(sorted(value.get("tools", [])) == EXPECTED_TOOLS, "stdio MCP tool surface changed")
    before = value.get("helper_process_count_before")
    after = value.get("helper_process_count_after")
    require(isinstance(before, int) and isinstance(after, int) and after <= before, "stdio helper count did not return to baseline")
    require("failure_code" not in value and "failure_message" not in value, "PASS stdio evidence contains failure fields")


def verify_http(value: dict[str, Any]) -> None:
    require(value.get("acceptance") == "m7-wsl-systemd-http", "unexpected HTTP evidence contract")
    require(value.get("status") == "PASS", "systemd HTTP acceptance status is not PASS")
    require_timestamp(value.get("started_at_utc"), "http.started_at_utc")
    require_timestamp(value.get("completed_at_utc"), "http.completed_at_utc")
    require_sha(value.get("config_sha256"), "http.config_sha256")
    require_sha(value.get("logical_host_sha256"), "http.logical_host_sha256")
    require_sha(value.get("keyword_sha256"), "http.keyword_sha256")
    require_sha(value.get("endpoint_sha256"), "http.endpoint_sha256")
    require_proxy_arg_shape(value.get("proxy_argv_shape"), "http.proxy_argv_shape")
    require(isinstance(value.get("source_id"), str) and bool(value["source_id"]), "HTTP source_id missing")
    require(isinstance(value.get("connection_id"), str) and bool(value["connection_id"]), "HTTP connection_id missing")
    service = value.get("service")
    require(isinstance(service, dict), "HTTP service evidence missing")
    require(service.get("active_state") == "active", "systemd service is not active")
    require(isinstance(service.get("user"), str) and bool(service["user"]), "systemd service user missing")
    require(isinstance(service.get("main_pid"), int) and service["main_pid"] > 0, "systemd MainPID invalid")
    binary = value.get("service_binary")
    require(isinstance(binary, dict), "systemd binary evidence missing")
    require_sha(binary.get("running_sha256"), "http.service_binary.running_sha256")
    require_sha(binary.get("expected_sha256"), "http.service_binary.expected_sha256")
    require(binary["running_sha256"] == binary["expected_sha256"], "running systemd binary differs from expected candidate")
    checks = value.get("checks")
    require(isinstance(checks, dict), "HTTP checks object missing")
    for name in ("initialize", "tools_list", "list_log_sources", "search_logs", "get_log_context", "helper_cleanup"):
        require(checks.get(name) == "PASS", f"HTTP check is not PASS: {name}")
    before = value.get("helper_process_count_before")
    after = value.get("helper_process_count_after")
    require(isinstance(before, int) and isinstance(after, int) and after <= before, "HTTP helper count did not return to baseline")
    require("failure_code" not in value and "failure_message" not in value, "PASS HTTP evidence contains failure fields")


def verify_pair(stdio: dict[str, Any], http: dict[str, Any]) -> None:
    verify_stdio(stdio)
    verify_http(http)
    pairs = (
        ("config_sha256", "config_sha256"),
        ("source_id", "source_id"),
        ("connection_id", "connection_id"),
        ("keyword_sha256", "keyword_sha256"),
        ("target_host_sha256", "logical_host_sha256"),
    )
    for stdio_key, http_key in pairs:
        require(stdio.get(stdio_key) == http.get(http_key), f"stdio/HTTP evidence mismatch: {stdio_key}/{http_key}")
    stdio_commit = stdio.get("buildinfo", {}).get("git_commit") if isinstance(stdio.get("buildinfo"), dict) else None
    http_commit = http.get("buildinfo", {}).get("git_commit") if isinstance(http.get("buildinfo"), dict) else None
    if stdio_commit and http_commit:
        require(stdio_commit == http_commit, "stdio/HTTP BUILDINFO git_commit mismatch")


def synthetic_pair() -> tuple[dict[str, Any], dict[str, Any]]:
    h = "a" * 64
    candidate = "b" * 64
    stdio = {
        "schema": "log-query-mcp-m7-wsl-acceptance-v1",
        "started_at_utc": "2026-08-10T11:00:00+00:00",
        "finished_at_utc": "2026-08-10T11:01:00+00:00",
        "status": "PASS",
        "source_id": "proxy-source",
        "connection_id": "proxy-connection",
        "target_host_sha256": h,
        "target_port": 22,
        "auth_type": "password",
        "proxy_program_basename": "ncat.exe",
        "proxy_args_shape": ["<literal>", "<literal>", "{host}", "{port}"],
        "config_sha256": h,
        "stdio_binary_sha256": candidate,
        "keyword_sha256": h,
        "buildinfo": {"git_commit": "deadbeef"},
        "helper_process_count_before": 1,
        "helper_process_count_after": 1,
        "direct_wsl_tcp_reachable": False,
        "direct_path_requirement_met": True,
        "initialize": "PASS",
        "tools": EXPECTED_TOOLS,
        "list_log_sources": "PASS",
        "search_logs": "PASS",
        "get_log_context": "PASS",
        "proxy_ssh_sftp_path": "PASS",
        "helper_cleanup": "PASS",
    }
    http = {
        "acceptance": "m7-wsl-systemd-http",
        "started_at_utc": "2026-08-10T11:02:00+00:00",
        "completed_at_utc": "2026-08-10T11:03:00+00:00",
        "status": "PASS",
        "config_sha256": h,
        "source_id": "proxy-source",
        "connection_id": "proxy-connection",
        "logical_host_sha256": h,
        "target_port": 22,
        "auth_type": "password",
        "helper_image": "ncat.exe",
        "proxy_argv_shape": ["<literal>", "<literal>", "{host}", "{port}"],
        "keyword_sha256": h,
        "endpoint_sha256": h,
        "service": {"active_state": "active", "user": "log-query-mcp", "main_pid": 123},
        "service_binary": {"running_sha256": candidate, "expected_sha256": candidate},
        "buildinfo": {"git_commit": "deadbeef"},
        "helper_process_count_before": 1,
        "helper_process_count_after": 1,
        "checks": {
            "initialize": "PASS",
            "tools_list": "PASS",
            "list_log_sources": "PASS",
            "search_logs": "PASS",
            "get_log_context": "PASS",
            "helper_cleanup": "PASS",
        },
    }
    return stdio, http


def self_test() -> None:
    stdio, http = synthetic_pair()
    verify_pair(stdio, http)
    broken = copy.deepcopy(http)
    broken["checks"]["search_logs"] = "FAIL"
    try:
        verify_pair(stdio, broken)
    except EvidenceError:
        pass
    else:
        raise EvidenceError("self-test failed to reject a broken HTTP evidence record")
    leaked = copy.deepcopy(stdio)
    leaked["match_ref"] = "must-never-be-persisted"
    try:
        verify_pair(leaked, http)
    except EvidenceError:
        pass
    else:
        raise EvidenceError("self-test failed to reject a sensitive evidence key")
    raw_argv = copy.deepcopy(stdio)
    raw_argv["proxy_args_shape"] = ["--proxy-type", "corporate-secret-token", "{host}", "{port}"]
    try:
        verify_pair(raw_argv, http)
    except EvidenceError:
        pass
    else:
        raise EvidenceError("self-test failed to reject unredacted ProxyCommand argv")


def main() -> int:
    args = parse_args()
    try:
        if args.self_test:
            require(not args.stdio_evidence and not args.http_evidence, "--self-test cannot be combined with evidence files")
            self_test()
            print("verify_m7_evidence: SELF_TEST_PASS (synthetic only; not real-target evidence)")
            return 0
        require(bool(args.stdio_evidence) and bool(args.http_evidence), "both --stdio-evidence and --http-evidence are required")
        stdio = load_json(Path(args.stdio_evidence))
        http = load_json(Path(args.http_evidence))
        verify_pair(stdio, http)
        print("verify_m7_evidence: PASS")
        return 0
    except EvidenceError as exc:
        print(f"verify_m7_evidence: FAIL: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())