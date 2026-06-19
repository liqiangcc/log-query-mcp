#!/usr/bin/env python3
"""Independent JSON-RPC smoke test for the log-query MCP stdio server.

This client intentionally does not use an MCP SDK. It verifies newline-delimited
stdio framing, lifecycle initialization, tool discovery, structured tool output,
pagination, and context lookup against a temporary real log source.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any, TextIO

PROTOCOL_VERSION = "2025-06-18"
DEFAULT_TIMEOUT_SECONDS = 10.0
EXPECTED_TOOLS = {"list_log_sources", "search_logs", "get_log_context"}


class SmokeFailure(RuntimeError):
    """Raised when the server violates the expected MCP smoke-test contract."""


class JsonRpcReader:
    def __init__(self, stream: TextIO) -> None:
        self._messages: queue.Queue[str | None] = queue.Queue()
        self._thread = threading.Thread(
            target=self._drain,
            args=(stream,),
            name="mcp-stdout-reader",
            daemon=True,
        )
        self._thread.start()

    def _drain(self, stream: TextIO) -> None:
        try:
            for line in stream:
                self._messages.put(line.rstrip("\n"))
        finally:
            self._messages.put(None)

    def wait_for_response(self, request_id: int, timeout: float) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise SmokeFailure(f"timed out waiting for JSON-RPC response {request_id}")
            try:
                raw = self._messages.get(timeout=remaining)
            except queue.Empty as error:
                raise SmokeFailure(
                    f"timed out waiting for JSON-RPC response {request_id}"
                ) from error
            if raw is None:
                raise SmokeFailure(
                    f"server closed stdout before responding to request {request_id}"
                )
            if not raw:
                continue
            try:
                message = json.loads(raw)
            except json.JSONDecodeError as error:
                raise SmokeFailure(f"server stdout contained non-JSON data: {raw!r}") from error
            if not isinstance(message, dict):
                raise SmokeFailure(f"server emitted a non-object JSON-RPC message: {message!r}")
            if message.get("id") == request_id:
                return message
            if "id" in message and "method" in message:
                raise SmokeFailure(
                    "server sent an unexpected request even though the smoke client declared "
                    f"no optional client capabilities: {message!r}"
                )
            # Notifications are legal. Ignore them while waiting for our response.


def send_message(process: subprocess.Popen[str], message: dict[str, Any]) -> None:
    if process.stdin is None:
        raise SmokeFailure("server stdin is not available")
    process.stdin.write(json.dumps(message, ensure_ascii=False, separators=(",", ":")))
    process.stdin.write("\n")
    process.stdin.flush()


def request(
    process: subprocess.Popen[str],
    reader: JsonRpcReader,
    request_id: int,
    method: str,
    params: dict[str, Any],
    timeout: float,
) -> dict[str, Any]:
    send_message(
        process,
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        },
    )
    response = reader.wait_for_response(request_id, timeout)
    if response.get("jsonrpc") != "2.0":
        raise SmokeFailure(f"response {request_id} omitted jsonrpc=2.0: {response!r}")
    if "error" in response:
        raise SmokeFailure(f"request {method} failed: {response['error']!r}")
    result = response.get("result")
    if not isinstance(result, dict):
        raise SmokeFailure(f"request {method} returned no object result: {response!r}")
    return result


def tool_call(
    process: subprocess.Popen[str],
    reader: JsonRpcReader,
    request_id: int,
    name: str,
    arguments: dict[str, Any],
    timeout: float,
) -> dict[str, Any]:
    result = request(
        process,
        reader,
        request_id,
        "tools/call",
        {"name": name, "arguments": arguments},
        timeout,
    )
    if result.get("isError", False):
        raise SmokeFailure(f"tool {name} returned an execution error: {result!r}")
    return extract_structured_content(result, name)


def extract_structured_content(result: dict[str, Any], tool_name: str) -> dict[str, Any]:
    structured = result.get("structuredContent")
    if isinstance(structured, dict):
        return structured

    # MCP recommends including serialized JSON text for backwards compatibility.
    content = result.get("content")
    if isinstance(content, list):
        for item in content:
            if not isinstance(item, dict) or item.get("type") != "text":
                continue
            text = item.get("text")
            if not isinstance(text, str):
                continue
            try:
                parsed = json.loads(text)
            except json.JSONDecodeError:
                continue
            if isinstance(parsed, dict):
                return parsed

    raise SmokeFailure(
        f"tool {tool_name} returned neither structuredContent nor JSON text: {result!r}"
    )


def write_fixture(directory: Path) -> Path:
    log_path = directory / "application.log"
    log_path.write_text(
        """2026-06-19T14:20:01+09:00 INFO before request\n"
        "2026-06-19T14:20:02+09:00 ERROR traceId=abc123 first failure\n"
        "    at payment::authorize(payment.rs:42)\n"
        "2026-06-19T14:20:03+09:00 ERROR traceId=abc123 second failure\n""",
        encoding="utf-8",
    )
    config_path = directory / "log-query-mcp.json"
    config_path.write_text(
        json.dumps(
            {
                "sources": [
                    {
                        "source_id": "payment-test",
                        "name": "Payment test",
                        "description": "Protocol smoke fixture",
                        "service": "payment-service",
                        "environment": "test",
                        "tags": ["smoke"],
                        "root": ".",
                        "files": ["application.log"],
                        "timestamp_rule": {
                            "type": "rfc3339",
                            "prefix_bytes": 64,
                        },
                    }
                ]
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    return config_path


def validate_tool_schema(tools: list[dict[str, Any]]) -> None:
    by_name = {
        tool.get("name"): tool
        for tool in tools
        if isinstance(tool, dict) and isinstance(tool.get("name"), str)
    }
    missing = EXPECTED_TOOLS.difference(by_name)
    if missing:
        raise SmokeFailure(f"tools/list omitted expected tools: {sorted(missing)!r}")

    search_schema = by_name["search_logs"].get("inputSchema")
    if not isinstance(search_schema, dict):
        raise SmokeFailure("search_logs has no inputSchema object")
    properties = search_schema.get("properties")
    if not isinstance(properties, dict):
        raise SmokeFailure("search_logs inputSchema has no properties object")
    for field in ("source_ids", "keyword", "max_results", "cursor"):
        if field not in properties:
            raise SmokeFailure(f"search_logs schema omitted {field!r}")

    context_schema = by_name["get_log_context"].get("inputSchema")
    if not isinstance(context_schema, dict):
        raise SmokeFailure("get_log_context has no inputSchema object")
    context_properties = context_schema.get("properties")
    if not isinstance(context_properties, dict) or "match_ref" not in context_properties:
        raise SmokeFailure("get_log_context schema omitted match_ref")


def run_smoke(server: Path, timeout: float) -> dict[str, Any]:
    if not server.is_file():
        raise SmokeFailure(f"stdio server binary does not exist: {server}")

    with tempfile.TemporaryDirectory(prefix="log-query-mcp-stdio-smoke-") as temp:
        temp_dir = Path(temp)
        config_path = write_fixture(temp_dir)
        environment = os.environ.copy()
        environment["LOG_QUERY_MCP_CONFIG"] = str(config_path)
        environment.setdefault("RUST_LOG", "warn")

        process = subprocess.Popen(
            [str(server)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            bufsize=1,
            env=environment,
        )
        if process.stdout is None or process.stderr is None:
            process.kill()
            raise SmokeFailure("failed to create server pipes")

        stderr_lines: list[str] = []

        def drain_stderr() -> None:
            for line in process.stderr:
                stderr_lines.append(line.rstrip("\n"))

        stderr_thread = threading.Thread(
            target=drain_stderr,
            name="mcp-stderr-reader",
            daemon=True,
        )
        stderr_thread.start()
        reader = JsonRpcReader(process.stdout)

        try:
            initialize = request(
                process,
                reader,
                1,
                "initialize",
                {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "log-query-mcp-stdio-smoke",
                        "version": "0.1.0",
                    },
                },
                timeout,
            )
            if initialize.get("protocolVersion") != PROTOCOL_VERSION:
                raise SmokeFailure(
                    "server negotiated an unexpected protocol version: "
                    f"{initialize.get('protocolVersion')!r}"
                )
            capabilities = initialize.get("capabilities")
            if not isinstance(capabilities, dict) or "tools" not in capabilities:
                raise SmokeFailure("server did not declare the tools capability")

            send_message(
                process,
                {"jsonrpc": "2.0", "method": "notifications/initialized"},
            )

            tools_result = request(process, reader, 2, "tools/list", {}, timeout)
            tools = tools_result.get("tools")
            if not isinstance(tools, list):
                raise SmokeFailure("tools/list did not return a tools array")
            validate_tool_schema(tools)

            sources = tool_call(
                process,
                reader,
                3,
                "list_log_sources",
                {},
                timeout,
            )
            source_items = sources.get("sources")
            if not isinstance(source_items, list) or not any(
                isinstance(item, dict) and item.get("source_id") == "payment-test"
                for item in source_items
            ):
                raise SmokeFailure("list_log_sources did not expose payment-test")

            search_arguments: dict[str, Any] = {
                "source_ids": ["payment-test"],
                "keyword": "abc123",
                "case_sensitive": False,
                "start_time": None,
                "end_time": None,
                "order": "oldest_first",
                "max_results": 1,
                "cursor": None,
            }
            first_page = tool_call(
                process,
                reader,
                4,
                "search_logs",
                search_arguments,
                timeout,
            )
            first_results = first_page.get("results")
            if not isinstance(first_results, list) or len(first_results) != 1:
                raise SmokeFailure(f"first search page was unexpected: {first_page!r}")
            first_match = first_results[0]
            if not isinstance(first_match, dict) or "first failure" not in str(
                first_match.get("content", "")
            ):
                raise SmokeFailure(f"first search result was unexpected: {first_match!r}")
            match_ref = first_match.get("match_ref")
            if not isinstance(match_ref, str) or not match_ref.startswith("mref_"):
                raise SmokeFailure("search result did not contain an opaque match_ref")
            next_cursor = first_page.get("next_cursor")
            if not isinstance(next_cursor, str) or not next_cursor:
                raise SmokeFailure("first search page did not return a continuation cursor")

            context = tool_call(
                process,
                reader,
                5,
                "get_log_context",
                {
                    "match_ref": match_ref,
                    "before_lines": 1,
                    "after_lines": 1,
                },
                timeout,
            )
            context_lines = context.get("lines")
            if not isinstance(context_lines, list) or len(context_lines) != 3:
                raise SmokeFailure(f"context response was unexpected: {context!r}")
            if "abc123" not in str(context_lines[1].get("content", "")):
                raise SmokeFailure("context middle line did not contain the search match")

            second_arguments = dict(search_arguments)
            second_arguments["cursor"] = next_cursor
            second_page = tool_call(
                process,
                reader,
                6,
                "search_logs",
                second_arguments,
                timeout,
            )
            second_results = second_page.get("results")
            if not isinstance(second_results, list) or len(second_results) != 1:
                raise SmokeFailure(f"second search page was unexpected: {second_page!r}")
            if "second failure" not in str(second_results[0].get("content", "")):
                raise SmokeFailure("second search page did not continue to the next match")

            return {
                "protocol_version": initialize["protocolVersion"],
                "server_name": initialize.get("serverInfo", {}).get("name"),
                "tool_names": sorted(
                    tool["name"]
                    for tool in tools
                    if isinstance(tool, dict) and isinstance(tool.get("name"), str)
                ),
                "source_ids": sorted(
                    item["source_id"]
                    for item in source_items
                    if isinstance(item, dict) and isinstance(item.get("source_id"), str)
                ),
                "first_match_line": first_match.get("line_number"),
                "context_line_count": len(context_lines),
                "pagination_verified": True,
            }
        except Exception as error:
            if stderr_lines:
                print("--- server stderr ---", file=sys.stderr)
                print("\n".join(stderr_lines[-100:]), file=sys.stderr)
            if isinstance(error, SmokeFailure):
                raise
            raise SmokeFailure(str(error)) from error
        finally:
            if process.stdin is not None and not process.stdin.closed:
                process.stdin.close()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--server",
        type=Path,
        default=Path("target/debug/log-query-mcp-stdio"),
        help="path to the already-built stdio server binary",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT_SECONDS,
        help="per-request timeout in seconds",
    )
    parser.add_argument(
        "--report",
        type=Path,
        help="optional path for the JSON smoke-test report",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        report = run_smoke(args.server.resolve(), args.timeout)
    except SmokeFailure as error:
        print(f"MCP stdio smoke test failed: {error}", file=sys.stderr)
        return 1

    encoded = json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True)
    print(encoded)
    if args.report is not None:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(encoded + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
