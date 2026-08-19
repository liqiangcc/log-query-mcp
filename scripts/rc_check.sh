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

python3 -m py_compile \
  scripts/verify_m7_evidence.py \
  scripts/m7_real_target_manifest.py

echo "rc_check: contracts"
python3 scripts/validate_contracts.py

echo "rc_check: Direct-only v2 release contract"
python3 - <<'PY'
import json
from pathlib import Path

example_path = Path("examples/log-query-mcp.v2.remote.json")
example = json.loads(example_path.read_text(encoding="utf-8"))

connections = example.get("connections", [])
assert any("proxy" not in connection for connection in connections), "v2 example must retain Direct SSH"
assert all("proxy" not in connection for connection in connections), "v2 release example must be Direct-only"
PY

echo "rc_check: M7 evidence verifier synthetic self-test"
python3 scripts/verify_m7_evidence.py --self-test

echo "rc_check: M7 run manifest synthetic self-test"
python3 scripts/m7_real_target_manifest.py self-test

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
echo "rc_check: note: verifier self-test and run-manifest self-test/lifecycle are synthetic/non-live only; Direct SSH live/performance gates and target production evidence remain separate gates; ProxyCommand is deferred post-v2"
