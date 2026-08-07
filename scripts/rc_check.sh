#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "rc_check: $*" >&2
  exit 1
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

target="${TARGET:-x86_64-unknown-linux-gnu}"
out_dir="${RC_OUT_DIR:-dist/rc-check}"

command -v cargo >/dev/null 2>&1 || die "cargo is required"
command -v rustc >/dev/null 2>&1 || die "rustc is required"
command -v python3 >/dev/null 2>&1 || die "python3 is required"
command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required"
command -v tar >/dev/null 2>&1 || die "tar is required"

python3 - <<'PY'
try:
    import jsonschema  # noqa: F401
except ImportError as exc:
    raise SystemExit("rc_check: python package jsonschema is required") from exc
PY

for script in \
  scripts/check_release_tag.sh \
  scripts/install.sh \
  scripts/uninstall.sh \
  scripts/healthcheck.sh \
  scripts/upgrade.sh \
  scripts/rollback.sh \
  scripts/package_release.sh \
  scripts/validate_release_package.sh \
  scripts/rc_check.sh \
  tests/healthcheck_test.sh \
  tests/upgrade_rollback_test.sh; do
  bash -n "${script}"
done

echo "rc_check: contracts"
python3 scripts/validate_contracts.py

echo "rc_check: rustfmt"
cargo fmt --all -- --check

echo "rc_check: clippy"
cargo clippy --locked --all-targets --all-features -- -D warnings

echo "rc_check: tests"
cargo test --locked --all-targets --all-features

echo "rc_check: release binaries"
cargo build --release --locked --bins --target "${target}"

echo "rc_check: protocol health-check failure matrix"
bash tests/healthcheck_test.sh

echo "rc_check: upgrade/rollback failure matrix"
bash tests/upgrade_rollback_test.sh

echo "rc_check: release package"
rm -rf "${out_dir}"
bash scripts/package_release.sh --target "${target}" --out-dir "${out_dir}" --require-docs
archive="$(find "${out_dir}" -maxdepth 1 -type f -name 'log-query-mcp-v*.tar.gz' -print -quit)"
[[ -n "${archive}" ]] || die "release archive was not produced"
bash scripts/validate_release_package.sh "${archive}" "${out_dir}/SHA256SUMS"

echo "rc_check: PASS"
echo "rc_check: note: real SSH/SFTP, multi-server concurrency and target-production acceptance remain separate live gates"
