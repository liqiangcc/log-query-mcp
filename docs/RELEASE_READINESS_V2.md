# Log Query MCP v2 Release Readiness

> Status: production-readiness contract for v2  
> Scope: Local Source + Remote SSH/SFTP Source + optional administrator-configured ProxyCommand Transport

## 1. Release boundary

v2 keeps the public MCP surface intentionally small:

```text
list_log_sources
search_logs
get_log_context
```

The release supports local Linux logs and administrator-configured remote SSH/SFTP logs. Remote SSH can use Direct TCP or an optional ProxyCommand raw-stream adapter. Remote logs are synchronized into a local generation cache before the existing scanner/query engine reads them.

ProxyCommand does not create a new MCP tool. It is only a connection adapter below SSH:

```text
Direct       : TCP -> SSH -> auth/known_hosts -> SFTP
ProxyCommand : admin program/argv -> raw stdin/stdout -> SSH -> auth/known_hosts -> SFTP
```

The release does **not** provide remote shell execution, arbitrary path reads, upload/write/delete, application deployment, remote restart, a generic SSH MCP API, dynamic client-supplied proxy commands, or credential injection into ProxyCommand.

## 2. Required automated gates

A release candidate is not ready unless all of the following are green on the candidate commit:

```text
Rust                         cargo fmt + clippy -D warnings + all tests + release build
Contracts                    v1/v2 config/schema contract validation
Direct SSH Transport         auth + host key + range read + M4/M5 + M6 security/fault/live gates
M7 ProxyCommand              success + strict host-key live path
M7 Proxy Auth                password/private/encrypted-key auth through ProxyCommand
M7 Proxy Sync                full/tail/from_now/incremental/rotation/truncate
M7 ProxyCommand Failures     startup/EOF/timeout/cancel/auth/crash/Broken/isolation
M7 Mixed Query               Local + Direct + Proxy + failed-Proxy isolation
M7 Proxy Restart             stale-cache fail-closed + restart/recovery
M7 Proxy Generation          cursor/match_ref/generation/cache-only context
M7 Proxy Performance         Direct/Proxy paired profiles + 300 reads + concurrency + cleanup
Release                      transport smoke + protocol health + package validation + upgrade/rollback
```

M6 historical performance evidence is recorded in `M6_PERFORMANCE_BASELINE_V2.md`; M7 current-candidate performance must come from `M7 Proxy Performance`. Historical elapsed numbers are comparison evidence, not a substitute for current M7 execution and not an SLA.

For non-live validation outside GitHub Actions, `bash scripts/rc_check.sh` runs all repository-local gates in one command. It also validates the ProxyCommand release contract: Direct + Proxy example shape, allowed placeholders, packaged v2 machine schema and required M7 delivery documents. It does **not** replace real Direct/Proxy SSH live gates, M7 performance evidence, or target WSL/production acceptance.

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
schemas/log-query-mcp-config-v2.schema.json
systemd/log-query-mcp.service
scripts/install.sh
scripts/uninstall.sh
scripts/healthcheck.sh
scripts/upgrade.sh
scripts/rollback.sh
docs/INSTALL.md
docs/OPERATIONS.md
docs/PRODUCTION_CHECKLIST.md
docs/CONFIG_SCHEMA_V2.md
docs/PROXY_COMMAND_TRANSPORT_V2.md
docs/M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md
docs/M7_PROXY_COMMAND_LIVE_GATE_V2.md
docs/M7_PROXY_AUTH_GATE_V2.md
docs/M7_PROXY_SYNC_GATE_V2.md
docs/M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md
docs/M7_PROXY_RESTART_GATE_V2.md
docs/M7_PROXY_GENERATION_GATE_V2.md
docs/M7_PROXY_PERFORMANCE_GATE_V2.md
docs/M6_PERFORMANCE_BASELINE_V2.md
docs/M6_FINAL_BASELINE_V2.md
docs/RELEASE_READINESS_V2.md
BUILDINFO
SHA256SUMS
```

The packaged v2 example must retain at least one Direct SSH connection and at least one `proxy.type=command` connection. Packaged ProxyCommand placeholders are restricted to whole-argument `{host}` / `{port}`. The packaged machine schema must contain `ProxyCommandConfig`.

`BUILDINFO` records version, target, Git commit/ref, UTC build time, and rustc version. The package contains an internal `SHA256SUMS`; the release directory also publishes a checksum for the archive itself.

`scripts/validate_release_package.sh` must accept the generated archive and verify package completeness, executable release helpers, ProxyCommand release-contract shape, internal checksums, outer archive checksum and BUILDINFO/version consistency.

## 4. Protocol health contract

`scripts/healthcheck.sh` is the standard production health check. By default it verifies both:

1. `systemctl is-active log-query-mcp.service`;
2. an MCP `initialize` request to `http://127.0.0.1:8000/mcp` returns a `jsonrpc=2.0` response whose `serverInfo` identifies `log-query-mcp` and does not contain a JSON-RPC error.

