#!/usr/bin/env python3
"""Validate v1/v2 JSON Schemas, fixtures, and frozen cross-field rules."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any, Callable

from jsonschema import Draft202012Validator, FormatChecker

ROOT = Path(__file__).resolve().parents[1]
CONFIG_V1_SCHEMA_PATH = ROOT / "schemas" / "log-query-mcp-config-v1.schema.json"
CONFIG_V2_SCHEMA_PATH = ROOT / "schemas" / "log-query-mcp-config-v2.schema.json"
MCP_SCHEMA_PATH = ROOT / "schemas" / "mcp-tools-v1.schema.json"
ERROR_V1_SCHEMA_PATH = ROOT / "schemas" / "tool-error-v1.schema.json"
ERROR_V2_SCHEMA_PATH = ROOT / "schemas" / "tool-error-v2.schema.json"
CONFIG_V1_EXAMPLE_PATH = ROOT / "examples" / "log-query-mcp.v1.json"
V2_VALID_FIXTURE_DIR = ROOT / "tests" / "contracts" / "v2" / "valid"
V2_INVALID_FIXTURE_DIR = ROOT / "tests" / "contracts" / "v2" / "invalid"

RuleValidator = Callable[[dict[str, Any]], None]


def load_json(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def check_schema(schema: dict[str, Any], path: Path) -> None:
    try:
        Draft202012Validator.check_schema(schema)
    except Exception as exc:  # jsonschema exposes several schema error subclasses
        raise AssertionError(f"invalid JSON Schema {path}: {exc}") from exc


def format_errors(errors: list[Any]) -> str:
    lines: list[str] = []
    for error in sorted(errors, key=lambda item: list(item.absolute_path)):
        path = ".".join(str(part) for part in error.absolute_path) or "<root>"
        lines.append(f"{path}: {error.message}")
    return "\n".join(lines)


def schema_errors(schema: dict[str, Any], instance: Any) -> list[Any]:
    validator = Draft202012Validator(schema, format_checker=FormatChecker())
    return list(validator.iter_errors(instance))


def validate_instance(
    schema: dict[str, Any], instance: Any, description: str
) -> None:
    errors = schema_errors(schema, instance)
    if errors:
        raise AssertionError(f"{description} is invalid:\n{format_errors(errors)}")


def definition_schema(root_schema: dict[str, Any], name: str) -> dict[str, Any]:
    return {
        "$schema": root_schema["$schema"],
        "$defs": root_schema["$defs"],
        "$ref": f"#/$defs/{name}",
    }


def assert_invalid(
    schema: dict[str, Any], instance: Any, description: str
) -> None:
    if not schema_errors(schema, instance):
        raise AssertionError(f"{description} unexpectedly passed validation")


def validate_common_config_rules(config: dict[str, Any]) -> None:
    sources = config["sources"]
    source_ids = [source["source_id"] for source in sources]
    if len(source_ids) != len(set(source_ids)):
        raise AssertionError("source_id values must be globally unique")

    limits = config.get("limits", {})
    default_results = limits.get("default_results_per_page", 50)
    max_results = limits.get("max_results_per_page", 200)
    max_line = limits.get("max_line_bytes", 16 * 1024)
    max_content = limits.get("max_returned_content_bytes", 512 * 1024)
    max_response = limits.get("max_response_bytes", 1024 * 1024)

    if default_results > max_results:
        raise AssertionError(
            "default_results_per_page must not exceed max_results_per_page"
        )
    if max_line > max_content:
        raise AssertionError(
            "max_line_bytes must not exceed max_returned_content_bytes"
        )
    if max_content >= max_response:
        raise AssertionError(
            "max_returned_content_bytes must be smaller than max_response_bytes"
        )


def validate_v1_config_rules(config: dict[str, Any]) -> None:
    validate_common_config_rules(config)


def validate_v2_config_rules(config: dict[str, Any]) -> None:
    validate_common_config_rules(config)

    connections = config.get("connections", [])
    connection_ids = [connection["connection_id"] for connection in connections]
    if len(connection_ids) != len(set(connection_ids)):
        raise AssertionError("connection_id values must be globally unique")
    known_connections = set(connection_ids)

    ssh_sources = []
    for source in config["sources"]:
        backend = source.get("backend", {})
        if backend.get("type") != "ssh":
            continue
        ssh_sources.append(source)
        connection_id = backend.get("connection_id")
        if connection_id not in known_connections:
            raise AssertionError(
                f"source_id={source['source_id']} references unknown connection_id"
            )

    if ssh_sources and not connections:
        raise AssertionError("SSH sources require at least one SSH connection")
    if ssh_sources and "cache" not in config:
        raise AssertionError("SSH sources require cache configuration")

    cache = config.get("cache")
    if cache is not None:
        max_bytes = cache["max_bytes"]
        max_bytes_per_source = cache["max_bytes_per_source"]
        if max_bytes_per_source > max_bytes:
            raise AssertionError(
                "cache.max_bytes_per_source must not exceed cache.max_bytes"
            )


def validate_v2_fixtures(schema: dict[str, Any]) -> None:
    valid_paths = sorted(V2_VALID_FIXTURE_DIR.glob("*.json"))
    invalid_paths = sorted(V2_INVALID_FIXTURE_DIR.glob("*.json"))
    if not valid_paths:
        raise AssertionError("no v2 valid fixtures found")
    if not invalid_paths:
        raise AssertionError("no v2 invalid fixtures found")

    for path in valid_paths:
        instance = load_json(path)
        validate_instance(schema, instance, f"valid v2 fixture {path.name}")
        validate_v2_config_rules(instance)

    for path in invalid_paths:
        instance = load_json(path)
        errors = schema_errors(schema, instance)
        if errors:
            continue
        try:
            validate_v2_config_rules(instance)
        except AssertionError:
            continue
        raise AssertionError(f"invalid v2 fixture {path.name} unexpectedly passed")


def validate_mcp_contracts(schema: dict[str, Any]) -> None:
    valid_instances: dict[str, Any] = {
        "ListLogSourcesRequest": {},
        "ListLogSourcesResponse": {
            "sources": [
                {
                    "source_id": "payment-test",
                    "name": "支付服务测试环境",
                    "description": "payment logs",
                    "service": "payment-service",
                    "environment": "test",
                    "tags": ["payment", "java"],
                }
            ]
        },
        "SearchLogsRequest": {
            "source_ids": ["payment-test", "order-test"],
            "keyword": "traceId=abc123",
            "case_sensitive": False,
            "start_time": "2026-06-19T14:00:00+09:00",
            "end_time": "2026-06-19T15:00:00+09:00",
            "order": "oldest_first",
            "max_results": 50,
            "cursor": None,
        },
        "SearchLogsResponse": {
            "results": [
                {
                    "match_ref": "mref_7c9f2d70a99244c1b9c8848f0c9cd807",
                    "source_id": "payment-test",
                    "file_id": "file-payment-test-0",
                    "file_name": "application.log",
                    "line_number": 42,
                    "timestamp": "2026-06-19T14:20:03.125+09:00",
                    "content": "traceId=abc123 PaymentAuthException",
                    "content_truncated": False,
                }
            ],
            "truncated": False,
            "next_cursor": None,
        },
        "GetLogContextRequest": {
            "match_ref": "mref_7c9f2d70a99244c1b9c8848f0c9cd807",
            "before_lines": 10,
            "after_lines": 30,
        },
        "GetLogContextResponse": {
            "source_id": "payment-test",
            "file_id": "file-payment-test-0",
            "file_name": "application.log",
            "start_line": 32,
            "end_line": 72,
            "lines": [{"line_number": 32, "content": "..."}],
            "truncated": False,
        },
    }

    for name, instance in valid_instances.items():
        validate_instance(
            definition_schema(schema, name), instance, f"valid {name} example"
        )

    invalid_newest = dict(valid_instances["SearchLogsRequest"])
    invalid_newest["order"] = "newest_first"
    assert_invalid(
        definition_schema(schema, "SearchLogsRequest"),
        invalid_newest,
        "newest_first v1 request",
    )

    invalid_path_field = dict(valid_instances["SearchLogsRequest"])
    invalid_path_field["path"] = "/etc/passwd"
    assert_invalid(
        definition_schema(schema, "SearchLogsRequest"),
        invalid_path_field,
        "request containing an unknown path field",
    )

    invalid_multiline = dict(valid_instances["SearchLogsRequest"])
    invalid_multiline["keyword"] = "abc\ndef"
    assert_invalid(
        definition_schema(schema, "SearchLogsRequest"),
        invalid_multiline,
        "multiline keyword",
    )


def validate_error_contract(
    schema: dict[str, Any], valid_codes: list[tuple[str, bool]], version: str
) -> None:
    for code, retryable in valid_codes:
        validate_instance(
            schema,
            {
                "code": code,
                "message": f"representative {version} {code.lower()} error",
                "retryable": retryable,
            },
            f"valid {version} {code} error",
        )

    assert_invalid(
        schema,
        {
            "code": "SERVER_PATH_LEAK",
            "message": "/var/log/private/application.log",
            "retryable": False,
        },
        f"unknown {version} tool error code",
    )
    assert_invalid(
        schema,
        {
            "code": "INTERNAL_ERROR",
            "message": "failure",
            "retryable": True,
            "details": {"path": "/var/log/private/application.log"},
        },
        f"{version} tool error containing an unapproved details field",
    )


def main() -> int:
    config_v1_schema = load_json(CONFIG_V1_SCHEMA_PATH)
    config_v2_schema = load_json(CONFIG_V2_SCHEMA_PATH)
    mcp_schema = load_json(MCP_SCHEMA_PATH)
    error_v1_schema = load_json(ERROR_V1_SCHEMA_PATH)
    error_v2_schema = load_json(ERROR_V2_SCHEMA_PATH)
    config_v1_example = load_json(CONFIG_V1_EXAMPLE_PATH)

    for schema, path in [
        (config_v1_schema, CONFIG_V1_SCHEMA_PATH),
        (config_v2_schema, CONFIG_V2_SCHEMA_PATH),
        (mcp_schema, MCP_SCHEMA_PATH),
        (error_v1_schema, ERROR_V1_SCHEMA_PATH),
        (error_v2_schema, ERROR_V2_SCHEMA_PATH),
    ]:
        check_schema(schema, path)

    validate_instance(
        config_v1_schema, config_v1_example, "v1 configuration example"
    )
    validate_v1_config_rules(config_v1_example)
    validate_v2_fixtures(config_v2_schema)
    validate_mcp_contracts(mcp_schema)

    validate_error_contract(
        error_v1_schema,
        [("UNKNOWN_SOURCE", False), ("FILE_CHANGED", True)],
        "v1",
    )
    validate_error_contract(
        error_v2_schema,
        [
            ("REMOTE_UNAVAILABLE", True),
            ("REMOTE_AUTH_FAILED", False),
            ("HOST_KEY_VERIFICATION_FAILED", False),
            ("SYNC_FAILED", True),
            ("CACHE_SCOPE_EXCEEDED", False),
            ("CACHE_LIMIT_EXCEEDED", False),
            ("CACHE_CORRUPTED", False),
        ],
        "v2",
    )

    print(
        "v1/v2 schemas, fixtures, MCP API, error models, and frozen rules are valid"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, ValueError) as exc:
        print(f"contract validation failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
