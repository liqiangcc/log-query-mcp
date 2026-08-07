#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/upgrade.sh <release-directory-or-tar.gz>

Verifies the release package, creates a rollback backup, atomically replaces
runtime files, restarts log-query-mcp, and automatically rolls back when the
post-upgrade health check fails.
EOF
}

die() {
  echo "upgrade: $*" >&2
  exit 1
}

[[ $# -eq 1 ]] || { usage >&2; exit 2; }
input="$1"

if [[ "${EUID}" -ne 0 && "${LOG_QUERY_MCP_ALLOW_NON_ROOT:-0}" != "1" ]]; then
  die "must run as root"
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
install_root="${LOG_QUERY_MCP_INSTALL_ROOT:-/opt/log-query-mcp}"
config_path="${LOG_QUERY_MCP_CONFIG_PATH:-/etc/log-query-mcp/config.json}"
unit_path="${LOG_QUERY_MCP_UNIT_PATH:-/etc/systemd/system/log-query-mcp.service}"
backup_root="${LOG_QUERY_MCP_BACKUP_ROOT:-/var/lib/log-query-mcp/backups}"
service_name="${LOG_QUERY_MCP_SERVICE_NAME:-log-query-mcp.service}"
systemctl_bin="${LOG_QUERY_MCP_SYSTEMCTL:-systemctl}"
healthcheck_cmd="${LOG_QUERY_MCP_HEALTHCHECK_CMD:-}"
rollback_script="${LOG_QUERY_MCP_ROLLBACK_SCRIPT:-${script_dir}/rollback.sh}"

tmp_extract=""
cleanup() {
  if [[ -n "${tmp_extract}" ]]; then
    rm -rf "${tmp_extract}"
  fi
}
trap cleanup EXIT

if [[ -d "${input}" ]]; then
  package_root="$(cd "${input}" && pwd)"
elif [[ -f "${input}" && "${input}" == *.tar.gz ]]; then
  tmp_extract="$(mktemp -d)"
  tar -xzf "${input}" -C "${tmp_extract}"
  mapfile -t roots < <(find "${tmp_extract}" -mindepth 1 -maxdepth 1 -type d -print)
  [[ "${#roots[@]}" -eq 1 ]] || die "release archive must contain exactly one top-level directory"
  package_root="${roots[0]}"
else
  die "release package not found"
fi

[[ -f "${package_root}/SHA256SUMS" ]] || die "release package is missing SHA256SUMS"
(
  cd "${package_root}"
  sha256sum -c SHA256SUMS >/dev/null
) || die "release package checksum verification failed"

for required in \
  bin/log-query-mcp \
  bin/log-query-mcp-stdio \
  systemd/log-query-mcp.service \
  BUILDINFO; do
  [[ -f "${package_root}/${required}" ]] || die "release package is missing ${required}"
done
[[ -x "${package_root}/bin/log-query-mcp" ]] || die "release binary is not executable"
[[ -x "${package_root}/bin/log-query-mcp-stdio" ]] || die "release stdio binary is not executable"
[[ -f "${rollback_script}" ]] || die "rollback helper not found: ${rollback_script}"

[[ -x "${install_root}/bin/log-query-mcp" ]] || die "current installation is missing log-query-mcp"
[[ -x "${install_root}/bin/log-query-mcp-stdio" ]] || die "current installation is missing log-query-mcp-stdio"

mkdir -p "${backup_root}"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
backup_dir="$(mktemp -d "${backup_root}/upgrade-${stamp}-XXXXXX")"
mkdir -p "${backup_dir}/bin"
cp -p -- "${install_root}/bin/log-query-mcp" "${backup_dir}/bin/log-query-mcp"
cp -p -- "${install_root}/bin/log-query-mcp-stdio" "${backup_dir}/bin/log-query-mcp-stdio"

if [[ -f "${install_root}/BUILDINFO" ]]; then
  cp -p -- "${install_root}/BUILDINFO" "${backup_dir}/BUILDINFO"
else
  : >"${backup_dir}/BUILDINFO.absent"
fi
if [[ -f "${config_path}" ]]; then
  cp -p -- "${config_path}" "${backup_dir}/config.json"
else
  : >"${backup_dir}/config.absent"
fi
if [[ -f "${unit_path}" ]]; then
  cp -p -- "${unit_path}" "${backup_dir}/service.unit"
else
  : >"${backup_dir}/service.absent"
fi
cat >"${backup_dir}/BACKUPINFO" <<EOF
created_at_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)
package_root=${package_root}
install_root=${install_root}
EOF

atomic_install() {
  local source="$1"
  local destination="$2"
  local mode="$3"
  local parent tmp

  parent="$(dirname "${destination}")"
  mkdir -p "${parent}"
  tmp="$(mktemp "${parent}/.upgrade.XXXXXX")"
  trap 'rm -f "${tmp:-}"' RETURN
  cp -- "${source}" "${tmp}"
  chmod "${mode}" "${tmp}"
  sync -f "${tmp}" 2>/dev/null || true
  mv -f -- "${tmp}" "${destination}"
  sync -f "${parent}" 2>/dev/null || true
  trap - RETURN
}

rollback_on_failure() {
  local exit_code="$?"
  trap - ERR
  echo "upgrade: post-mutation step failed; rolling back from ${backup_dir}" >&2
  if ! bash "${rollback_script}" "${backup_dir}"; then
    echo "upgrade: automatic rollback also failed; manual recovery required from ${backup_dir}" >&2
  fi
  exit "${exit_code}"
}
trap rollback_on_failure ERR

atomic_install "${package_root}/bin/log-query-mcp" "${install_root}/bin/log-query-mcp" 0755
atomic_install "${package_root}/bin/log-query-mcp-stdio" "${install_root}/bin/log-query-mcp-stdio" 0755
atomic_install "${package_root}/BUILDINFO" "${install_root}/BUILDINFO" 0644
atomic_install "${package_root}/systemd/log-query-mcp.service" "${unit_path}" 0644

# The production config is deliberately not replaced during upgrade. It is only backed up
# so rollback can restore the exact pre-upgrade state if an external migration changed it.
if command -v "${systemctl_bin}" >/dev/null 2>&1; then
  "${systemctl_bin}" daemon-reload
  "${systemctl_bin}" restart "${service_name}"
fi

if [[ -n "${healthcheck_cmd}" ]]; then
  bash -c "${healthcheck_cmd}"
elif command -v "${systemctl_bin}" >/dev/null 2>&1; then
  "${systemctl_bin}" is-active --quiet "${service_name}"
fi

trap - ERR
echo "upgrade: installed release from ${package_root}"
echo "upgrade: rollback backup ${backup_dir}"
