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

if [[ -n "${outer_sums}" ]]; then
  [[ -f "${outer_sums}" ]] || die "outer checksum file not found"
  (
    cd "$(dirname "${archive}")"
    sha256sum -c "$(realpath "${outer_sums}")"
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
  systemd/log-query-mcp.service \
  scripts/install.sh \
  scripts/uninstall.sh \
  scripts/upgrade.sh \
  scripts/rollback.sh \
  docs/INSTALL.md \
  docs/OPERATIONS.md \
  docs/PRODUCTION_CHECKLIST.md \
  docs/M6_PERFORMANCE_BASELINE_V2.md \
  docs/RELEASE_READINESS_V2.md \
  BUILDINFO \
  SHA256SUMS; do
  [[ -f "${root}/${path}" ]] || die "missing package entry: ${path}"
done

for path in bin/log-query-mcp bin/log-query-mcp-stdio scripts/install.sh scripts/uninstall.sh scripts/upgrade.sh scripts/rollback.sh; do
  [[ -x "${root}/${path}" ]] || die "expected executable package entry: ${path}"
done

(
  cd "${root}"
  sha256sum -c SHA256SUMS
) >/dev/null || die "internal package checksum failed"

version="$(awk -F= '$1 == "version" {print $2}' "${root}/BUILDINFO")"
[[ -n "${version}" ]] || die "BUILDINFO does not contain version"
expected_prefix="log-query-mcp-v${version}-"
[[ "$(basename "${root}")" == "${expected_prefix}"* ]] || die "package directory and BUILDINFO version disagree"

grep -q '^git_commit=' "${root}/BUILDINFO" || die "BUILDINFO missing git_commit"
grep -q '^target=' "${root}/BUILDINFO" || die "BUILDINFO missing target"
grep -q '^rustc=' "${root}/BUILDINFO" || die "BUILDINFO missing rustc"
grep -q '^built_at_utc=' "${root}/BUILDINFO" || die "BUILDINFO missing built_at_utc"

echo "validate_release_package: package is complete and checksums are valid"
