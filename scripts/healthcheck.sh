#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "healthcheck: $*" >&2
  exit 1
}

service_name="${LOG_QUERY_MCP_SERVICE_NAME:-log-query-mcp.service}"
systemctl_bin="${LOG_QUERY_MCP_SYSTEMCTL:-systemctl}"
curl_bin="${LOG_QUERY_MCP_CURL:-curl}"
url="${LOG_QUERY_MCP_URL:-http://127.0.0.1:8000/mcp}"
timeout_seconds="${LOG_QUERY_MCP_HEALTHCHECK_TIMEOUT_SECONDS:-5}"
skip_systemd="${LOG_QUERY_MCP_HEALTHCHECK_SKIP_SYSTEMD:-0}"

if [[ "${skip_systemd}" != "1" ]]; then
  command -v "${systemctl_bin}" >/dev/null 2>&1 || die "systemctl command not found: ${systemctl_bin}"
  "${systemctl_bin}" is-active --quiet "${service_name}" || die "service is not active: ${service_name}"
fi

command -v "${curl_bin}" >/dev/null 2>&1 || die "curl command not found: ${curl_bin}"

response="$(mktemp)"
trap 'rm -f "${response}"' EXIT

request='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"production-healthcheck","version":"0.2.0"}}}'

"${curl_bin}" \
  --fail \
  --silent \
  --show-error \
  --max-time "${timeout_seconds}" \
  --output "${response}" \
  --header 'Content-Type: application/json' \
  --header 'Accept: application/json, text/event-stream' \
  --data "${request}" \
  "${url}" || die "MCP initialize request failed"

grep -Eq '"jsonrpc"[[:space:]]*:[[:space:]]*"2\.0"' "${response}" || die "MCP response is missing jsonrpc=2.0"
grep -q '"serverInfo"' "${response}" || die "MCP response is missing serverInfo"
grep -q '"log-query-mcp"' "${response}" || die "MCP response is from an unexpected server"
if grep -q '"error"' "${response}"; then
  die "MCP initialize returned a JSON-RPC error"
fi

echo "healthcheck: service active and MCP initialize succeeded"
