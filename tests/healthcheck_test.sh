#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

state_file="${tmp}/service.state"
curl_mode_file="${tmp}/curl.mode"

echo active >"${state_file}"
echo success >"${curl_mode_file}"

fake_systemctl="${tmp}/systemctl"
cat >"${fake_systemctl}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "is-active" ]]; then
  [[ "$(cat "${FAKE_SYSTEMCTL_STATE}")" == active ]]
  exit
fi
exit 0
EOF
chmod +x "${fake_systemctl}"

fake_curl="${tmp}/curl"
cat >"${fake_curl}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
output=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --output)
      output="$2"
      shift 2
      ;;
    --header|--data|--max-time)
      shift 2
      ;;
    --fail|--silent|--show-error)
      shift
      ;;
    *)
      shift
      ;;
  esac
done
[[ -n "${output}" ]]
case "$(cat "${FAKE_CURL_MODE_FILE}")" in
  success)
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"log-query-mcp","version":"0.2.0"}}}' >"${output}"
    ;;
  json-error)
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"bad"}}' >"${output}"
    ;;
  wrong-server)
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"other-server"}}}' >"${output}"
    ;;
  transport-failure)
    exit 7
    ;;
  *)
    exit 8
    ;;
esac
EOF
chmod +x "${fake_curl}"

export LOG_QUERY_MCP_SYSTEMCTL="${fake_systemctl}"
export LOG_QUERY_MCP_CURL="${fake_curl}"
export LOG_QUERY_MCP_URL="http://127.0.0.1:8000/mcp"
export FAKE_SYSTEMCTL_STATE="${state_file}"
export FAKE_CURL_MODE_FILE="${curl_mode_file}"

# Healthy service + valid MCP initialize response succeeds.
bash "${repo_root}/scripts/healthcheck.sh"

# An inactive service fails before accepting protocol health.
echo inactive >"${state_file}"
if bash "${repo_root}/scripts/healthcheck.sh"; then
  echo "expected inactive service health check to fail" >&2
  exit 1
fi

# Protocol-level JSON-RPC error is unhealthy even when systemd says active.
echo active >"${state_file}"
echo json-error >"${curl_mode_file}"
if bash "${repo_root}/scripts/healthcheck.sh"; then
  echo "expected JSON-RPC error health check to fail" >&2
  exit 1
fi

# A response from the wrong server is rejected.
echo wrong-server >"${curl_mode_file}"
if bash "${repo_root}/scripts/healthcheck.sh"; then
  echo "expected wrong-server health check to fail" >&2
  exit 1
fi

# HTTP/transport failure is rejected.
echo transport-failure >"${curl_mode_file}"
if bash "${repo_root}/scripts/healthcheck.sh"; then
  echo "expected transport failure health check to fail" >&2
  exit 1
fi

# Protocol-only mode is available for containers/tests without systemd.
echo success >"${curl_mode_file}"
echo inactive >"${state_file}"
LOG_QUERY_MCP_HEALTHCHECK_SKIP_SYSTEMD=1 bash "${repo_root}/scripts/healthcheck.sh"

echo "healthcheck_test: all scenarios passed"
