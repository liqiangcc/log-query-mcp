#!/usr/bin/env python3
"""Independent Streamable HTTP smoke test for the log-query MCP server.

The client intentionally uses only Python's standard library rather than an MCP
SDK. It verifies HTTP framing, initialization, session propagation, protocol
version headers, tool discovery, real log search, pagination, context lookup,
and graceful SIGTERM shutdown.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any, TextIO

import mcp_stdio_smoke as shared

PROTOCOL_VERSION = shared.PROTOCOL_VERSION
DEFAULT_TIMEOUT_SECONDS = shared.DEFAULT_TIMEOUT_SECONDS


class HttpSmokeFailure(shared.SmokeFailure):
    """Raised when the Streamable HTTP server violates the smoke contract."""


class StreamableHttpClient:
    def __init__(self, host: str, port: int, endpoint: str, timeout: float) -> None:
        self.host = host
        self.port = port
        self.endpoint = endpoint
        self.timeout = timeout
        self.session_id: str | None = None
        self.initialized = False
        self.response_content_types: set[str] = set()

    def request(
        self,
        request_id: int,
        method: str,
        params: dict[str, Any],
    ) -> dict[str, Any]:
        message = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        }
        response = self._post(message, expected_request_id=request_id)
        if response.get("jsonrpc") != "2.0":
            raise HttpSmokeFailure(
                f"response {request_id} omitted jsonrpc=2.0: {response!r}"
            )
        if "error" in response:
            raise HttpSmokeFailure(f"request {method} failed: {response['error']!r}")
        result = response.get("result")
        if not isinstance(result, dict):
            raise HttpSmokeFailure(
                f"request {method} returned no object result: {response!r}"
            )
        return result

    def notification(self, method: str, params: dict[str, Any] | None = None) -> None:
        message: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            message["params"] = params
        self._post(message, expected_request_id=None)

    def tool_call(
        self,
        request_id: int,
        name: str,
        arguments: dict[str, Any],
    ) -> dict[str, Any]:
        result = self.request(
            request_id,
            "tools/call",
            {"name": name, "arguments": arguments},
        )
        if result.get("isError", False):
            raise HttpSmokeFailure(f"tool {name} returned an execution error: {result!r}")
        return shared.extract_structured_content(result, name)

    def close_session(self) -> int | None:
        if self.session_id is None:
            return None
        connection = self._connection()
        try:
            connection.request(
                "DELETE",
                self.endpoint,
                headers=self._headers(include_content_type=False),
            )
            response = connection.getresponse()
            response.read()
            if response.status not in {200, 202, 204, 405}:
                raise HttpSmokeFailure(
                    f"session DELETE returned unexpected HTTP status {response.status}"
                )
            return response.status
        finally:
            connection.close()

    def _post(
        self,
        message: dict[str, Any],
        expected_request_id: int | None,
    ) -> dict[str, Any]:
        encoded = json.dumps(
            message,
            ensure_ascii=False,
            separators=(",", ":"),
        ).encode("utf-8")
        connection = self._connection()
        try:
            connection.request(
                "POST",
                self.endpoint,
                body=encoded,
                headers=self._headers(include_content_type=True),
            )
            response = connection.getresponse()
            body = response.read()
            session_id = response.getheader("Mcp-Session-Id")
            if session_id is not None:
                if self.session_id is not None and self.session_id != session_id:
                    raise HttpSmokeFailure("server changed the MCP session ID unexpectedly")
                self.session_id = session_id

            if expected_request_id is None:
                if response.status != 202:
                    raise HttpSmokeFailure(
                        "notification POST must return HTTP 202, got "
                        f"{response.status}: {body!r}"
                    )
                if body.strip():
                    raise HttpSmokeFailure(
                        f"notification POST returned an unexpected body: {body!r}"
                    )
                return {}

            if response.status != 200:
                raise HttpSmokeFailure(
                    f"request POST returned HTTP {response.status}: {body!r}"
                )
            content_type = response.getheader("Content-Type", "")
            media_type = content_type.split(";", 1)[0].strip().lower()
            self.response_content_types.add(media_type or "<missing>")
            messages = decode_http_messages(media_type, body)
            for candidate in messages:
                if candidate.get("id") == expected_request_id:
                    return candidate
                if "id" in candidate and "method" in candidate:
                    raise HttpSmokeFailure(
                        "server sent an unexpected request even though the client declared "
                        f"no optional capabilities: {candidate!r}"
                    )
            raise HttpSmokeFailure(
                f"HTTP response omitted JSON-RPC response {expected_request_id}: {messages!r}"
            )
        finally:
            connection.close()

    def _connection(self) -> http.client.HTTPConnection:
        return http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)

    def _headers(self, include_content_type: bool) -> dict[str, str]:
        headers = {
            "Accept": "application/json, text/event-stream",
            "Connection": "close",
        }
        if include_content_type:
            headers["Content-Type"] = "application/json"
        if self.session_id is not None:
            headers["Mcp-Session-Id"] = self.session_id
        if self.initialized:
            headers["MCP-Protocol-Version"] = PROTOCOL_VERSION
        return headers


def decode_http_messages(media_type: str, body: bytes) -> list[dict[str, Any]]:
    try:
        text = body.decode("utf-8")
    except UnicodeDecodeError as error:
        raise HttpSmokeFailure("HTTP response was not valid UTF-8") from error

    if media_type == "application/json":
        try:
            message = json.loads(text)
        except json.JSONDecodeError as error:
            raise HttpSmokeFailure(f"HTTP JSON response was invalid: {text!r}") from error
        if not isinstance(message, dict):
            raise HttpSmokeFailure(f"HTTP JSON response was not an object: {message!r}")
        return [message]

    if media_type == "text/event-stream":
        messages: list[dict[str, Any]] = []
        data_lines: list[str] = []

        def finish_event() -> None:
            if not data_lines:
                return
            payload = "\n".join(data_lines)
            data_lines.clear()
            try:
                message = json.loads(payload)
            except json.JSONDecodeError as error:
                raise HttpSmokeFailure(f"SSE data was not valid JSON: {payload!r}") from error
            if not isinstance(message, dict):
                raise HttpSmokeFailure(f"SSE JSON message was not an object: {message!r}")
            messages.append(message)

        for line in text.splitlines():
            if not line:
                finish_event()
                continue
            if line.startswith(":"):
                continue
            if line.startswith("data:"):
                value = line[5:]
                if value.startswith(" "):
                    value = value[1:]
                data_lines.append(value)
        finish_event()
        if not messages:
            raise HttpSmokeFailure(f"SSE response contained no JSON-RPC messages: {text!r}")
        return messages

    raise HttpSmokeFailure(
        f"request response used unsupported Content-Type {media_type!r}: {text!r}"
    )


def find_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def wait_for_listener(
    process: subprocess.Popen[str],
    host: str,
    port: int,
    timeout: float,
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        return_code = process.poll()
        if return_code is not None:
            raise HttpSmokeFailure(
                f"HTTP server exited before listening with status {return_code}"
            )
        try:
            with socket.create_connection((host, port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise HttpSmokeFailure(f"HTTP server did not listen on {host}:{port}")


def drain(stream: TextIO, target: list[str]) -> None:
    for line in stream:
        target.append(line.rstrip("\n"))


def run_smoke(server: Path, timeout: float) -> dict[str, Any]:
    if not server.is_file():
        raise HttpSmokeFailure(f"HTTP server binary does not exist: {server}")

    with tempfile.TemporaryDirectory(prefix="log-query-mcp-http-smoke-") as temp:
        temp_dir = Path(temp)
        config_path = shared.write_fixture(temp_dir)
        host = "127.0.0.1"
        port = find_free_port()
        environment = os.environ.copy()
        environment["LOG_QUERY_MCP_CONFIG"] = str(config_path)
        environment["LOG_QUERY_MCP_BIND"] = f"{host}:{port}"
        environment.setdefault("RUST_LOG", "warn")

        process = subprocess.Popen(
            [str(server)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            env=environment,
        )
        if process.stdout is None or process.stderr is None:
            process.kill()
            raise HttpSmokeFailure("failed to create HTTP server output pipes")

        stdout_lines: list[str] = []
        stderr_lines: list[str] = []
        stdout_thread = threading.Thread(
            target=drain,
            args=(process.stdout, stdout_lines),
            name="mcp-http-stdout-reader",
            daemon=True,
        )
        stderr_thread = threading.Thread(
            target=drain,
            args=(process.stderr, stderr_lines),
            name="mcp-http-stderr-reader",
            daemon=True,
        )
        stdout_thread.start()
        stderr_thread.start()

        report: dict[str, Any] | None = None
        failure: Exception | None = None
        try:
            wait_for_listener(process, host, port, timeout)
            client = StreamableHttpClient(host, port, "/mcp", timeout)
            initialize = client.request(
                1,
                "initialize",
                {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "log-query-mcp-http-smoke",
                        "version": "0.1.0",
                    },
                },
            )
            if initialize.get("protocolVersion") != PROTOCOL_VERSION:
                raise HttpSmokeFailure(
                    "server negotiated an unexpected protocol version: "
                    f"{initialize.get('protocolVersion')!r}"
                )
            capabilities = initialize.get("capabilities")
            if not isinstance(capabilities, dict) or "tools" not in capabilities:
                raise HttpSmokeFailure("server did not declare the tools capability")

            client.initialized = True
            client.notification("notifications/initialized")

            tools_result = client.request(2, "tools/list", {})
            tools = tools_result.get("tools")
            if not isinstance(tools, list):
                raise HttpSmokeFailure("tools/list did not return a tools array")
            shared.validate_tool_schema(tools)

            sources = client.tool_call(3, "list_log_sources", {})
            source_items = sources.get("sources")
            if not isinstance(source_items, list) or not any(
                isinstance(item, dict) and item.get("source_id") == "payment-test"
                for item in source_items
            ):
                raise HttpSmokeFailure("list_log_sources did not expose payment-test")

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
            first_page = client.tool_call(4, "search_logs", search_arguments)
            first_results = first_page.get("results")
            if not isinstance(first_results, list) or len(first_results) != 1:
                raise HttpSmokeFailure(f"first search page was unexpected: {first_page!r}")
            first_match = first_results[0]
            if not isinstance(first_match, dict) or "first failure" not in str(
                first_match.get("content", "")
            ):
                raise HttpSmokeFailure(f"first search result was unexpected: {first_match!r}")
            match_ref = first_match.get("match_ref")
            if not isinstance(match_ref, str) or not match_ref.startswith("mref_"):
                raise HttpSmokeFailure("search result did not contain an opaque match_ref")
            next_cursor = first_page.get("next_cursor")
            if not isinstance(next_cursor, str) or not next_cursor:
                raise HttpSmokeFailure("first search page did not return a continuation cursor")

            context = client.tool_call(
                5,
                "get_log_context",
                {
                    "match_ref": match_ref,
                    "before_lines": 1,
                    "after_lines": 1,
                },
            )
            context_lines = context.get("lines")
            if not isinstance(context_lines, list) or len(context_lines) != 3:
                raise HttpSmokeFailure(f"context response was unexpected: {context!r}")
            if "abc123" not in str(context_lines[1].get("content", "")):
                raise HttpSmokeFailure("context middle line did not contain the search match")

            second_arguments = dict(search_arguments)
            second_arguments["cursor"] = next_cursor
            second_page = client.tool_call(6, "search_logs", second_arguments)
            second_results = second_page.get("results")
            if not isinstance(second_results, list) or len(second_results) != 1:
                raise HttpSmokeFailure(f"second search page was unexpected: {second_page!r}")
            if "second failure" not in str(second_results[0].get("content", "")):
                raise HttpSmokeFailure("second search page did not continue to the next match")

            delete_status = client.close_session()
            report = {
                "transport": "streamable_http",
                "protocol_version": initialize["protocolVersion"],
                "server_name": initialize.get("serverInfo", {}).get("name"),
                "session_assigned": client.session_id is not None,
                "session_delete_status": delete_status,
                "response_content_types": sorted(client.response_content_types),
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
        except Exception as error:  # noqa: BLE001 - normalize below for diagnostics
            failure = error
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)
            stdout_thread.join(timeout=1)
            stderr_thread.join(timeout=1)

        if failure is not None:
            if stdout_lines:
                print("--- server stdout ---", file=sys.stderr)
                print("\n".join(stdout_lines[-100:]), file=sys.stderr)
            if stderr_lines:
                print("--- server stderr ---", file=sys.stderr)
                print("\n".join(stderr_lines[-100:]), file=sys.stderr)
            if isinstance(failure, HttpSmokeFailure):
                raise failure
            raise HttpSmokeFailure(str(failure)) from failure

        if process.returncode != 0:
            raise HttpSmokeFailure(
                f"HTTP server did not shut down cleanly after SIGTERM: {process.returncode}"
            )
        if report is None:
            raise HttpSmokeFailure("HTTP smoke test produced no report")
        report["graceful_sigterm_verified"] = True
        return report


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--server",
        type=Path,
        default=Path("target/debug/log-query-mcp"),
        help="path to the already-built Streamable HTTP server binary",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT_SECONDS,
        help="per-request and startup timeout in seconds",
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
    except shared.SmokeFailure as error:
        print(f"MCP Streamable HTTP smoke test failed: {error}", file=sys.stderr)
        return 1

    encoded = json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True)
    print(encoded)
    if args.report is not None:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(encoded + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
