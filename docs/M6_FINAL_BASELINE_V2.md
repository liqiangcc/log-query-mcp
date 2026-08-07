# Log Query MCP v2 M6 Final Baseline

> 状态：implementation complete / final candidate CI verification externally blocked  
> 日期：2026-08-07  
> 分支：`feat/v2-m1-backend-config`

## 1. 结论

v2 的 Remote SSH/SFTP + Local Cache 主链路与 M6 production-hardening 工作已经实现完成：

```text
M0 Contract / Research       DONE
M1 Backend / Config          DONE
M2 SSH/SFTP Transport        DONE
M3 CacheStore                DONE
M4 SyncEngine                DONE
M5 Remote Query Integration  DONE
M6-A Security/Fault Matrix   DONE
M6-B Negative/Fault Tests    DONE
M6-C Multi-Server Acceptance DONE
M6-D Cache/Readonly/Redaction DONE
M6-E Performance             IMPLEMENTED
M6-F Production Readiness    DONE
```

这里的 `IMPLEMENTED` 与 `DONE` 指仓库实现和验证入口已经补齐，不代表最新 candidate commit 已经取得最终 GitHub Actions 全绿。最新 rerun 被 GitHub Actions Billing/Spending Limit 在 runner 启动前阻止，因此 Release Candidate Gate 仍为 **BLOCKED**。

## 2. 生产能力边界

AI-facing MCP 工具仍然只有：

```text
list_log_sources
search_logs
get_log_context
```

支持：

- Local Source；
- administrator-configured Remote SSH Source；
- Password / private key / encrypted private key；
- strict host-key verification；
- SFTP read-only transport；
- incremental local generation cache；
- full / tail / from_now bootstrap；
- append / rotation / truncate / replacement continuity handling；
- Local + multiple Remote mixed query；
- cursor / match_ref snapshot-generation consistency；
- bounded resource limits and fail-closed errors。

明确不支持：

```text
Remote Exec
SSH shell
arbitrary remote path read
upload/write/delete
application deployment/restart
remote grep command execution
```

## 3. 安全与故障证据

历史成功 SSH live Gate 已覆盖：

- Password / encrypted private-key authentication；
- wrong password / wrong key；
- missing / changed known_hosts；
- permission denied / missing remote file；
- bounded offset range read；
- 300 continuous range reads with deterministic SFTP handle close；
- operation timeout → Broken；
- network disconnect → Broken；
- cancellation / semaphore permit release；
- Remote symlink rejection；
- M4 sync through real SFTP；
- M5 Remote Query through SFTP + Cache；
- two independent SSH servers；
- global SSH semaphore；
- one-server failure isolation；
- server restart / fail-closed / recovery；
- old generation / cursor / match_ref stability。

Cache fault tests cover manifest/data corruption, orphan staging, quota and recovery boundaries. Public errors/results continue to avoid Secret, absolute remote/cache path and backtrace leakage.

## 4. 性能证据

Large-file evidence run `31195030284` succeeded and is recorded in `M6_PERFORMANCE_BASELINE_V2.md`.

Key engineering properties:

```text
100 MiB full cold bootstrap ~4.9 s on recorded runner
1 GiB full cold bootstrap   ~48.6 s
10 GiB logical tail(64MiB)  ~3.1 s, ~64MiB payload cached
unchanged probe             64 KiB remote read
cache local scan            0 remote bytes
append                       payload + bounded probes only
```

A dedicated single/dual-server concurrency harness has been added and wired into the live SSH workflow. Its newest live elapsed metrics are intentionally not recorded because GitHub Actions Billing blocked the runner before execution.

## 5. Release / Package Readiness

The release package now contains:

- both production binaries；
- v1/v2 example config；
- systemd unit；
- install / uninstall / upgrade / rollback scripts；
- production docs and M6 baselines；
- `BUILDINFO`；
- internal `SHA256SUMS`；
- outer archive `SHA256SUMS`。

`scripts/validate_release_package.sh` validates package shape, executable entries, internal checksum, outer checksum and BUILDINFO/version consistency.

Release workflow uses least privilege:

```text
package job: contents: read
publish job: contents: write only for tag
```

Tag publication only happens after the verified package job succeeds.

## 6. Upgrade / Rollback Readiness

`scripts/upgrade.sh`:

1. validates package checksum before mutation；
2. backs up binaries / BUILDINFO / config / systemd unit；
3. preserves production config during normal upgrade；
4. uses same-directory temporary file + rename replacement；
5. reloads/restarts service；
6. runs health check；
7. automatically invokes rollback after post-mutation failure。

`scripts/rollback.sh` restores the exact pre-upgrade runtime state and performs restart + health check.

`tests/upgrade_rollback_test.sh` was executed locally in an isolated filesystem model and passed:

- successful upgrade；
- config preservation；
- explicit rollback；
- restart-failure automatic rollback；
- corrupt checksum fail-before-mutation；
- tar.gz upgrade input。

The release package validator was also locally exercised with a valid package and a deliberately corrupted archive; valid input passed and corrupt outer checksum failed closed.

## 7. Documentation Readiness

Production docs are now aligned with v2:

- `README.md`；
- `INSTALL.md`；
- `OPERATIONS.md`；
- `PRODUCTION_CHECKLIST.md`；
- `RELEASE_READINESS_V2.md`；
- `M6_PERFORMANCE_BASELINE_V2.md`；
- `M6_SECURITY_FAULT_MATRIX_V2.md`。

They explicitly document Local vs Remote boundary, read-only SSH/SFTP, Secret/known_hosts, cache capacity, host-key rotation, Remote failure semantics, safe upgrade/rollback and unsupported Remote Exec/Deploy.

## 8. External blocker

The newest candidate workflows were rejected before any runner steps because GitHub reported:

```text
The job was not started because recent account payments have failed
or your spending limit needs to be increased.
```

Therefore this is **not a code/test failure**, but it also means the newest candidate commit has **not** received a valid final CI pass.

Required action outside the repository:

1. resolve GitHub Billing / Spending Limit；
2. rerun candidate Rust / Contracts / SSH / Release gates；
3. record concurrency metrics；
4. require all critical gates green；
5. only then mark RC Ready。

## 9. Publication boundary

This branch is implementation-ready but has not been silently published.

The following remain separate explicit GitHub actions:

```text
create PR
merge to main
create version tag
publish GitHub Release
production deployment/acceptance
```

They must not be performed merely because implementation work is complete. Release also must not be created while the final CI gate is blocked.

## 10. Final state

```text
repository implementation       COMPLETE
production docs                 COMPLETE
release packaging               COMPLETE
upgrade/rollback tooling        COMPLETE
local release-script validation PASS
historical M1-M6 live evidence  PASS
latest candidate CI             BLOCKED (GitHub Billing)
RC Ready                        NO, until candidate gates rerun green
formal Release                  NOT CREATED
production target acceptance    PENDING target environment
```
