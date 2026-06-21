#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "install: $*" >&2
  exit 1
}

if [[ "${EUID}" -ne 0 ]]; then
  die "must run as root"
fi

package_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
service_user="log-query-mcp"
service_group="log-query-mcp"
install_root="/opt/log-query-mcp"
bin_dir="${install_root}/bin"
config_dir="/etc/log-query-mcp"
config_path="${config_dir}/config.json"
unit_path="/etc/systemd/system/log-query-mcp.service"

[[ -x "${package_root}/bin/log-query-mcp" ]] || die "missing ${package_root}/bin/log-query-mcp"
[[ -x "${package_root}/bin/log-query-mcp-stdio" ]] || die "missing ${package_root}/bin/log-query-mcp-stdio"
[[ -f "${package_root}/systemd/log-query-mcp.service" ]] || die "missing systemd unit"
[[ -f "${package_root}/examples/log-query-mcp.v1.json" ]] || die "missing example config"

if ! getent group "${service_group}" >/dev/null; then
  groupadd --system "${service_group}"
fi

if ! id -u "${service_user}" >/dev/null 2>&1; then
  nologin="/usr/sbin/nologin"
  if [[ ! -x "${nologin}" ]]; then
    nologin="/sbin/nologin"
  fi
  useradd --system --no-create-home --home-dir /nonexistent --shell "${nologin}" --gid "${service_group}" "${service_user}"
fi

install -d -m 0755 "${bin_dir}"
install -m 0755 "${package_root}/bin/log-query-mcp" "${bin_dir}/log-query-mcp"
install -m 0755 "${package_root}/bin/log-query-mcp-stdio" "${bin_dir}/log-query-mcp-stdio"
if [[ -f "${package_root}/BUILDINFO" ]]; then
  install -m 0644 "${package_root}/BUILDINFO" "${install_root}/BUILDINFO"
fi

install -d -m 0750 -o root -g "${service_group}" "${config_dir}"
if [[ ! -e "${config_path}" ]]; then
  install -m 0640 -o root -g "${service_group}" "${package_root}/examples/log-query-mcp.v1.json" "${config_path}"
  echo "install: wrote example config to ${config_path}" >&2
else
  echo "install: keeping existing ${config_path}" >&2
fi

install -m 0644 "${package_root}/systemd/log-query-mcp.service" "${unit_path}"

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
  echo "install: installed ${unit_path}" >&2
  echo "install: run 'systemctl enable --now log-query-mcp.service' after reviewing ${config_path}" >&2
else
  echo "install: systemctl not found; copied files but did not reload systemd" >&2
fi
