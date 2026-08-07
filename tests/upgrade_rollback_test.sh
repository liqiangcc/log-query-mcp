#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

install_root="${tmp}/opt/log-query-mcp"
config_path="${tmp}/etc/log-query-mcp/config.json"
unit_path="${tmp}/etc/systemd/system/log-query-mcp.service"
backup_root="${tmp}/backups"
state_file="${tmp}/service.state"
fail_next_restart="${tmp}/fail-next-restart"
mkdir -p "${install_root}/bin" "$(dirname "${config_path}")" "$(dirname "${unit_path}")" "${backup_root}"

fake_systemctl="${tmp}/systemctl"
cat >"${fake_systemctl}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  daemon-reload) exit 0 ;;
  stop) echo inactive >"${FAKE_SYSTEMCTL_STATE}" ;;
  restart)
    if [[ -f "${FAKE_SYSTEMCTL_FAIL_NEXT_RESTART}" ]]; then
      rm -f "${FAKE_SYSTEMCTL_FAIL_NEXT_RESTART}"
      exit 1
    fi
    echo active >"${FAKE_SYSTEMCTL_STATE}"
    ;;
  is-active)
    [[ "$(cat "${FAKE_SYSTEMCTL_STATE}" 2>/dev/null || true)" == active ]]
    ;;
  *) exit 0 ;;
esac
EOF
chmod +x "${fake_systemctl}"

write_binary() {
  local path="$1"
  local version="$2"
  cat >"${path}" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "--health" ]]; then
  exit 0
fi
echo "${version}"
EOF
  chmod +x "${path}"
}

reset_old_install() {
  rm -rf "${install_root}" "$(dirname "${config_path}")" "$(dirname "${unit_path}")" "${backup_root}"
  mkdir -p "${install_root}/bin" "$(dirname "${config_path}")" "$(dirname "${unit_path}")" "${backup_root}"
  write_binary "${install_root}/bin/log-query-mcp" old
  write_binary "${install_root}/bin/log-query-mcp-stdio" old-stdio
  printf 'version=old\n' >"${install_root}/BUILDINFO"
  printf '{"sentinel":"keep-me"}\n' >"${config_path}"
  printf 'old-unit\n' >"${unit_path}"
  echo active >"${state_file}"
  rm -f "${fail_next_restart}"
}

make_package() {
  local root="$1"
  rm -rf "${root}"
  mkdir -p "${root}/bin" "${root}/systemd"
  write_binary "${root}/bin/log-query-mcp" new
  write_binary "${root}/bin/log-query-mcp-stdio" new-stdio
  printf 'new-unit\n' >"${root}/systemd/log-query-mcp.service"
  cat >"${root}/BUILDINFO" <<'EOF'
package=log-query-mcp-v9.9.9-test
version=9.9.9
target=test
git_commit=test
git_ref=test
built_at_utc=2026-08-07T00:00:00Z
rustc=test
EOF
  (
    cd "${root}"
    find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum >SHA256SUMS
  )
}

export LOG_QUERY_MCP_ALLOW_NON_ROOT=1
export LOG_QUERY_MCP_INSTALL_ROOT="${install_root}"
export LOG_QUERY_MCP_CONFIG_PATH="${config_path}"
export LOG_QUERY_MCP_UNIT_PATH="${unit_path}"
export LOG_QUERY_MCP_BACKUP_ROOT="${backup_root}"
export LOG_QUERY_MCP_SYSTEMCTL="${fake_systemctl}"
export LOG_QUERY_MCP_ROLLBACK_SCRIPT="${repo_root}/scripts/rollback.sh"
export LOG_QUERY_MCP_HEALTHCHECK_CMD="${install_root}/bin/log-query-mcp --health"
export FAKE_SYSTEMCTL_STATE="${state_file}"
export FAKE_SYSTEMCTL_FAIL_NEXT_RESTART="${fail_next_restart}"

package_root="${tmp}/log-query-mcp-v9.9.9-test"

# 1. Normal upgrade keeps config, installs new binaries/unit, and leaves a usable rollback backup.
reset_old_install
make_package "${package_root}"
"${repo_root}/scripts/upgrade.sh" "${package_root}"
[[ "$("${install_root}/bin/log-query-mcp")" == new ]]
[[ "$(cat "${config_path}")" == '{"sentinel":"keep-me"}' ]]
[[ "$(cat "${unit_path}")" == new-unit ]]
[[ "$(cat "${state_file}")" == active ]]
mapfile -t backups < <(find "${backup_root}" -mindepth 1 -maxdepth 1 -type d | sort)
[[ "${#backups[@]}" -eq 1 ]]

# 2. Explicit rollback restores the exact pre-upgrade binary/config/unit state.
"${repo_root}/scripts/rollback.sh" "${backups[0]}"
[[ "$("${install_root}/bin/log-query-mcp")" == old ]]
[[ "$(cat "${config_path}")" == '{"sentinel":"keep-me"}' ]]
[[ "$(cat "${unit_path}")" == old-unit ]]
[[ "$(cat "${state_file}")" == active ]]

# 3. A failed post-upgrade restart triggers automatic rollback.
reset_old_install
make_package "${package_root}"
touch "${fail_next_restart}"
if "${repo_root}/scripts/upgrade.sh" "${package_root}"; then
  echo "expected upgrade to fail when restart fails" >&2
  exit 1
fi
[[ "$("${install_root}/bin/log-query-mcp")" == old ]]
[[ "$(cat "${config_path}")" == '{"sentinel":"keep-me"}' ]]
[[ "$(cat "${unit_path}")" == old-unit ]]
[[ "$(cat "${state_file}")" == active ]]

# 4. Corrupt packages fail before any mutation.
reset_old_install
make_package "${package_root}"
printf 'corruption\n' >>"${package_root}/bin/log-query-mcp"
if "${repo_root}/scripts/upgrade.sh" "${package_root}"; then
  echo "expected checksum failure" >&2
  exit 1
fi
[[ "$("${install_root}/bin/log-query-mcp")" == old ]]
[[ "$(cat "${unit_path}")" == old-unit ]]
[[ -z "$(find "${backup_root}" -mindepth 1 -maxdepth 1 -type d -print -quit)" ]]

# 5. Archive input follows the same verified path.
reset_old_install
make_package "${package_root}"
archive="${tmp}/log-query-mcp-v9.9.9-test.tar.gz"
tar -C "${tmp}" -czf "${archive}" "$(basename "${package_root}")"
"${repo_root}/scripts/upgrade.sh" "${archive}"
[[ "$("${install_root}/bin/log-query-mcp")" == new ]]
[[ "$(cat "${config_path}")" == '{"sentinel":"keep-me"}' ]]

echo "upgrade_rollback_test: all scenarios passed"
