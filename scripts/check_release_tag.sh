#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "check_release_tag: $*" >&2
  exit 1
}

if [[ $# -ne 1 ]]; then
  die "usage: $0 v<version>"
fi

tag="$1"
case "${tag}" in
  v*) ;;
  *) die "release tag must start with v, got ${tag}" ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

tag_version="${tag#v}"
cargo_version="$(
  cargo metadata --no-deps --format-version 1 |
    python3 -c 'import json,sys; m=json.load(sys.stdin); print(next(p["version"] for p in m["packages"] if p["name"] == "log-query-mcp"))'
)"

if [[ "${tag_version}" != "${cargo_version}" ]]; then
  die "tag ${tag} does not match Cargo.toml package.version ${cargo_version}"
fi

echo "check_release_tag: ${tag} matches Cargo.toml package.version"
