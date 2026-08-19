#!/usr/bin/env python3
"""Traceable real-WSL acceptance for M7 ProxyCommand.

The script deliberately uses only Python stdlib. It never prints or stores Secret
values or log content. A real acceptance run proves the WSL-specific path:

WSL direct TCP unavailable -> Windows .exe ProxyCommand -> SSH/SFTP -> MCP tools.

Use --validate-config-only for repository/package static checks. That mode performs
no network access and starts no process.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import platform
import queue
import socket
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

EXPECTED_TOOLS = {"list_log_sources", "search_logs", "get_log_context"}
PROTOCOL_VERSION = "2025-06-18"


class AcceptanceError(RuntimeError):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.safe_message = message


@dataclass(frozen=True)
class ProxyTarget:
    source_id: str
    connection_id: str
    host: str
    port: int
    auth_type: str
    known_hosts_file: str
    program: str
    args: list[str]
    helper_image: str


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run traceable M7 WSL -> Windows ProxyCommand -> SSH/SFTP acceptance."
    )
    parser.add_argument("--config", required=True, help="v2 config used by the acceptance run")
    parser.add_argument("--source-id", required=True, help="ProxyCommand-backed source to query")
    parser.add_argument("--keyword", help="known marker that must exist in the selected remote log")
    parser.add_argument(
        "--stdio-bin",
        default="/opt/log-query-mcp/bin/log-query-mcp-stdio",
        help="log-query-mcp-stdio binary to exercise",
    )
    parser.add_argument(
        "--buildinfo",
        default="/opt/log-query-mcp/BUILDINFO",
        help="optional installed BUILDINFO path used for traceability",
    )
    parser.add_argument(
        "--evidence-dir",
        default="m7-wsl-evidence",
        help="directory for redacted JSON evidence",
    )
    parser.add_argument(
        "--direct-timeout-seconds", type=float, default=3.0, help="WSL direct TCP probe timeout"
    )
    parser.add_argument(
        "--mcp-timeout-seconds", type=float, default=30.0, help="per MCP response timeout"
    )
    parser.add_argument("--before-lines", type=int, default=1)
    parser.add_argument("--after-lines", type=int, default=1)
    parser.add_argument(
        "--tasklist-bin",
        default="tasklist.exe",
        help="Windows tasklist executable used to prove helper cleanup",
    )
    parser.add_argument(
        "--allow-direct-reachable",
        action="store_true",
        help="do not fail if WSL can directly reach the SSH target; not valid for final WSL-path proof",
    )
    parser.add_argument(
        "--validate-config-only",
        action="store_true",
        help="validate the selected ProxyCommand config only; no WSL/network/process checks",
    )
    return parser.parse_args()


def load_target(config_path: Path, source_id: str, *, real_acceptance: bool) -> tuple[dict[str, Any], ProxyTarget]:
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
            "final WSL acceptance requires a Windows .exe ProxyCommand helper",
        )

    return config, ProxyTarget(
        source_id=source_id,
        connection_id=connection_id,
        host=host,
        port=port,
        auth_type=auth_type,
        known_hosts_file=known_hosts_file,
        program=program,
        args=list(args),
        helper_image=helper_image,
    )


def is_wsl() -> bool:
    if os.environ.get("WSL_INTEROP") or os.environ.get("WSL_DISTRO_NAME"):
        return True
    candidates = [platform.release()]
    try:
        candidates.append(Path("/proc/sys/kernel/osrelease").read_text(encoding="utf-8"))
    except OSError:
        pass
    return any("microsoft" in value.lower() for value in candidates)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def redact_proxy_args_shape(args: list[str]) -> list[str]:
    """Retain only structural ProxyCommand argv information in evidence."""
    return [item if item in {"{host}", "{port}"} else "<literal>" for item in args]


def direct_probe(host: str, port: int, timeout_seconds: float) -> tuple[bool, str]:
    try:
        with socket.create_connection((host, port), timeout=timeout_seconds):
            return True, "connected"
    except (OSError, socket.gaierror) as exc:
        return False, type(exc).__name__


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


def start_reader_thread(stream: Any, output: queue.Queue[dict[str, Any]]) -> threading.Thread:
    def reader() -> None:
        for line in stream:
            line = line.strip()
            if not line:
                continue
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                output.put({"__protocol_error__": True})
                return
            if isinstance(message, dict):
                output.put(message)

    thread = threading.Thread(target=reader, name="m7-wsl-mcp-stdout", daemon=True)
    thread.start()
    return thread


def drain_stderr(stream: Any) -> threading.Thread:
    def reader() -> None:
        # Intentionally discard raw stderr. Acceptance evidence must not persist
        # potentially sensitive diagnostics, paths, argv, or log content.
        for _ in stream:
            pass

    thread = threading.Thread(target=reader, name="m7-wsl-mcp-stderr", daemon=True)
    thread.start()
    return thread


def send_json(proc: subprocess.Popen[str], value: dict[str, Any]) -> None:
    if proc.stdin is None:
        raise AcceptanceError("MCP_STDIN_CLOSED", "MCP stdio stdin is unavailable")
    proc.stdin.write(json.dumps(value, separators=(",", ":")) + "\n")
    proc.stdin.flush()


def request(
    proc: subprocess.Popen[str],
    responses: queue.Queue[dict[str, Any]],
    request_id: int,
    method: str,
    params: dict[str, Any],
    timeout_seconds: float,
) -> dict[str, Any]:
    send_json(
        proc,
        {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params},
    )
    deadline = time.monotonic() + timeout_seconds
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise AcceptanceError("MCP_RESPONSE_TIMEOUT", f"MCP response timed out for {method}")
        try:
            message = responses.get(timeout=remaining)
        except queue.Empty as exc:
            raise AcceptanceError("MCP_RESPONSE_TIMEOUT", f"MCP response timed out for {method}") from exc
        if message.get("__protocol_error__"):
            raise AcceptanceError("MCP_STDOUT_INVALID", "MCP stdout contained a non-JSON protocol line")
        if message.get("id") != request_id:
            continue
        if "error" in message:
            raise AcceptanceError("MCP_JSONRPC_ERROR", f"MCP returned JSON-RPC error for {method}")
        if not isinstance(message.get("result"), dict):
            raise AcceptanceError("MCP_RESULT_INVALID", f"MCP result is missing for {method}")
        return message


def call_tool(
    proc: subprocess.Popen[str],
    responses: queue.Queue[dict[str, Any]],
    request_id: int,
    name: str,
    arguments: dict[str, Any],
    timeout_seconds: float,
) -> dict[str, Any]:
    response = request(
        proc,
        responses,
        request_id,
        "tools/call",
        {"name": name, "arguments": arguments},
        timeout_seconds,
    )
    result = response["result"]
    if result.get("isError") is True or result.get("is_error") is True:
        raise AcceptanceError("MCP_TOOL_ERROR", f"MCP tool {name} returned a sanitized tool error")
    content = result.get("content")
    if not isinstance(content, list):
        raise AcceptanceError("MCP_TOOL_RESULT_INVALID", f"MCP tool {name} returned invalid content")
    text = next(
        (
            item.get("text")
            for item in content
            if isinstance(item, dict) and item.get("type") == "text" and isinstance(item.get("text"), str)
        ),
        None,
    )
    if text is None:
        raise AcceptanceError("MCP_TOOL_TEXT_MISSING", f"MCP tool {name} returned no text payload")
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        raise AcceptanceError("MCP_TOOL_JSON_INVALID", f"MCP tool {name} payload is not JSON") from exc
    if not isinstance(payload, dict):
        raise AcceptanceError("MCP_TOOL_JSON_INVALID", f"MCP tool {name} payload is not an object")
    return payload


def terminate_process(proc: subprocess.Popen[str] | None) -> None:
    if proc is None:
        return
    try:
        if proc.stdin is not None:
            proc.stdin.close()
    except OSError:
        pass
    try:
        proc.terminate()
        proc.wait(timeout=5)
    except (OSError, subprocess.TimeoutExpired):
        try:
            proc.kill()
            proc.wait(timeout=5)
        except (OSError, subprocess.TimeoutExpired):
            pass


def wait_for_helper_baseline(tasklist_bin: str, helper_image: str, baseline: int) -> int:
    last = windows_process_count(tasklist_bin, helper_image)
    for _ in range(10):
        if last <= baseline:
            return last
        time.sleep(0.25)
        last = windows_process_count(tasklist_bin, helper_image)
    return last


def write_evidence(directory: Path, evidence: dict[str, Any]) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    path = directory / f"m7-wsl-acceptance-{timestamp}.json"
    payload = json.dumps(evidence, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    path.write_text(payload, encoding="utf-8")
    try:
        path.chmod(0o600)
    except OSError:
        pass
    return path


def validate_only(config_path: Path, source_id: str) -> int:
    _, target = load_target(config_path, source_id, real_acceptance=False)
    print(
        json.dumps(
            {
                "status": "VALID",
                "source_id": target.source_id,
                "connection_id": target.connection_id,
                "proxy_type": "command",
                "placeholders": sorted(item for item in target.args if item in {"{host}", "{port}"}),
            },
            separators=(",", ":"),
        )
    )
    return 0


def run_acceptance(args: argparse.Namespace) -> int:
    config_path = Path(args.config)
    _, target = load_target(config_path, args.source_id, real_acceptance=True)
    if not args.keyword:
        raise AcceptanceError("KEYWORD_REQUIRED", "--keyword is required for a real acceptance run")
    if not is_wsl():
        raise AcceptanceError("NOT_WSL", "real M7 WSL acceptance must run inside WSL")
    if args.before_lines < 0 or args.after_lines < 0:
        raise AcceptanceError("CONTEXT_LINES_INVALID", "context line counts must be non-negative")

    stdio_bin = Path(args.stdio_bin)
    if not stdio_bin.is_file() or not os.access(stdio_bin, os.X_OK):
        raise AcceptanceError("STDIO_BINARY_MISSING", "log-query-mcp-stdio binary is not executable")

    evidence: dict[str, Any] = {
        "schema": "log-query-mcp-m7-wsl-acceptance-v1",
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "status": "RUNNING",
        "source_id": target.source_id,
        "connection_id": target.connection_id,
        "target_host_sha256": sha256_text(target.host),
        "target_port": target.port,
        "auth_type": target.auth_type,
        "proxy_program_basename": target.helper_image,
        "proxy_args_shape": redact_proxy_args_shape(target.args),
        "config_sha256": sha256_file(config_path),
        "stdio_binary_sha256": sha256_file(stdio_bin),
        "keyword_sha256": sha256_text(args.keyword),
        "wsl_distro_name": os.environ.get("WSL_DISTRO_NAME", "unknown"),
        "kernel_release": platform.release(),
        "buildinfo": read_buildinfo(Path(args.buildinfo)),
    }

    proc: subprocess.Popen[str] | None = None
    helper_before: int | None = None
    responses: queue.Queue[dict[str, Any]] = queue.Queue()
    try:
        helper_before = windows_process_count(args.tasklist_bin, target.helper_image)
        evidence["helper_process_count_before"] = helper_before

        direct_reachable, direct_detail = direct_probe(
            target.host, target.port, args.direct_timeout_seconds
        )
        evidence["direct_wsl_tcp_reachable"] = direct_reachable
        evidence["direct_probe_result_class"] = direct_detail
        if direct_reachable and not args.allow_direct_reachable:
            raise AcceptanceError(
                "DIRECT_PATH_REACHABLE",
                "WSL can directly reach the SSH target; this run does not prove the required host-network gap",
            )
        evidence["direct_path_requirement_met"] = not direct_reachable

        child_env = os.environ.copy()
        child_env["LOG_QUERY_MCP_CONFIG"] = str(config_path)
        proc = subprocess.Popen(
            [str(stdio_bin)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            env=child_env,
        )
        assert proc.stdout is not None
        assert proc.stderr is not None
        start_reader_thread(proc.stdout, responses)
        drain_stderr(proc.stderr)

        initialized = request(
            proc,
            responses,
            1,
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "m7-wsl-acceptance", "version": "1"},
            },
            args.mcp_timeout_seconds,
        )
        server_name = initialized["result"].get("serverInfo", {}).get("name")
        if server_name != "log-query-mcp":
            raise AcceptanceError("MCP_SERVER_IDENTITY_INVALID", "MCP serverInfo identity is unexpected")
        evidence["initialize"] = "PASS"

        send_json(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools_response = request(
            proc, responses, 2, "tools/list", {}, args.mcp_timeout_seconds
        )
        tools = tools_response["result"].get("tools")
        if not isinstance(tools, list):
            raise AcceptanceError("TOOLS_LIST_INVALID", "tools/list response is invalid")
        tool_names = {
            item.get("name") for item in tools if isinstance(item, dict) and isinstance(item.get("name"), str)
        }
        if tool_names != EXPECTED_TOOLS:
            raise AcceptanceError("TOOL_SURFACE_CHANGED", "MCP tool surface is not exactly the expected three tools")
        evidence["tools"] = sorted(tool_names)

        sources = call_tool(
            proc, responses, 3, "list_log_sources", {}, args.mcp_timeout_seconds
        )
        source_items = sources.get("sources")
        if not isinstance(source_items, list) or not any(
            isinstance(item, dict) and item.get("source_id") == target.source_id for item in source_items
        ):
            raise AcceptanceError("SOURCE_NOT_LISTED", "list_log_sources did not expose the selected source_id")
        evidence["list_log_sources"] = "PASS"

        search = call_tool(
            proc,
            responses,
            4,
            "search_logs",
            {"source_ids": [target.source_id], "keyword": args.keyword, "max_results": 10},
            args.mcp_timeout_seconds,
        )
        results = search.get("results")
        if not isinstance(results, list) or not results:
            raise AcceptanceError("SEARCH_NO_MATCH", "search_logs returned no match for the acceptance marker")
        selected = next(
            (
                item
                for item in results
                if isinstance(item, dict)
                and item.get("source_id") == target.source_id
                and isinstance(item.get("match_ref"), str)
            ),
            None,
        )
        if selected is None:
            raise AcceptanceError("SEARCH_RESULT_INVALID", "search_logs returned no usable match_ref for the selected source")
        evidence["search_logs"] = "PASS"
        evidence["search_result_count"] = len(results)

        context = call_tool(
            proc,
            responses,
            5,
            "get_log_context",
            {
                "match_ref": selected["match_ref"],
                "before_lines": args.before_lines,
                "after_lines": args.after_lines,
            },
            args.mcp_timeout_seconds,
        )
        lines = context.get("lines")
        if context.get("source_id") != target.source_id or not isinstance(lines, list) or not lines:
            raise AcceptanceError("CONTEXT_INVALID", "get_log_context did not return context for the selected source")
        if not any(
            isinstance(line, dict)
            and isinstance(line.get("content"), str)
            and args.keyword in line["content"]
            for line in lines
        ):
            raise AcceptanceError("CONTEXT_MARKER_MISSING", "get_log_context did not contain the acceptance marker")
        evidence["get_log_context"] = "PASS"
        evidence["context_line_count"] = len(lines)

        evidence["proxy_ssh_sftp_path"] = "PASS"
        evidence["status"] = "PASS"
    except AcceptanceError as exc:
        evidence["status"] = "FAIL"
        evidence["failure_code"] = exc.code
        evidence["failure_message"] = exc.safe_message
    except (OSError, subprocess.SubprocessError) as exc:
        evidence["status"] = "FAIL"
        evidence["failure_code"] = "LOCAL_PROCESS_FAILURE"
        evidence["failure_message"] = "local acceptance process failed"
    finally:
        terminate_process(proc)
        if helper_before is not None:
            try:
                helper_after = wait_for_helper_baseline(
                    args.tasklist_bin, target.helper_image, helper_before
                )
                evidence["helper_process_count_after"] = helper_after
                evidence["helper_cleanup"] = "PASS" if helper_after <= helper_before else "FAIL"
                if helper_after > helper_before:
                    evidence["status"] = "FAIL"
                    evidence["failure_code"] = "HELPER_PROCESS_LEAK"
                    evidence["failure_message"] = "Windows ProxyCommand helper count did not return to baseline"
            except AcceptanceError as exc:
                evidence["helper_cleanup"] = "FAIL"
                evidence["status"] = "FAIL"
                evidence["failure_code"] = exc.code
                evidence["failure_message"] = exc.safe_message
        evidence["finished_at_utc"] = datetime.now(timezone.utc).isoformat()

    evidence_path = write_evidence(Path(args.evidence_dir), evidence)
    print(f"m7_wsl_acceptance: {evidence['status']}")
    print(f"m7_wsl_acceptance: evidence={evidence_path}")
    if evidence["status"] != "PASS":
        print(
            f"m7_wsl_acceptance: failure={evidence.get('failure_code', 'UNKNOWN')}",
            file=sys.stderr,
        )
        return 1
    return 0


def main() -> int:
    args = parse_args()
    config_path = Path(args.config)
    if args.validate_config_only:
        try:
            return validate_only(config_path, args.source_id)
        except AcceptanceError as exc:
            print(f"m7_wsl_acceptance: {exc.code}: {exc.safe_message}", file=sys.stderr)
            return 1
    try:
        return run_acceptance(args)
    except AcceptanceError as exc:
        print(f"m7_wsl_acceptance: {exc.code}: {exc.safe_message}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())