#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/m7_real_target_acceptance.sh \
  --config <path> \
  --source-id <id> \
  --keyword <marker> [options]

Required:
  --config <path>                 Production v2 config used by both acceptance gates.
  --source-id <id>                ProxyCommand-backed source to exercise.
  --keyword <marker>              Existing marker that must be found in the remote log.

Options:
  --stdio-bin <path>              Default: /opt/log-query-mcp/bin/log-query-mcp-stdio
  --http-bin <path>               Default: /opt/log-query-mcp/bin/log-query-mcp
  --buildinfo <path>              Default: /opt/log-query-mcp/BUILDINFO
  --evidence-root <dir>           Default: /var/lib/log-query-mcp/m7-wsl-evidence
  --url <url>                     Default: http://127.0.0.1:8000/mcp
  --service-name <name>           Default: log-query-mcp.service
  --expected-service-user <user>  Default: log-query-mcp
  --systemctl-bin <path>          Default: systemctl
  --curl-bin <path>               Default: curl
  --tasklist-bin <path>           Default: tasklist.exe
  --before-lines <n>              Default: 1
  --after-lines <n>               Default: 1
  -h, --help                      Show this help.

This orchestrator runs, in order:
  A. service-identity stdio WSL acceptance
  B. production systemd/MCP healthcheck
  C. production systemd HTTP Proxy-source acceptance
  D. offline stdio+HTTP evidence pair verification

It does not create markers, mutate production config, inject Secrets, restart the
service, or turn synthetic validation into real-target PASS evidence.
EOF
}

die() {
  echo "m7_real_target_acceptance: $*" >&2
  exit 1
}

config=""
source_id=""
keyword=""
stdio_bin="/opt/log-query-mcp/bin/log-query-mcp-stdio"
http_bin="/opt/log-query-mcp/bin/log-query-mcp"
buildinfo="/opt/log-query-mcp/BUILDINFO"
evidence_root="/var/lib/log-query-mcp/m7-wsl-evidence"
url="http://127.0.0.1:8000/mcp"
service_name="log-query-mcp.service"
expected_service_user="log-query-mcp"
systemctl_bin="systemctl"
curl_bin="curl"
tasklist_bin="tasklist.exe"
before_lines="1"
after_lines="1"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --config) config="${2:-}"; shift 2 ;;
    --source-id) source_id="${2:-}"; shift 2 ;;
    --keyword) keyword="${2:-}"; shift 2 ;;
    --stdio-bin) stdio_bin="${2:-}"; shift 2 ;;
    --http-bin) http_bin="${2:-}"; shift 2 ;;
    --buildinfo) buildinfo="${2:-}"; shift 2 ;;
    --evidence-root) evidence_root="${2:-}"; shift 2 ;;
    --url) url="${2:-}"; shift 2 ;;
    --service-name) service_name="${2:-}"; shift 2 ;;
    --expected-service-user) expected_service_user="${2:-}"; shift 2 ;;
    --systemctl-bin) systemctl_bin="${2:-}"; shift 2 ;;
    --curl-bin) curl_bin="${2:-}"; shift 2 ;;
    --tasklist-bin) tasklist_bin="${2:-}"; shift 2 ;;
    --before-lines) before_lines="${2:-}"; shift 2 ;;
    --after-lines) after_lines="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

[[ -n "${config}" ]] || die "--config is required"
[[ -n "${source_id}" ]] || die "--source-id is required"
[[ -n "${keyword}" ]] || die "--keyword is required"
[[ -f "${config}" ]] || die "config is not a regular file"
[[ -x "${stdio_bin}" ]] || die "stdio binary is not executable"
[[ -x "${http_bin}" ]] || die "HTTP binary is not executable"
[[ "${before_lines}" =~ ^[0-9]+$ ]] || die "--before-lines must be a non-negative integer"
[[ "${after_lines}" =~ ^[0-9]+$ ]] || die "--after-lines must be a non-negative integer"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
for path in \
  "${script_dir}/m7_wsl_acceptance.sh" \
  "${script_dir}/m7_wsl_acceptance.py" \
  "${script_dir}/m7_wsl_http_acceptance.py" \
  "${script_dir}/verify_m7_evidence.py" \
  "${script_dir}/healthcheck.sh"; do
  [[ -f "${path}" ]] || die "required acceptance component is missing: $(basename "${path}")"
