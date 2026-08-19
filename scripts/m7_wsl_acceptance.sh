#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
expected_user="${M7_WSL_EXPECTED_USER:-log-query-mcp}"
actual_user="$(id -un)"

if [[ "${M7_WSL_ALLOW_USER_MISMATCH:-0}" != "1" && "${actual_user}" != "${expected_user}" ]]; then
  echo "m7_wsl_acceptance: SERVICE_IDENTITY_MISMATCH: expected ${expected_user}, got ${actual_user}" >&2
  echo "m7_wsl_acceptance: run the real acceptance as the configured service identity" >&2
  exit 1
fi

exec python3 "${script_dir}/m7_wsl_acceptance.py" "$@"
