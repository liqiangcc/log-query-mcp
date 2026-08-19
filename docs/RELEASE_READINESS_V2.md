# Log Query MCP v2 Release Readiness

> Status: production-readiness contract for v2  
> Scope: Local Source + Remote SSH/SFTP Source over Direct TCP
>
> ProxyCommand is implemented in the repository but explicitly deferred to post-v2. It is not a v2.0 supported transport, release-package example, or release-blocking gate. The related code, tests, and design documents remain for a later v2.1/post-v2 validation cycle.

## 1. Release boundary

v2 keeps the public MCP surface intentionally small:

```text
list_log_sources
search_logs
get_log_context
```

The release supports local Linux logs and administrator-configured remote SSH/SFTP logs over Direct TCP. Remote logs are synchronized into a local generation cache before the existing scanner/query engine reads them.

The deferred ProxyCommand implementation does not create a new MCP tool. If enabled in a future post-v2 release, it remains only a connection adapter below SSH:

```text
Direct       : TCP -> SSH -> auth/known_hosts -> SFTP
ProxyCommand : admin program/argv -> raw stdin/stdout -> SSH -> auth/known_hosts -> SFTP
```

The release does **not** provide remote shell execution, arbitrary path reads, upload/write/delete, application deployment, remote restart, a generic SSH MCP API, dynamic client-supplied proxy commands, or credential injection into ProxyCommand.

## 2. Required automated gates

A v2.0 release candidate is not ready unless all of the following are green on the candidate commit:

```text
Rust                         cargo fmt + clippy -D warnings + all tests + release build
Contracts                    v1/v2 config/schema contract validation
Direct SSH Transport         auth + host key + range read + M4/M5 + M6 security/fault/live gates
Release                      transport smoke + protocol health + package validation + upgrade/rollback
```

M6 historical performance evidence is recorded in `M6_PERFORMANCE_BASELINE_V2.md`; current-candidate Direct performance evidence must be recorded separately. Historical elapsed numbers are comparison evidence, not an SLA. M7 ProxyCommand workflows remain post-v2 regression gates and are not required for v2.0.

For non-live validation outside GitHub Actions, `bash scripts/rc_check.sh` runs all repository-local v2 gates in one command. It validates the Direct-only v2 example and package contract. It does **not** replace real Direct SSH live/performance evidence or target production acceptance. ProxyCommand validation remains a separate post-v2 workflow and is not silently treated as a v2.0 PASS.

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
docs/M6_PERFORMANCE_BASELINE_V2.md
docs/M6_FINAL_BASELINE_V2.md
docs/RELEASE_READINESS_V2.md
BUILDINFO
SHA256SUMS
```

The packaged v2 example must contain at least one Direct SSH connection and must not contain a `proxy` connection. The v2.0 package does not promise ProxyCommand support. The machine schema may retain the deferred `ProxyCommandConfig` definition for forward compatibility, but it is not part of the v2.0 example or acceptance contract.

`BUILDINFO` records version, target, Git commit/ref, UTC build time, and rustc version. The package contains an internal `SHA256SUMS`; the release directory also publishes a checksum for the archive itself.

`scripts/validate_release_package.sh` must accept the generated archive and verify package completeness, Direct-only v2 example shape, internal checksums, outer archive checksum and BUILDINFO/version consistency.

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

ProxyCommand upgrade/rollback acceptance is deferred with the transport itself and is not a v2.0 release condition. v2.0 upgrade/rollback must preserve administrator configuration and pass the Direct/local protocol health checks.

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
- Direct SSH outage, log rotation, restart, upgrade and rollback operator exercises where applicable.

For v2.0, WSL/Windows-helper acceptance is deferred with ProxyCommand. If the selected deployment uses WSL, the release acceptance must cover the configured Direct TCP path; no Windows helper is required by the v2.0 contract:

```text
WSL service identity
    +
Direct TCP -> SSH handshake -> strict known_hosts -> auth -> SFTP
    +
list_log_sources/search_logs/get_log_context PASS
```

Items that have not been executed on the target server remain `待验收`; CI must never mark them as completed on behalf of production operations.

## 8. Current external verification blocker

Core v2 implementation and Direct release integration are present on Draft PR #25. The current candidate GitHub Actions jobs remain blocked before runner start by the account Billing/Spending Limit condition tracked in GitHub Issue #23. ProxyCommand jobs are retained as post-v2 validation and do not block the v2.0 release scope.

This must be treated as **verification blocked**, not as PASS and not as a known code failure. `steps=null` means the job did not execute its test steps.

Therefore the current state is:

```text
implementation/harness       present
release integration          present
current candidate CI         blocked
current Direct performance data pending
real Direct/WSL acceptance   pending where deployment requires it
ProxyCommand v2.0 gate       deferred / non-blocking
RC Ready                     NO
```

After Billing is restored, rerun the same candidate workflows and record all missing live/performance/package evidence before declaring RC Ready.

## 9. Release flow

```text
candidate commit
    ↓
repository-local rc_check PASS
    ↓
Direct SSH + current Direct performance gate green
    ↓
real Direct/WSL acceptance recorded where applicable
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

Marking the PR Ready, merging the PR and creating the release tag are separate publication actions. A formal Release must not be created while Issue #23 remains unresolved, required Direct/production acceptance is missing, or any required v2.0 Final Gate is not green. Deferred ProxyCommand jobs are not v2.0 Final Gates.
