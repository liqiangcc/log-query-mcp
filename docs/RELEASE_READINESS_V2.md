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
M6 Performance    100 MiB full + 1 GiB full + 10 GiB logical tail baseline
Release           transport smoke + package assembly + package validation + upgrade/rollback test
```

Performance evidence is recorded in `M6_PERFORMANCE_BASELINE_V2.md`. It is an engineering baseline, not an SLA.

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
scripts/upgrade.sh
scripts/rollback.sh
docs/INSTALL.md
docs/OPERATIONS.md
docs/PRODUCTION_CHECKLIST.md
docs/M6_PERFORMANCE_BASELINE_V2.md
docs/RELEASE_READINESS_V2.md
BUILDINFO
SHA256SUMS
```

`BUILDINFO` records version, target, Git commit/ref, UTC build time, and rustc version. The package contains an internal `SHA256SUMS`; the release directory also publishes a checksum for the archive itself.

`scripts/validate_release_package.sh` must accept the generated archive and verify both package completeness and checksums.

## 4. Upgrade and rollback contract

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
6. execute a health check;
7. automatically invoke rollback if a post-mutation step fails.

Manual rollback is:

```bash
sudo scripts/rollback.sh /var/lib/log-query-mcp/backups/<backup-dir>
```

The automated `tests/upgrade_rollback_test.sh` covers successful upgrade, explicit rollback, failed-restart automatic rollback, checksum failure before mutation, config preservation, and archive input.

## 5. Security invariants

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

## 6. Production acceptance

CI evidence is necessary but not sufficient for the first production deployment. The target environment must still execute `PRODUCTION_CHECKLIST.md`, including:

- target Linux/kernel/glibc validation;
- real service-account and log permissions;
- real known_hosts and Secret provisioning;
- cache capacity review;
- MCP initialize/tools/search/context smoke tests;
- SSH outage, log rotation, restart, upgrade and rollback operator exercises where applicable.

Items that have not been executed on the target server remain `待验收`; CI must never mark them as completed on behalf of production operations.

## 7. Release flow

```text
candidate commit
    ↓
all CI gates green
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

Creating/merging the PR and creating the release tag are separate GitHub publication actions and must be explicitly authorized at the time they are performed.
