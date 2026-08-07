#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/package_release.sh [options]

Options:
  --target <triple>     Release target triple. Default: x86_64-unknown-linux-gnu
  --out-dir <dir>       Output directory. Default: dist
  --bin-dir <dir>       Directory containing release binaries.
  --tag <tag>           Validate v* tag against Cargo.toml version before packaging.
  --require-docs        Fail if production operations docs are missing.
  -h, --help            Show this help.
EOF
}

die() {
  echo "package_release: $*" >&2
  exit 1
}

target="x86_64-unknown-linux-gnu"
out_dir="dist"
bin_dir=""
tag=""
require_docs=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      target="${2:-}"
      [[ -n "${target}" ]] || die "--target requires a value"
      shift 2
      ;;
    --out-dir)
      out_dir="${2:-}"
      [[ -n "${out_dir}" ]] || die "--out-dir requires a value"
      shift 2
      ;;
    --bin-dir)
      bin_dir="${2:-}"
      [[ -n "${bin_dir}" ]] || die "--bin-dir requires a value"
      shift 2
      ;;
    --tag)
      tag="${2:-}"
      [[ -n "${tag}" ]] || die "--tag requires a value"
      shift 2
      ;;
    --require-docs)
      require_docs=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

if [[ -n "${tag}" ]]; then
  scripts/check_release_tag.sh "${tag}"
fi

version="$(
  cargo metadata --no-deps --format-version 1 |
    python3 -c 'import json,sys; m=json.load(sys.stdin); print(next(p["version"] for p in m["packages"] if p["name"] == "log-query-mcp"))'
)"
package_name="log-query-mcp-v${version}-${target}"

if [[ -z "${bin_dir}" ]]; then
  if [[ -x "target/${target}/release/log-query-mcp" ]]; then
    bin_dir="target/${target}/release"
  else
    bin_dir="target/release"
  fi
fi

[[ -x "${bin_dir}/log-query-mcp" ]] || die "missing executable ${bin_dir}/log-query-mcp"
[[ -x "${bin_dir}/log-query-mcp-stdio" ]] || die "missing executable ${bin_dir}/log-query-mcp-stdio"
[[ -f examples/log-query-mcp.v1.json ]] || die "missing v1 example config"
[[ -f examples/log-query-mcp.v2.remote.json ]] || die "missing v2 remote example config"
[[ -f systemd/log-query-mcp.service ]] || die "missing systemd unit"
[[ -f scripts/install.sh ]] || die "missing install script"
[[ -f scripts/uninstall.sh ]] || die "missing uninstall script"

rm -rf "${out_dir:?}/${package_name}"
mkdir -p "${out_dir}/${package_name}/bin"

install -m 0755 "${bin_dir}/log-query-mcp" "${out_dir}/${package_name}/bin/log-query-mcp"
install -m 0755 "${bin_dir}/log-query-mcp-stdio" "${out_dir}/${package_name}/bin/log-query-mcp-stdio"
install -D -m 0644 examples/log-query-mcp.v1.json "${out_dir}/${package_name}/examples/log-query-mcp.v1.json"
install -D -m 0644 examples/log-query-mcp.v2.remote.json "${out_dir}/${package_name}/examples/log-query-mcp.v2.remote.json"
install -D -m 0644 systemd/log-query-mcp.service "${out_dir}/${package_name}/systemd/log-query-mcp.service"
install -D -m 0755 scripts/install.sh "${out_dir}/${package_name}/scripts/install.sh"
install -D -m 0755 scripts/uninstall.sh "${out_dir}/${package_name}/scripts/uninstall.sh"

for doc in docs/INSTALL.md docs/OPERATIONS.md docs/PRODUCTION_CHECKLIST.md; do
  if [[ -f "${doc}" ]]; then
    install -D -m 0644 "${doc}" "${out_dir}/${package_name}/${doc}"
  elif [[ "${require_docs}" -eq 1 ]]; then
    die "missing required production doc ${doc}"
  else
    echo "package_release: warning: ${doc} not present; PR E will add production docs" >&2
  fi
done

git_commit="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
git_ref="$(git describe --always --dirty --tags 2>/dev/null || echo unknown)"
built_at_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
rustc_version="$(rustc --version)"

cat >"${out_dir}/${package_name}/BUILDINFO" <<EOF
package=${package_name}
version=${version}
target=${target}
git_commit=${git_commit}
git_ref=${git_ref}
built_at_utc=${built_at_utc}
rustc=${rustc_version}
EOF

(
  cd "${out_dir}/${package_name}"
  find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum
) >"${out_dir}/${package_name}/SHA256SUMS"

archive="${out_dir}/${package_name}.tar.gz"
rm -f "${archive}"
tar -C "${out_dir}" -czf "${archive}" "${package_name}"

(
  cd "${out_dir}"
  sha256sum "$(basename "${archive}")" >SHA256SUMS
)

echo "package_release: wrote ${archive}"
echo "package_release: wrote ${out_dir}/SHA256SUMS"
