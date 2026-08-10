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
  scripts/m7_wsl_acceptance.sh \
  scripts/rc_check.sh \
  tests/healthcheck_test.sh \
  tests/upgrade_rollback_test.sh; do
  bash -n "${script}"
done

python3 -m py_compile scripts/m7_wsl_acceptance.py

echo "rc_check: contracts"
python3 scripts/validate_contracts.py

echo "rc_check: ProxyCommand release contract"
python3 - <<'PY'
import json
from pathlib import Path

example_path = Path("examples/log-query-mcp.v2.remote.json")
schema_path = Path("schemas/log-query-mcp-config-v2.schema.json")
example = json.loads(example_path.read_text(encoding="utf-8"))
schema = json.loads(schema_path.read_text(encoding="utf-8"))

connections = example.get("connections", [])
assert any("proxy" not in connection for connection in connections), "v2 example must retain Direct SSH"
proxy_connections = [
    connection
    for connection in connections
    if connection.get("proxy", {}).get("type") == "command"
]
assert proxy_connections, "v2 example must contain ProxyCommand"
for connection in proxy_connections:
    proxy = connection["proxy"]
    assert proxy.get("program"), "ProxyCommand program must be non-empty"
    args = proxy.get("args", [])
    assert "{host}" in args and "{port}" in args, "WSL ProxyCommand example needs {host}/{port}"
    for argument in args:
        if "{" in argument or "}" in argument:
            assert argument in {"{host}", "{port}"}, "unsupported ProxyCommand placeholder"
assert "ProxyCommandConfig" in schema.get("$defs", {}), "v2 schema missing ProxyCommandConfig"
PY

echo "rc_check: WSL acceptance client static precheck"
python3 scripts/m7_wsl_acceptance.py \
  --validate-config-only \
  --config examples/log-query-mcp.v2.remote.json \
  --source-id inventory-remote-via-host >/dev/null

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
echo "rc_check: note: Direct SSH, M7 ProxyCommand live/auth/sync/failure/restart/generation/performance gates and real WSL/production acceptance remain separate live gates"