#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/rollback.sh <backup-dir>

Restores a backup created by scripts/upgrade.sh, restarts log-query-mcp, and
verifies service + MCP protocol health. Production defaults can be overridden
for isolated tests with LOG_QUERY_MCP_* environment variables.
EOF
}

die() {
  echo "rollback: $*" >&2
  exit 1
}

[[ $# -eq 1 ]] || { usage >&2; exit 2; }
backup_dir="$1"
[[ -d "${backup_dir}" ]] || die "backup directory not found"

if [[ "${EUID}" -ne 0 && "${LOG_QUERY_MCP_ALLOW_NON_ROOT:-0}" != "1" ]]; then
  die "must run as root"
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
install_root="${LOG_QUERY_MCP_INSTALL_ROOT:-/opt/log-query-mcp}"
config_path="${LOG_QUERY_MCP_CONFIG_PATH:-/etc/log-query-mcp/config.json}"
unit_path="${LOG_QUERY_MCP_UNIT_PATH:-/etc/systemd/system/log-query-mcp.service}"
service_name="${LOG_QUERY_MCP_SERVICE_NAME:-log-query-mcp.service}"
systemctl_bin="${LOG_QUERY_MCP_SYSTEMCTL:-systemctl}"
healthcheck_cmd="${LOG_QUERY_MCP_HEALTHCHECK_CMD:-}"
healthcheck_script="${LOG_QUERY_MCP_HEALTHCHECK_SCRIPT:-${script_dir}/healthcheck.sh}"

atomic_restore() {
  local source="$1"
  local destination="$2"
  local mode="$3"
  local parent tmp

  parent="$(dirname "${destination}")"
  mkdir -p "${parent}"
  tmp="$(mktemp "${parent}/.rollback.XXXXXX")"
  trap 'rm -f "${tmp:-}"' RETURN
  # Backups are created with cp -p. Preserve their ownership on restore so
  # service-readable files such as root:log-query-mcp config.json do not become
  # root:root after rollback.
  cp -p -- "${source}" "${tmp}"
  chmod "${mode}" "${tmp}"
  sync -f "${tmp}" 2>/dev/null || true
  mv -f -- "${tmp}" "${destination}"
  sync -f "${parent}" 2>/dev/null || true
  trap - RETURN
}

run_healthcheck() {
  if [[ -n "${healthcheck_cmd}" ]]; then
    bash -c "${healthcheck_cmd}"
  else
    [[ -f "${healthcheck_script}" ]] || die "health check helper not found: ${healthcheck_script}"
    LOG_QUERY_MCP_SERVICE_NAME="${service_name}" \
    LOG_QUERY_MCP_SYSTEMCTL="${systemctl_bin}" \
      bash "${healthcheck_script}"
  fi
}

command -v "${systemctl_bin}" >/dev/null 2>&1 || die "systemctl command not found: ${systemctl_bin}"
"${systemctl_bin}" stop "${service_name}" >/dev/null 2>&1 || true

[[ -f "${backup_dir}/bin/log-query-mcp" ]] || die "backup is missing log-query-mcp"
[[ -f "${backup_dir}/bin/log-query-mcp-stdio" ]] || die "backup is missing log-query-mcp-stdio"
atomic_restore "${backup_dir}/bin/log-query-mcp" "${install_root}/bin/log-query-mcp" 0755
atomic_restore "${backup_dir}/bin/log-query-mcp-stdio" "${install_root}/bin/log-query-mcp-stdio" 0755

if [[ -f "${backup_dir}/BUILDINFO" ]]; then
  atomic_restore "${backup_dir}/BUILDINFO" "${install_root}/BUILDINFO" 0644
elif [[ -f "${backup_dir}/BUILDINFO.absent" ]]; then
  rm -f "${install_root}/BUILDINFO"
fi

if [[ -f "${backup_dir}/config.json" ]]; then
  atomic_restore "${backup_dir}/config.json" "${config_path}" 0640
elif [[ -f "${backup_dir}/config.absent" ]]; then
  rm -f "${config_path}"
fi

if [[ -f "${backup_dir}/service.unit" ]]; then
  atomic_restore "${backup_dir}/service.unit" "${unit_path}" 0644
elif [[ -f "${backup_dir}/service.absent" ]]; then
  rm -f "${unit_path}"
fi

"${systemctl_bin}" daemon-reload
"${systemctl_bin}" restart "${service_name}"
run_healthcheck || die "health check failed after rollback"

echo "rollback: restored ${backup_dir}"
