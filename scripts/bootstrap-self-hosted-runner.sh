#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  RUNNER_TOKEN=... RUNNER_PACKAGE_URL=... ./scripts/bootstrap-self-hosted-runner.sh

Optional:
  RUNNER_URL=https://github.com/liqiangcc/log-query-mcp
  RUNNER_NAME=verification-pilot-<hostname>
  RUNNER_WORK_DIR=/tmp/log-query-mcp-actions-runner

Purpose:
  Register and run one ephemeral GitHub Actions self-hosted runner dedicated to
  the verification pilot. The runner gets label `verification-pilot` and exits
  after one assigned job.

Security:
  - RUNNER_TOKEN must be a temporary registration token from GitHub Settings.
  - The token is never written into this repository.
  - Do not run this on a production host or a machine containing production secrets.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

: "${RUNNER_TOKEN:?RUNNER_TOKEN is required}"
: "${RUNNER_PACKAGE_URL:?RUNNER_PACKAGE_URL is required; copy the Linux x64 archive URL from GitHub Settings -> Actions -> Runners -> New self-hosted runner}"

RUNNER_URL="${RUNNER_URL:-https://github.com/liqiangcc/log-query-mcp}"
RUNNER_NAME="${RUNNER_NAME:-verification-pilot-$(hostname)}"
RUNNER_WORK_DIR="${RUNNER_WORK_DIR:-/tmp/log-query-mcp-actions-runner}"

case "$RUNNER_WORK_DIR" in
  /tmp/*) ;;
  *)
    echo "RUNNER_WORK_DIR must be under /tmp for this pilot" >&2
    exit 2
    ;;
esac

if [[ -e "$RUNNER_WORK_DIR" ]]; then
  echo "Runner work directory already exists: $RUNNER_WORK_DIR" >&2
  echo "Remove it only after confirming no runner process is using it." >&2
  exit 2
fi

mkdir -p "$RUNNER_WORK_DIR"
cd "$RUNNER_WORK_DIR"

cleanup() {
  if [[ -f ./config.sh && -f .runner ]]; then
    echo "Runner directory retained for diagnostics: $RUNNER_WORK_DIR" >&2
  fi
}
trap cleanup EXIT

archive=actions-runner.tar.gz
curl --fail --location --proto '=https' --tlsv1.2 "$RUNNER_PACKAGE_URL" --output "$archive"
tar xzf "$archive"
rm -f "$archive"

./config.sh \
  --unattended \
  --ephemeral \
  --url "$RUNNER_URL" \
  --token "$RUNNER_TOKEN" \
  --name "$RUNNER_NAME" \
  --labels verification-pilot \
  --work _work

unset RUNNER_TOKEN

echo "Starting ephemeral runner '$RUNNER_NAME' for $RUNNER_URL"
echo "Expected label: verification-pilot"
echo "The process exits after one assigned job."
exec ./run.sh
