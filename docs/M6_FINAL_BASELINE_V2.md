# Log Query MCP v2 M6 Final Baseline

> 状态：repository implementation complete / final candidate CI externally blocked  
> 日期：2026-08-08  
> 分支：`feat/v2-m1-backend-config`  
> Draft PR：#25

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

`IMPLEMENTED` / `DONE` 指仓库实现、测试入口和发布工具已经补齐。它不等价于最新 candidate commit 已经取得最终 GitHub Actions 全绿；Actions 当前仍被 Billing/Spending Limit 在 runner 启动前阻止，因此 Release Candidate Gate 为 **BLOCKED**。

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

A dedicated single/dual-server concurrency harness is wired into the real two-OpenSSH live workflow. Its newest elapsed metrics remain unrecorded because Actions Billing blocks the runner before execution.

## 5. Release / Package Readiness

The release package contains:

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

`scripts/rollback.sh` restores the exact pre-upgrade runtime state and performs restart + health check. A final static review found that ordinary `cp` during restore could lose the backed-up group ownership of service-readable files such as `root:log-query-mcp` config. This was fixed in commit `838782ec27316b4493083bc37dafd8445f8e975c` by preserving backup ownership during atomic restore.

The isolated upgrade/rollback test was strengthened in commit `33c6e07c54a0d9729903020f6518e098e6d1e276` to verify config/unit modes. It was then executed outside GitHub Actions and passed all scenarios:

- successful upgrade；
- config preservation (`0640`)；
- explicit rollback；
- unit mode restoration (`0644`)；
- restart-failure automatic rollback；
- corrupt checksum fail-before-mutation；
- tar.gz upgrade input。

The release package validator was also previously exercised with valid and deliberately corrupted inputs; corrupt integrity checks fail closed.

## 7. Documentation / Review Readiness

Production docs are aligned with v2:

- `README.md`；
- `INSTALL.md`；
- `OPERATIONS.md`；
- `PRODUCTION_CHECKLIST.md`；
- `RELEASE_READINESS_V2.md`；
- `M6_PERFORMANCE_BASELINE_V2.md`；
- `M6_SECURITY_FAULT_MATRIX_V2.md`。

Draft PR `#25` (`feat: add v2 remote SSH/SFTP log query backend`) is open against `main` and intentionally remains Draft while the candidate CI cannot run.

## 8. External blocker

A second rerun attempt on 2026-08-08 was again rejected before any runner step. Rust job `92936278060` reported:

```text
The job was not started because recent account payments have failed
or your spending limit needs to be increased.
```

Therefore this is **not a code/test failure**, but it also means the newest candidate commit has **not** received a valid final CI pass.

Required external action:

1. resolve GitHub Billing / Spending Limit；
2. rerun candidate Rust / Contracts / SSH / Release gates；
3. record concurrency metrics；
4. require all critical gates green；
5. only then mark PR ready / merge / tag / publish。

## 9. Publication boundary

Current state:

```text
Draft PR                       CREATED (#25)
merge to main                  BLOCKED by Final Gate
tag / GitHub Release           BLOCKED by Final Gate
production deployment          requires target environment acceptance
```

No merge, tag or release must be forced while the final candidate gate cannot execute.

## 10. Final state

```text
repository implementation       COMPLETE
production docs                 COMPLETE
release packaging               COMPLETE
upgrade/rollback tooling        COMPLETE
local release-script validation PASS
rollback ownership hardening    PASS (local isolated validation)
historical M1-M6 live evidence  PASS
Draft PR                        OPEN (#25)
latest candidate CI             BLOCKED (GitHub Billing)
RC Ready                        NO, until candidate gates rerun green
formal Release                  NOT CREATED
production target acceptance    PENDING target environment
```
