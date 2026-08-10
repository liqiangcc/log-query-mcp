#!/usr/bin/env python3
"""Traceable systemd Streamable HTTP acceptance for M7 ProxyCommand.

This client targets the production `log-query-mcp` HTTP service rather than
starting another binary. It verifies that the actual systemd service identity
can exercise a ProxyCommand-backed source end to end.

The script uses Python stdlib only and deliberately redacts Secret values,
logical host plaintext, match_ref, raw stderr, and log content from evidence.
Use --validate-config-only for repository/package static checks.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

EXPECTED_TOOLS = {"list_log_sources", "search_logs", "get_log_context"}
PROTOCOL_VERSION = "2025-06-18"
MAX_HTTP_RESPONSE_BYTES = 8 * 1024 * 1024


class AcceptanceError(RuntimeError):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.safe_message = message


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run M7 systemd HTTP ProxyCommand source acceptance."
    )
    parser.add_argument("--config", required=True, help="v2 config used by the running service")
    parser.add_argument("--source-id", required=True, help="ProxyCommand-backed source to query")
    parser.add_argument("--keyword", help="known marker that must exist in the selected remote log")
    parser.add_argument("--url", default="http://127.0.0.1:8000/mcp")
    parser.add_argument("--service-name", default="log-query-mcp.service")
    parser.add_argument("--expected-service-user", default="log-query-mcp")
    parser.add_argument("--systemctl-bin", default="systemctl")
    parser.add_argument("--tasklist-bin", default="tasklist.exe")
    parser.add_argument("--buildinfo", default="/opt/log-query-mcp/BUILDINFO")
    parser.add_argument("--evidence-dir", default="m7-wsl-evidence")
    parser.add_argument("--timeout-seconds", type=float, default=30.0)
    parser.add_argument("--before-lines", type=int, default=1)
    parser.add_argument("--after-lines", type=int, default=1)
    parser.add_argument(
        "--validate-config-only",
        action="store_true",
        help="validate selected ProxyCommand config only; no systemd/network/process checks",
    )
    return parser.parse_args()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def read_buildinfo(path: Path) -> dict[str, str]:
    if not path.is_file():
        return {}
    allowed = {"version", "target", "git_commit", "git_ref", "built_at_utc", "rustc"}
    result: dict[str, str] = {}
    try:
        for line in path.read_text(encoding="utf-8").splitlines():
            key, sep, value = line.partition("=")
            if sep and key in allowed:
                result[key] = value
    except OSError:
        return {}
    return result


def load_target(config_path: Path, source_id: str, *, real_acceptance: bool) -> tuple[dict[str, Any], dict[str, Any]]:
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise AcceptanceError("CONFIG_UNREADABLE", "acceptance config could not be loaded") from exc

    if config.get("version") != 2:
        raise AcceptanceError("CONFIG_NOT_V2", "acceptance config must use version=2")

    source = next((item for item in config.get("sources", []) if item.get("source_id") == source_id), None)
    if not isinstance(source, dict):
        raise AcceptanceError("SOURCE_NOT_FOUND", "selected source_id is not present in the config")
    backend = source.get("backend")
    if not isinstance(backend, dict) or backend.get("type") != "ssh":
        raise AcceptanceError("SOURCE_NOT_SSH", "selected source must use backend.type=ssh")

    connection_id = backend.get("connection_id")
    if not isinstance(connection_id, str) or not connection_id:
        raise AcceptanceError("CONNECTION_ID_MISSING", "selected source has no SSH connection_id")
    connection = next(
        (item for item in config.get("connections", []) if item.get("connection_id") == connection_id),
        None,
    )
    if not isinstance(connection, dict):
        raise AcceptanceError("CONNECTION_NOT_FOUND", "selected source SSH connection is missing")

    proxy = connection.get("proxy")
    if not isinstance(proxy, dict) or proxy.get("type") != "command":
        raise AcceptanceError("PROXY_NOT_COMMAND", "selected SSH connection must use proxy.type=command")
    program = proxy.get("program")
    args = proxy.get("args")
    if not isinstance(program, str) or not program:
        raise AcceptanceError("PROXY_PROGRAM_INVALID", "ProxyCommand program must be non-empty")
    if not isinstance(args, list) or not all(isinstance(item, str) for item in args):
        raise AcceptanceError("PROXY_ARGS_INVALID", "ProxyCommand args must be a string array")
    for item in args:
        if ("{" in item or "}" in item) and item not in {"{host}", "{port}"}:
            raise AcceptanceError(
                "PROXY_PLACEHOLDER_INVALID",
                "ProxyCommand placeholders must be whole argv items {host} or {port}",
            )
    if "{host}" not in args or "{port}" not in args:
        raise AcceptanceError(
            "PROXY_TARGET_PLACEHOLDERS_MISSING",
            "WSL acceptance requires both {host} and {port} ProxyCommand argv items",
        )

    host = connection.get("host")
    port = connection.get("port", 22)
    if not isinstance(host, str) or not host or not isinstance(port, int) or not (1 <= port <= 65535):
        raise AcceptanceError("TARGET_INVALID", "SSH logical host/port is invalid")

    auth = connection.get("auth")
    auth_type = auth.get("type") if isinstance(auth, dict) else None
    if auth_type not in {"password", "private_key"}:
        raise AcceptanceError("AUTH_INVALID", "SSH authentication type is not supported by acceptance")

    host_key = connection.get("host_key")
    known_hosts_file = host_key.get("known_hosts_file") if isinstance(host_key, dict) else None
    if not isinstance(known_hosts_file, str) or not known_hosts_file:
        raise AcceptanceError("KNOWN_HOSTS_MISSING", "strict known_hosts_file must be configured")
    if real_acceptance and not Path(known_hosts_file).is_file():
        raise AcceptanceError("KNOWN_HOSTS_UNREADABLE", "configured known_hosts_file is not a regular file")

    helper_image = program.replace("\\", "/").rsplit("/", 1)[-1]
    if real_acceptance and not helper_image.lower().endswith(".exe"):
        raise AcceptanceError(
            "HELPER_NOT_WINDOWS_EXE",
            "final WSL systemd acceptance requires a Windows .exe ProxyCommand helper",
        )

    target = {
        "source_id": source_id,
        "connection_id": connection_id,
        "host": host,
        "port": port,
        "auth_type": auth_type,
        "known_hosts_file": known_hosts_file,
        "program": program,
        "args": list(args),
        "helper_image": helper_image,
    }
    return config, target


def systemd_snapshot(systemctl_bin: str, service_name: str, expected_user: str) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            [
                systemctl_bin,
                "show",
                service_name,
                "--property=ActiveState",
                "--property=User",
                "--property=MainPID",
                "--no-pager",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=10,
            text=True,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise AcceptanceError("SYSTEMCTL_UNAVAILABLE", "systemctl could not inspect the production service") from exc
    if completed.returncode != 0:
        raise AcceptanceError("SYSTEMD_LOOKUP_FAILED", "systemctl could not inspect the production service")

    values: dict[str, str] = {}
    for line in completed.stdout.splitlines():
        key, sep, value = line.partition("=")
        if sep:
            values[key] = value
    if values.get("ActiveState") != "active":
        raise AcceptanceError("SERVICE_NOT_ACTIVE", "production log-query-mcp service is not active")
    if values.get("User") != expected_user:
        raise AcceptanceError(
            "SERVICE_IDENTITY_MISMATCH",
            "production service User does not match the expected service identity",
        )
    try:
        main_pid = int(values.get("MainPID", "0"))
    except ValueError as exc:
        raise AcceptanceError("SERVICE_PID_INVALID", "production service MainPID is invalid") from exc
    if main_pid <= 0:
        raise AcceptanceError("SERVICE_PID_INVALID", "production service has no live MainPID")
    return {"active_state": "active", "user": expected_user, "main_pid": main_pid}


def windows_process_count(tasklist_bin: str, image_name: str) -> int:
    try:
        completed = subprocess.run(
            [tasklist_bin, "/FO", "CSV", "/NH", "/FI", f"IMAGENAME eq {image_name}"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise AcceptanceError(
            "TASKLIST_UNAVAILABLE",
            "Windows tasklist could not be executed from WSL; Windows interop is required",
        ) from exc
    if completed.returncode != 0:
        raise AcceptanceError("TASKLIST_FAILED", "Windows tasklist returned a failure status")

    decoded = completed.stdout.decode(errors="replace")
    count = 0
    for row in csv.reader(decoded.splitlines()):
        if row and row[0].strip().casefold() == image_name.casefold():
            count += 1
    return count


def wait_for_helper_baseline(tasklist_bin: str, helper_image: str, baseline: int) -> int:
    last = windows_process_count(tasklist_bin, helper_image)
    for _ in range(20):
        if last <= baseline:
            return last
        time.sleep(0.25)
        last = windows_process_count(tasklist_bin, helper_image)
    return last


def parse_http_payload(body: bytes, content_type: str) -> dict[str, Any] | None:
    if not body.strip():
        return None
    text = body.decode("utf-8", errors="strict")
    if "text/event-stream" in content_type.lower():
        candidates: list[dict[str, Any]] = []
        for line in text.splitlines():
            if not line.startswith("data:"):
                continue
            raw = line[5:].strip()
            if not raw:
                continue
            try:
                value = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if isinstance(value, dict):
                candidates.append(value)
        if not candidates:
            raise AcceptanceError("HTTP_SSE_INVALID", "MCP HTTP response contained no JSON SSE data")
        return candidates[-1]
    try:
        value = json.loads(text)
    except json.JSONDecodeError as exc:
        raise AcceptanceError("HTTP_JSON_INVALID", "MCP HTTP response is not valid JSON") from exc
    if not isinstance(value, dict):
        raise AcceptanceError("HTTP_JSON_INVALID", "MCP HTTP response JSON is not an object")
    return value


class McpHttpClient:
    def __init__(self, url: str, timeout_seconds: float) -> None:
        self.url = url
        self.timeout_seconds = timeout_seconds
        self.session_id: str | None = None

    def post(self, value: dict[str, Any], *, allow_empty: bool = False) -> dict[str, Any] | None:
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if self.session_id:
            headers["Mcp-Session-Id"] = self.session_id
        request = urllib.request.Request(
            self.url,
            data=json.dumps(value, separators=(",", ":")).encode("utf-8"),
            headers=headers,
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout_seconds) as response:
                body = response.read(MAX_HTTP_RESPONSE_BYTES + 1)
                if len(body) > MAX_HTTP_RESPONSE_BYTES:
                    raise AcceptanceError("HTTP_RESPONSE_TOO_LARGE", "MCP HTTP response exceeded acceptance limit")
                response_session = response.headers.get("Mcp-Session-Id")
                if response_session:
                    self.session_id = response_session
                payload = parse_http_payload(body, response.headers.get("Content-Type", ""))
        except urllib.error.HTTPError as exc:
            raise AcceptanceError("HTTP_STATUS_ERROR", "MCP HTTP endpoint returned a failure status") from exc
        except (urllib.error.URLError, TimeoutError, socket.timeout, OSError) as exc:
            raise AcceptanceError("HTTP_TRANSPORT_ERROR", "MCP HTTP endpoint request failed") from exc
        if payload is None and not allow_empty:
            raise AcceptanceError("HTTP_EMPTY_RESPONSE", "MCP HTTP endpoint returned an empty response")
        return payload

    def request(self, request_id: int, method: str, params: dict[str, Any]) -> dict[str, Any]:
        payload = self.post(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        )
        if not isinstance(payload, dict):
            raise AcceptanceError("MCP_RESULT_INVALID", f"MCP result is missing for {method}")
        if payload.get("id") != request_id:
            raise AcceptanceError("MCP_RESPONSE_ID_MISMATCH", f"MCP response id mismatch for {method}")
        if "error" in payload:
            raise AcceptanceError("MCP_JSONRPC_ERROR", f"MCP returned JSON-RPC error for {method}")
        if not isinstance(payload.get("result"), dict):
            raise AcceptanceError("MCP_RESULT_INVALID", f"MCP result is missing for {method}")
        return payload

    def notify_initialized(self) -> None:
        self.post(
            {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
            allow_empty=True,
        )


def parse_tool_result(response: dict[str, Any], tool_name: str) -> dict[str, Any]:
    result = response.get("result")
    if not isinstance(result, dict):
        raise AcceptanceError("MCP_TOOL_RESULT_INVALID", f"MCP tool {tool_name} returned invalid result")
    if result.get("isError") is True or result.get("is_error") is True:
        raise AcceptanceError("MCP_TOOL_ERROR", f"MCP tool {tool_name} returned a sanitized tool error")
    content = result.get("content")
    if not isinstance(content, list):
        raise AcceptanceError("MCP_TOOL_RESULT_INVALID", f"MCP tool {tool_name} returned invalid content")
    text = next(
        (
            item.get("text")
            for item in content
            if isinstance(item, dict) and item.get("type") == "text" and isinstance(item.get("text"), str)
        ),
        None,
    )
    if text is None:
        raise AcceptanceError("MCP_TOOL_TEXT_MISSING", f"MCP tool {tool_name} returned no text payload")
    try:
        value = json.loads(text)
    except json.JSONDecodeError as exc:
        raise AcceptanceError("MCP_TOOL_JSON_INVALID", f"MCP tool {tool_name} payload is not JSON") from exc
    if not isinstance(value, dict):
        raise AcceptanceError("MCP_TOOL_JSON_INVALID", f"MCP tool {tool_name} payload is not an object")
    return value


def call_tool(client: McpHttpClient, request_id: int, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
    return parse_tool_result(
        client.request(
            request_id,
            "tools/call",
            {"name": name, "arguments": arguments},
        ),
        name,
    )


def write_evidence(directory: Path, evidence: dict[str, Any]) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    path = directory / f"m7-wsl-http-acceptance-{timestamp}.json"
    path.write_text(json.dumps(evidence, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    try:
        path.chmod(0o600)
    except OSError:
        pass
    return path


def run_real(args: argparse.Namespace, config_path: Path, target: dict[str, Any]) -> tuple[dict[str, Any], Path]:
    if not args.keyword:
        raise AcceptanceError("KEYWORD_REQUIRED", "real systemd HTTP acceptance requires --keyword")
    if args.before_lines < 0 or args.after_lines < 0:
        raise AcceptanceError("CONTEXT_RANGE_INVALID", "context line counts must be non-negative")
    if args.timeout_seconds <= 0:
        raise AcceptanceError("TIMEOUT_INVALID", "timeout must be positive")

    systemd = systemd_snapshot(args.systemctl_bin, args.service_name, args.expected_service_user)
    helper_before = windows_process_count(args.tasklist_bin, target["helper_image"])
    evidence: dict[str, Any] = {
        "acceptance": "m7-wsl-systemd-http",
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "status": "FAIL",
        "config_sha256": sha256_file(config_path),
        "source_id": target["source_id"],
        "connection_id": target["connection_id"],
        "logical_host_sha256": sha256_text(target["host"]),
        "target_port": target["port"],
        "auth_type": target["auth_type"],
        "helper_image": target["helper_image"],
        "proxy_argv_shape": [
            item if item in {"{host}", "{port}"} else "<literal>" for item in target["args"]
        ],
        "keyword_sha256": sha256_text(args.keyword),
        "endpoint_sha256": sha256_text(args.url),
        "service": systemd,
        "buildinfo": read_buildinfo(Path(args.buildinfo)),
        "helper_process_count_before": helper_before,
        "checks": {},
    }

    try:
        client = McpHttpClient(args.url, args.timeout_seconds)
        initialized = client.request(
            1,
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "m7-wsl-http-acceptance", "version": "0.1.0"},
            },
        )
        server_info = initialized["result"].get("serverInfo")
        if not isinstance(server_info, dict) or server_info.get("name") != "log-query-mcp":
            raise AcceptanceError("WRONG_MCP_SERVER", "HTTP endpoint is not log-query-mcp")
        evidence["checks"]["initialize"] = "PASS"

        client.notify_initialized()
        tools_response = client.request(2, "tools/list", {})
        tools = tools_response["result"].get("tools")
        if not isinstance(tools, list):
            raise AcceptanceError("TOOLS_LIST_INVALID", "tools/list did not return tools")
        tool_names = {
            item.get("name") for item in tools if isinstance(item, dict) and isinstance(item.get("name"), str)
        }
        if tool_names != EXPECTED_TOOLS:
            raise AcceptanceError("TOOLS_SURFACE_CHANGED", "MCP tool surface is not the expected three tools")
        evidence["checks"]["tools_list"] = "PASS"

        sources = call_tool(client, 3, "list_log_sources", {})
        source_items = sources.get("sources")
        if not isinstance(source_items, list) or not any(
            isinstance(item, dict) and item.get("source_id") == target["source_id"] for item in source_items
        ):
            raise AcceptanceError("SOURCE_NOT_LISTED", "selected ProxyCommand source is not listed by MCP")
        evidence["checks"]["list_log_sources"] = "PASS"

        search = call_tool(
            client,
            4,
            "search_logs",
            {
                "source_ids": [target["source_id"]],
                "keyword": args.keyword,
                "case_sensitive": True,
                "order": "oldest_first",
                "max_results": 10,
            },
        )
        results = search.get("results")
        if not isinstance(results, list) or not results:
            raise AcceptanceError("SEARCH_NO_MATCH", "search_logs returned no acceptance marker match")
        selected = next(
            (
                item
                for item in results
                if isinstance(item, dict)
                and item.get("source_id") == target["source_id"]
                and isinstance(item.get("content"), str)
                and args.keyword in item["content"]
                and isinstance(item.get("match_ref"), str)
            ),
            None,
        )
        if selected is None:
            raise AcceptanceError("SEARCH_NO_MATCH", "search_logs did not return the expected source marker")
        evidence["checks"]["search_logs"] = "PASS"
        evidence["search_result_count"] = len(results)

        context = call_tool(
            client,
            5,
            "get_log_context",
            {
                "match_ref": selected["match_ref"],
                "before_lines": args.before_lines,
                "after_lines": args.after_lines,
            },
        )
        lines = context.get("lines")
        if not isinstance(lines, list) or not any(
            isinstance(item, dict)
            and isinstance(item.get("content"), str)
            and args.keyword in item["content"]
            for item in lines
        ):
            raise AcceptanceError("CONTEXT_MARKER_MISSING", "get_log_context did not contain the acceptance marker")
        evidence["checks"]["get_log_context"] = "PASS"
        evidence["context_line_count"] = len(lines)

        helper_after = wait_for_helper_baseline(args.tasklist_bin, target["helper_image"], helper_before)
        evidence["helper_process_count_after"] = helper_after
        if helper_after > helper_before:
            raise AcceptanceError("HELPER_PROCESS_LEAK", "ProxyCommand helper process count did not return to baseline")
        evidence["checks"]["helper_cleanup"] = "PASS"
        evidence["status"] = "PASS"
        evidence["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    except AcceptanceError as exc:
        evidence["failure_code"] = exc.code
        evidence["failure_message"] = exc.safe_message
        evidence["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
        try:
            evidence["helper_process_count_after"] = windows_process_count(
                args.tasklist_bin, target["helper_image"]
            )
        except AcceptanceError:
            pass
        path = write_evidence(Path(args.evidence_dir), evidence)
        raise AcceptanceError(exc.code, f"{exc.safe_message}; evidence={path}") from exc

    path = write_evidence(Path(args.evidence_dir), evidence)
    return evidence, path


def main() -> int:
    args = parse_args()
    config_path = Path(args.config)
    try:
        _, target = load_target(config_path, args.source_id, real_acceptance=not args.validate_config_only)
        if args.validate_config_only:
            print("m7_wsl_http_acceptance: config validation PASS")
            return 0
        evidence, path = run_real(args, config_path, target)
        if evidence.get("status") != "PASS":
            raise AcceptanceError("ACCEPTANCE_FAILED", "systemd HTTP acceptance did not pass")
        print(f"m7_wsl_http_acceptance: PASS evidence={path}")
        return 0
    except AcceptanceError as exc:
        print(f"m7_wsl_http_acceptance: {exc.code}: {exc.safe_message}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
