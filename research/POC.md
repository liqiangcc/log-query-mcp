# R-01 / R-02 protocol POC

This POC validates the Rust project baseline and the official `rmcp` tools-only server surface before implementing real file access.

## What is implemented

- Rust 2024 project with `#![forbid(unsafe_code)]`.
- Official `rmcp` SDK with structured input/output schemas.
- Streamable HTTP endpoint at `http://127.0.0.1:8000/mcp`.
- Optional stdio binary.
- Three tools backed by deterministic in-memory records:
  - `list_log_sources`
  - `search_logs`
  - `get_log_context`
- Basic validation and unit tests.
- Small cross-service log fixtures.

## Run Streamable HTTP

```bash
cargo run
```

Override the bind address when testing inside a controlled network:

```bash
LOG_QUERY_MCP_BIND=0.0.0.0:8000 cargo run
```

The default intentionally listens only on loopback.

## Run over stdio

```bash
cargo run --bin log-query-mcp-stdio
```

## Inspect

For stdio:

```bash
npx @modelcontextprotocol/inspector cargo run --bin log-query-mcp-stdio
```

For Streamable HTTP, start the server and configure the inspector or client endpoint as:

```text
http://127.0.0.1:8000/mcp
```

## Deliberate limitations

This is a protocol POC, not the production scanner:

- No server file-system access.
- No `openat2()` implementation yet.
- No real timeout or cancellation propagation into a scanner.
- No cursor pagination.
- Time range fields are accepted but not applied.
- `match_ref` is deterministic test data rather than a secure stateful reference.

These items belong to R-04 through R-09 in the technical research plan.
