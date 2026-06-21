#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "uninstall: $*" >&2
  exit 1
}

purge_config=0
if [[ "${1:-}" == "--purge-config" ]]; then
  purge_config=1
elif [[ $# -gt 0 ]]; then
  die "usage: $0 [--purge-config]"
fi

if [[ "${EUID}" -ne 0 ]]; then
  die "must run as root"
fi

unit_path="/etc/systemd/system/log-query-mcp.service"
install_root="/opt/log-query-mcp"
config_dir="/etc/log-query-mcp"

if command -v systemctl >/dev/null 2>&1; then
  systemctl disable --now log-query-mcp.service >/dev/null 2>&1 || true
fi

rm -f "${unit_path}"
rm -f "${install_root}/bin/log-query-mcp" "${install_root}/bin/log-query-mcp-stdio" "${install_root}/BUILDINFO"
rmdir "${install_root}/bin" "${install_root}" >/dev/null 2>&1 || true

if [[ "${purge_config}" -eq 1 ]]; then
  rm -rf "${config_dir}"
else
  echo "uninstall: keeping ${config_dir}; pass --purge-config to remove it" >&2
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
fi

echo "uninstall: removed log-query-mcp service and binaries" >&2
