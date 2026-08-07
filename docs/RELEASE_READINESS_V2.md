# Log Query MCP v2 Release Readiness

> Status: production-readiness contract for v2  
> Scope: Local Source + Remote SSH/SFTP Source

## 1. Release boundary

v2 keeps the public MCP surface intentionally small:

```text
list_log_sources
search_logs
get_log_context
```

The release supports local Linux logs and administrator-configured remote SSH/SFTP logs. Remote logs are synchronized into a local generation cache before the existing scanner/query engine reads them.

The release does **not** provide remote shell execution, arbitrary path reads, upload/write/delete, application deployment, remote restart, or a generic SSH MCP API.

## 2. Required automated gates

A release candidate is not ready unless all of the following are green on the candidate commit:

```text
Rust              cargo fmt + clippy -D warnings + all tests + release build
Contracts         v1/v2 config/schema contract validation
SSH Transport     auth + host key + range read + M4/M5 + M6 security/fault/live gates
M6 Performance    large-file baseline + single/dual-server concurrency evidence
Release           transport smoke + protocol health + package validation + upgrade/rollback test
```

Performance evidence is recorded in `M6_PERFORMANCE_BASELINE_V2.md`. It is an engineering baseline, not an SLA.

Rust, Contracts, SSH Transport and Release workflows expose `workflow_dispatch` so the same candidate commit can be rerun after an external runner outage without creating an unrelated commit.

For non-live validation outside GitHub Actions, `bash scripts/rc_check.sh` runs all repository-local gates in one command. It does **not** replace the real SSH/SFTP multi-server live gate or target-production acceptance.

## 3. Release package contract

The release archive is named:

```text
log-query-mcp-v{version}-x86_64-unknown-linux-gnu.tar.gz
```

The package must contain at least:

```text
bin/log-query-mcp
bin/log-query-mcp-stdio
examples/log-query-mcp.v1.json
examples/log-query-mcp.v2.remote.json
systemd/log-query-mcp.service
scripts/install.sh
scripts/uninstall.sh
scripts/healthcheck.sh
scripts/upgrade.sh
scripts/rollback.sh
docs/INSTALL.md
docs/OPERATIONS.md
docs/PRODUCTION_CHECKLIST.md
docs/M6_PERFORMANCE_BASELINE_V2.md
docs/M6_FINAL_BASELINE_V2.md
docs/RELEASE_READINESS_V2.md
BUILDINFO
SHA256SUMS
```

`BUILDINFO` records version, target, Git commit/ref, UTC build time, and rustc version. The package contains an internal `SHA256SUMS`; the release directory also publishes a checksum for the archive itself.

`scripts/validate_release_package.sh` must accept the generated archive and verify package completeness, executable release helpers, internal checksums, outer archive checksum and BUILDINFO/version consistency.

## 4. Protocol health contract

`scripts/healthcheck.sh` is the standard production health check. By default it verifies both:

1. `systemctl is-active log-query-mcp.service`;
2. an MCP `initialize` request to `http://127.0.0.1:8000/mcp` returns a `jsonrpc=2.0` response whose `serverInfo` identifies `log-query-mcp` and does not contain a JSON-RPC error.

A process being alive is therefore not sufficient for a successful upgrade.

The health check supports controlled overrides through `LOG_QUERY_MCP_*` variables for alternate URL, systemctl path, curl path and timeout. `LOG_QUERY_MCP_HEALTHCHECK_SKIP_SYSTEMD=1` is reserved for container/test environments where protocol-only validation is intentional.

`tests/healthcheck_test.sh` covers:

- healthy service + valid MCP response;
- inactive service;
- JSON-RPC error;
- response from the wrong server;
- HTTP/transport failure;
- explicit protocol-only mode.

## 5. Upgrade and rollback contract

Production upgrade is performed with:

```bash
sudo scripts/upgrade.sh /path/to/log-query-mcp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
```

The upgrade process must:

1. verify the release package before mutation;
2. create a rollback backup of binaries, BUILDINFO, config, and systemd unit;
3. preserve the existing production config during a normal upgrade;
4. atomically replace runtime files using same-directory temporary files and rename;
5. reload/restart systemd;
6. execute the service + MCP protocol health check;
7. automatically invoke rollback if a post-mutation step fails.

Manual rollback is:

```bash
sudo scripts/rollback.sh /var/lib/log-query-mcp/backups/<backup-dir>
```

Rollback also requires the restored service to pass the same protocol-level health check.

The automated `tests/upgrade_rollback_test.sh` covers successful upgrade, explicit rollback, failed-restart automatic rollback, checksum failure before mutation, config preservation, and archive input.

## 6. Security invariants

The release must preserve these invariants:

- Local Source remains rooted in the v1 `openat2()` safety model.
- Remote Source uses SSH + SFTP only; no SSH Exec or shell channel is exposed.
- Host key verification fails closed.
- Credentials are resolved from Secret references and are not returned in MCP errors/results.
- Remote file selection is administrator configured; AI requests cannot provide host, username, root, credential, or arbitrary path.
- Remote logs are regular-file-only and synchronized into a bounded local cache.
- Partial Tail/FromNow cache coverage returns `CACHE_SCOPE_EXCEEDED` rather than a false negative.
- Remote failure does not silently fall back to stale cache unless an explicit future policy says otherwise; v2 defaults fail closed.
- Cache files do not contain SSH credentials and use restrictive permissions.

## 7. Production acceptance

CI evidence is necessary but not sufficient for the first production deployment. The target environment must still execute `PRODUCTION_CHECKLIST.md`, including:

- target Linux/kernel/glibc validation;
- real service-account and log permissions;
- real known_hosts and Secret provisioning;
- cache capacity review;
- MCP initialize/tools/search/context smoke tests;
- SSH outage, log rotation, restart, upgrade and rollback operator exercises where applicable.

Items that have not been executed on the target server remain `待验收`; CI must never mark them as completed on behalf of production operations.

## 8. Current external verification blocker

Repository implementation is complete, but the latest candidate GitHub Actions jobs are currently blocked before runner start by the account Billing/Spending Limit condition tracked in GitHub Issue #23.

This must be treated as **verification blocked**, not as PASS and not as a known code failure. After billing is restored, rerun the same candidate workflows and record the missing concurrency metrics before declaring RC Ready.

## 9. Release flow

```text
candidate commit
    ↓
repository-local rc_check PASS
    ↓
all required live/CI gates green
    ↓
release dry-run package validated
    ↓
merge to main
    ↓
tag v{Cargo.toml version}
    ↓
Release workflow validates tag/version
    ↓
GitHub Release publishes archive + SHA256SUMS
    ↓
operator verifies checksum
    ↓
install/upgrade
    ↓
production checklist
```

Creating/merging the PR and creating the release tag are separate GitHub publication actions and must be explicitly authorized at the time they are performed. A formal Release must not be created while Issue #23 remains unresolved or any required Final Gate is not green.
