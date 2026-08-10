#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: scripts/validate_release_package.sh <release.tar.gz> [outer-SHA256SUMS]" >&2
}

die() {
  echo "validate_release_package: $*" >&2
  exit 1
}

[[ $# -ge 1 && $# -le 2 ]] || { usage; exit 2; }
archive="$1"
outer_sums="${2:-}"
[[ -f "${archive}" ]] || die "archive not found: ${archive}"
archive="$(realpath "${archive}")"

if [[ -n "${outer_sums}" ]]; then
  [[ -f "${outer_sums}" ]] || die "outer checksum file not found"
  outer_sums="$(realpath "${outer_sums}")"
  (
    cd "$(dirname "${archive}")"
    sha256sum -c "${outer_sums}"
  ) >/dev/null || die "outer archive checksum failed"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT
tar -xzf "${archive}" -C "${tmp}"
mapfile -t roots < <(find "${tmp}" -mindepth 1 -maxdepth 1 -type d -print)
[[ "${#roots[@]}" -eq 1 ]] || die "archive must contain exactly one top-level directory"
root="${roots[0]}"

for path in \
  bin/log-query-mcp \
  bin/log-query-mcp-stdio \
  examples/log-query-mcp.v1.json \
  examples/log-query-mcp.v2.remote.json \
  schemas/log-query-mcp-config-v2.schema.json \
  systemd/log-query-mcp.service \
  scripts/install.sh \
  scripts/uninstall.sh \
  scripts/upgrade.sh \
  scripts/rollback.sh \
  scripts/healthcheck.sh \
  scripts/m7_wsl_acceptance.py \
  scripts/m7_wsl_acceptance.sh \
  scripts/m7_wsl_http_acceptance.py \
  scripts/verify_m7_evidence.py \
  docs/INSTALL.md \
  docs/OPERATIONS.md \
  docs/PRODUCTION_CHECKLIST.md \
  docs/CONFIG_SCHEMA_V2.md \
  docs/PROXY_COMMAND_TRANSPORT_V2.md \
  docs/M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md \
  docs/M7_PROXY_COMMAND_LIVE_GATE_V2.md \
  docs/M7_PROXY_AUTH_GATE_V2.md \
  docs/M7_PROXY_SYNC_GATE_V2.md \
  docs/M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md \
  docs/M7_PROXY_RESTART_GATE_V2.md \
  docs/M7_PROXY_GENERATION_GATE_V2.md \
  docs/M7_PROXY_PERFORMANCE_GATE_V2.md \
  docs/M7_WSL_ACCEPTANCE_V2.md \
  docs/M7_WSL_SYSTEMD_HTTP_ACCEPTANCE_V2.md \
  docs/M7_REAL_TARGET_EXECUTION_RUNBOOK_V2.md \
  docs/M6_PERFORMANCE_BASELINE_V2.md \
  docs/M6_FINAL_BASELINE_V2.md \
  docs/RELEASE_READINESS_V2.md \
  BUILDINFO \
  SHA256SUMS; do
  [[ -f "${root}/${path}" ]] || die "missing package entry: ${path}"
done

for path in \
  bin/log-query-mcp \
  bin/log-query-mcp-stdio \
  scripts/install.sh \
  scripts/uninstall.sh \
  scripts/upgrade.sh \
  scripts/rollback.sh \
  scripts/healthcheck.sh \
  scripts/m7_wsl_acceptance.py \
  scripts/m7_wsl_acceptance.sh \
  scripts/m7_wsl_http_acceptance.py \
  scripts/verify_m7_evidence.py; do
  [[ -x "${root}/${path}" ]] || die "expected executable package entry: ${path}"
done

(
  cd "${root}"
  sha256sum -c SHA256SUMS
) >/dev/null || die "internal package checksum failed"

python3 - "${root}/examples/log-query-mcp.v2.remote.json" "${root}/schemas/log-query-mcp-config-v2.schema.json" <<'PY'
import json
import sys

example_path, schema_path = sys.argv[1:]
with open(example_path, encoding="utf-8") as handle:
    example = json.load(handle)
with open(schema_path, encoding="utf-8") as handle:
    schema = json.load(handle)

connections = example.get("connections", [])
if not any("proxy" not in connection for connection in connections):
    raise SystemExit("release v2 example must retain at least one Direct SSH connection")
proxy_connections = [connection for connection in connections if connection.get("proxy", {}).get("type") == "command"]
if not proxy_connections:
    raise SystemExit("release v2 example must contain at least one ProxyCommand connection")
for connection in proxy_connections:
    proxy = connection["proxy"]
    if not proxy.get("program"):
        raise SystemExit("ProxyCommand example program must be non-empty")
    args = proxy.get("args", [])
    if "{host}" not in args or "{port}" not in args:
        raise SystemExit("ProxyCommand WSL example must contain {host} and {port}")
    for argument in args:
        if "{" in argument or "}" in argument:
            if argument not in {"{host}", "{port}"}:
                raise SystemExit("ProxyCommand example contains an unsupported placeholder")
if "ProxyCommandConfig" not in schema.get("$defs", {}):
    raise SystemExit("packaged v2 schema is missing ProxyCommandConfig")
PY

bash -n "${root}/scripts/m7_wsl_acceptance.sh" || die "packaged M7 WSL acceptance wrapper has invalid shell syntax"
python3 -m py_compile \
  "${root}/scripts/m7_wsl_acceptance.py" \
  "${root}/scripts/m7_wsl_http_acceptance.py" \
  "${root}/scripts/verify_m7_evidence.py" || \
  die "packaged M7 acceptance Python tooling has invalid syntax"
python3 "${root}/scripts/m7_wsl_acceptance.py" \
  --validate-config-only \
  --config "${root}/examples/log-query-mcp.v2.remote.json" \
  --source-id inventory-remote-via-host >/dev/null || \
  die "packaged M7 WSL acceptance client failed static config validation"
python3 "${root}/scripts/m7_wsl_http_acceptance.py" \
  --validate-config-only \
  --config "${root}/examples/log-query-mcp.v2.remote.json" \
  --source-id inventory-remote-via-host >/dev/null || \
  die "packaged M7 WSL HTTP acceptance client failed static config validation"
python3 "${root}/scripts/verify_m7_evidence.py" --self-test >/dev/null || \
  die "packaged M7 evidence verifier self-test failed"

version="$(awk -F= '$1 == "version" {print $2}' "${root}/BUILDINFO")"
[[ -n "${version}" ]] || die "BUILDINFO does not contain version"
expected_prefix="log-query-mcp-v${version}-"
[[ "$(basename "${root}")" == "${expected_prefix}"* ]] || die "package directory and BUILDINFO version disagree"

grep -q '^git_commit=' "${root}/BUILDINFO" || die "BUILDINFO missing git_commit"
grep -q '^target=' "${root}/BUILDINFO" || die "BUILDINFO missing target"
grep -q '^rustc=' "${root}/BUILDINFO" || die "BUILDINFO missing rustc"
grep -q '^built_at_utc=' "${root}/BUILDINFO" || die "BUILDINFO missing built_at_utc"

echo "validate_release_package: package is complete and checksums are valid"