done

actual_user="$(id -un)"
if [[ "${actual_user}" != "${expected_service_user}" ]]; then
  die "SERVICE_IDENTITY_MISMATCH: expected ${expected_service_user}, got ${actual_user}"
fi

run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
run_dir="${evidence_root%/}/run-${run_id}"
mkdir -p "${run_dir}"
chmod 0700 "${run_dir}" 2>/dev/null || true

printf '%s\n' "m7_real_target_acceptance: run_dir=${run_dir}"
printf '%s\n' "m7_real_target_acceptance: Gate A - service-identity stdio WSL acceptance"
M7_WSL_EXPECTED_USER="${expected_service_user}" \
  "${script_dir}/m7_wsl_acceptance.sh" \
    --config "${config}" \
    --source-id "${source_id}" \
    --keyword "${keyword}" \
    --stdio-bin "${stdio_bin}" \
    --buildinfo "${buildinfo}" \
    --evidence-dir "${run_dir}" \
    --tasklist-bin "${tasklist_bin}" \
    --before-lines "${before_lines}" \
    --after-lines "${after_lines}"

mapfile -t stdio_evidence < <(find "${run_dir}" -maxdepth 1 -type f -name 'm7-wsl-acceptance-*.json' -print | sort)
[[ "${#stdio_evidence[@]}" -eq 1 ]] || die "Gate A did not produce exactly one stdio evidence file"

printf '%s\n' "m7_real_target_acceptance: Gate B - production healthcheck"
LOG_QUERY_MCP_SERVICE_NAME="${service_name}" \
LOG_QUERY_MCP_SYSTEMCTL="${systemctl_bin}" \
LOG_QUERY_MCP_CURL="${curl_bin}" \
LOG_QUERY_MCP_URL="${url}" \
  "${script_dir}/healthcheck.sh"

printf '%s\n' "m7_real_target_acceptance: Gate C - production systemd HTTP Proxy-source acceptance"
python3 "${script_dir}/m7_wsl_http_acceptance.py" \
  --config "${config}" \
  --source-id "${source_id}" \
  --keyword "${keyword}" \
  --url "${url}" \
  --service-name "${service_name}" \
  --expected-service-user "${expected_service_user}" \
  --systemctl-bin "${systemctl_bin}" \
  --tasklist-bin "${tasklist_bin}" \
  --expected-http-bin "${http_bin}" \
  --buildinfo "${buildinfo}" \
  --evidence-dir "${run_dir}" \
  --before-lines "${before_lines}" \
  --after-lines "${after_lines}"

mapfile -t http_evidence < <(find "${run_dir}" -maxdepth 1 -type f -name 'm7-wsl-http-acceptance-*.json' -print | sort)
[[ "${#http_evidence[@]}" -eq 1 ]] || die "Gate C did not produce exactly one HTTP evidence file"

printf '%s\n' "m7_real_target_acceptance: Gate D - offline evidence pair verification"
python3 "${script_dir}/verify_m7_evidence.py" \
  --stdio-evidence "${stdio_evidence[0]}" \
  --http-evidence "${http_evidence[0]}"

printf '%s\n' "m7_real_target_acceptance: PASS"
printf '%s\n' "m7_real_target_acceptance: stdio_evidence=${stdio_evidence[0]}"
printf '%s\n' "m7_real_target_acceptance: http_evidence=${http_evidence[0]}"
printf '%s\n' "m7_real_target_acceptance: evidence_pair=PASS"