A process being alive is therefore not sufficient for a successful upgrade.

The health check supports controlled overrides through `LOG_QUERY_MCP_*` variables for alternate URL, systemctl path, curl path and timeout. `LOG_QUERY_MCP_HEALTHCHECK_SKIP_SYSTEMD=1` is reserved for container/test environments where protocol-only validation is intentional.

`tests/healthcheck_test.sh` covers healthy service + valid MCP response, inactive service, JSON-RPC error, wrong server, HTTP/transport failure and explicit protocol-only mode.

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

For deployments using ProxyCommand, post-upgrade and post-rollback acceptance must additionally re-run a known Proxy source query and verify helper cleanup. Upgrade/rollback must not rewrite the existing production ProxyCommand configuration with the packaged example.

## 6. Security invariants

The release must preserve these invariants:

- Local Source remains rooted in the v1 `openat2()` safety model.
- Remote Source uses SSH + SFTP only; no SSH Exec or shell channel is exposed.
- Host key verification fails closed.
- Credentials are resolved from Secret references and are not returned in MCP errors/results.
- Remote file selection is administrator configured; AI requests cannot provide host, username, root, credential, proxy, or arbitrary path.
- ProxyCommand is administrator configured only and uses direct `program + argv[]` process creation; the application does not build Shell command strings.
- ProxyCommand placeholders are limited to whole-argument `{host}` / `{port}` and do not include credentials, usernames, source IDs or remote paths.
- ProxyCommand stdout is the SSH raw byte stream; stderr is bounded/diagnostic and must not be returned raw to AI.
- ProxyCommand does not receive SSH password, private-key content or passphrase.
- Strict known_hosts identity remains the logical SSH target `host:port`, not the proxy process, Windows host or localhost.
- Proxy child lifecycle is tied to the SSH session; timeout/cancellation/failure/normal close must not leave orphan helpers.
- Direct and Proxy sessions share the same global SSH concurrency limit.
- Remote logs are regular-file-only and synchronized into a bounded local cache.
- Partial Tail/FromNow cache coverage returns `CACHE_SCOPE_EXCEEDED` rather than a false negative.
- Remote/Proxy failure does not silently fall back to stale cache; v2 keeps `allow_stale_on_error=false` fail-closed behavior.
- Cache files do not contain SSH credentials and use restrictive permissions.

## 7. Production acceptance

CI evidence is necessary but not sufficient for the first production deployment. The target environment must still execute `PRODUCTION_CHECKLIST.md`, including:

- target Linux/kernel/glibc validation;
- real service-account and log permissions;
- real known_hosts and Secret provisioning;
- cache capacity review;
- MCP initialize/tools/search/context smoke tests;
- SSH/Proxy outage, log rotation, restart, upgrade and rollback operator exercises where applicable.

For M7, at least one traceable real WSL acceptance is required before RC completion when ProxyCommand is release-critical for the intended WSL/host-network use case:

```text
WSL Direct path unavailable
    +
Windows Host/VPN path available
    +
service identity can launch approved Windows helper
    +
ProxyCommand -> SSH handshake -> strict known_hosts -> auth -> SFTP
    +
list_log_sources/search_logs/get_log_context PASS
    +
no orphan helper / no sensitive error leakage
```

Items that have not been executed on the target server remain `待验收`; CI must never mark them as completed on behalf of production operations.

## 8. Current external verification blocker

M7 core implementation, functional/performance harnesses and Release Integration are present on Draft PR #25, but the current candidate GitHub Actions jobs remain blocked before runner start by the account Billing/Spending Limit condition tracked in GitHub Issue #23.

This must be treated as **verification blocked**, not as PASS and not as a known code failure. `steps=null` means the job did not execute its test steps.

Therefore the current state is:

```text
implementation/harness       present
release integration          present
current candidate CI         blocked
current M7 performance data  none
real WSL acceptance          pending
RC Ready                     NO
```

After Billing is restored, rerun the same candidate workflows and record all missing live/performance/package evidence before declaring RC Ready.

## 9. Release flow

```text
candidate commit
    ↓
repository-local rc_check PASS
    ↓
Direct SSH + all M7 live/performance gates green
    ↓
real WSL acceptance recorded
    ↓
release dry-run package validated
    ↓
PR Ready / review
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

Marking the PR Ready, merging the PR and creating the release tag are separate publication actions. A formal Release must not be created while Issue #23 remains unresolved, real WSL acceptance is missing, or any required Final Gate is not green.
