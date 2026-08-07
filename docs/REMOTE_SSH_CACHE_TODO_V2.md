# Log Query MCP v2 Remote SSH/Cache 实施 TODO

> 状态：Repository implementation complete / Final RC CI blocked by GitHub Actions Billing  
> 日期：2026-08-08  
> 总方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)  
> 最终基线：[`M6_FINAL_BASELINE_V2.md`](./M6_FINAL_BASELINE_V2.md)  
> Draft PR：#25  
> External blocker：Issue #23

## 0. 冻结原则

v2 全部实现继续遵守：

- AI-facing 工具只有 `list_log_sources`、`search_logs`、`get_log_context`。
- 不新增 `ssh_exec` / Shell / arbitrary remote path / write / upload / deploy。
- Local Source 的 v1 `openat2()` 安全语义不回退。
- Remote Source 只通过管理员配置的 SSH/SFTP read-only Transport 获取日志。
- Remote 日志必须先形成稳定本地 generation snapshot，再进入 Scanner / Query Engine。
- 默认 fail-closed，不静默使用 stale cache。
- Tail/FromNow 覆盖不足必须返回 `CACHE_SCOPE_EXCEEDED`，不能制造假阴性。
- SSH 是内部 Transport，不是业务 API。
- Cache generation + snapshot length 是 Remote cursor / match_ref 的稳定边界。

## 1. M0～M5 — DONE

- [x] M0：ADR、v2 config/error contract、`russh + russh-sftp` research。
- [x] M1：v1/v2 config routing、`SourceBackend` / `LocalBackend`。
- [x] M2：SecretResolver、Password/private/encrypted key、strict known_hosts、只读 SFTP、timeout/broken-session/semaphore、300 range-read handle regression。
- [x] M3：opaque Cache IDs、0700/0600、atomic generation/append、crash recovery、snapshot pin、quota/GC。
- [x] M4：full/tail/from_now、incremental append、continuity fingerprint、rotation/truncate/replacement、sync budget。
- [x] M5：Remote discovery/query through local cache、Local+Remote、cursor/match_ref generation consistency、`CACHE_SCOPE_EXCEEDED`。

实现基线：

- [`M1_IMPLEMENTATION_BASELINE_V2.md`](./M1_IMPLEMENTATION_BASELINE_V2.md)
- [`M2_IMPLEMENTATION_BASELINE_V2.md`](./M2_IMPLEMENTATION_BASELINE_V2.md)
- [`M3_IMPLEMENTATION_BASELINE_V2.md`](./M3_IMPLEMENTATION_BASELINE_V2.md)
- [`M4_IMPLEMENTATION_BASELINE_V2.md`](./M4_IMPLEMENTATION_BASELINE_V2.md)
- [`M5_IMPLEMENTATION_BASELINE_V2.md`](./M5_IMPLEMENTATION_BASELINE_V2.md)

## 2. M6-A～D Security / Fault / Multi-Server — DONE

- [x] AI cannot submit SSH host/user/credential/root/arbitrary path。
- [x] Remote symlink escape rejected；regular-file-only。
- [x] no Remote Exec / Shell / write / upload / delete。
- [x] redacted public errors/results。
- [x] cache corruption/orphan/quota/recovery evidence。
- [x] auth / host-key / timeout / disconnect / cancellation failure matrix。
- [x] server restart fail-closed + recovery。
- [x] two independent SSH servers + Local mixed query。
- [x] global SSH semaphore + one-server failure isolation。

矩阵：[`M6_SECURITY_FAULT_MATRIX_V2.md`](./M6_SECURITY_FAULT_MATRIX_V2.md)

## 3. M6-E Performance — IMPLEMENTATION COMPLETE

Large-file evidence：

- [x] 100 MiB full bootstrap。
- [x] 1 GiB full bootstrap。
- [x] 10 GiB logical Tail(64 MiB) bootstrap。
- [x] unchanged continuity probe = 64 KiB。
- [x] append only transfers payload + bounded probes。
- [x] local cache scan = 0 remote bytes。
- [x] 300 continuous range-read handle regression。

Concurrency：

- [x] single-server 4-query benchmark harness。
- [x] dual-server concurrent-query benchmark harness。
- [x] harness wired into real two-OpenSSH SSH Transport workflow。
- [ ] newest live elapsed metrics — **BLOCKED before runner start by GitHub Actions Billing**。

基线：[`M6_PERFORMANCE_BASELINE_V2.md`](./M6_PERFORMANCE_BASELINE_V2.md)

## 4. M6-F Production Readiness — DONE

### Release / package

- [x] binaries + v1/v2 examples + systemd + production docs。
- [x] install / uninstall / `healthcheck.sh` / upgrade / rollback helpers。
- [x] `BUILDINFO` + inner/outer `SHA256SUMS`。
- [x] package validator checks shape, executable helpers, checksum and BUILDINFO/version consistency。
- [x] Release package job defaults to `contents: read`。
- [x] only tag publish job receives `contents: write` and depends on verified package job。

### Protocol health / lifecycle

- [x] standard health check requires systemd active + valid MCP `initialize` response。
- [x] protocol-health failure matrix covers inactive service / JSON-RPC error / wrong server / transport failure。
- [x] upgrade verifies checksum before mutation, preserves config, backs up runtime state and atomically replaces runtime files。
- [x] restart/protocol-health failure triggers automatic rollback。
- [x] rollback restores exact previous state and must pass protocol health。
- [x] rollback preserves backed-up ownership/modes for service-readable config/unit。
- [x] isolated lifecycle tests cover successful upgrade / explicit rollback / restart failure / corrupt package / tar input。

### Final Candidate helper / docs

- [x] `scripts/rc_check.sh` runs all repository-local non-live gates in one command。
- [x] Rust、Contracts、SSH Transport、Release expose manual rerun paths；M6 Performance already exposes profile-based `workflow_dispatch`。
- [x] README / INSTALL / OPERATIONS / PRODUCTION_CHECKLIST / RELEASE_READINESS / M6_FINAL aligned with v2。
- [x] Draft PR #25 updated with final production-readiness scope。

Release contract：[`RELEASE_READINESS_V2.md`](./RELEASE_READINESS_V2.md)

## 5. Final Gate — EXTERNALLY BLOCKED

Latest verified repository head before the final baseline-only update:

```text
d38337b855d59649cb0143f515238510b992b2e4
```

On that exact head GitHub created these PR candidate runs:

```text
Rust      31200874586
Contracts 31200875744
Release   31200875028
```

All required package/test jobs were rejected **before runner execution**. Rust and Contracts jobs had `steps=[]`; Release package had `steps=[]` and its dependent publish job was correctly skipped. GitHub's annotation states:

```text
The job was not started because recent account payments have failed
or your spending limit needs to be increased.
```

因此这不是已知代码失败，但也绝不能标成 PASS。

Canonical blocker：Issue #23。

Billing 恢复后剩余的是验证动作，不是继续开发：

- [ ] run `bash scripts/rc_check.sh` on the candidate source/environment and record result。
- [ ] candidate Rust Gate PASS。
- [ ] candidate Contracts Gate PASS。
- [ ] candidate SSH live Gate PASS，记录 single/dual concurrency metrics。
- [ ] candidate Release Gate PASS（shell syntax / smoke / protocol health / lifecycle / package validation）。
- [ ] 若 transport/sync/performance 相关代码相对成功 large-file evidence 有变化，则重跑 M6 Performance；否则记录可追溯复用理由。
- [ ] 确认没有 unexplained critical failure，然后才可把 RC 标成 Ready。

## 6. 发布 / 环境动作

这些不是仓库剩余实现任务：

- [x] Draft PR #25 已创建，保持 Draft。
- [ ] Mark PR Ready / merge main — Final Gate 全绿后且需显式授权。
- [ ] 创建 `v{Cargo.toml version}` tag — Final Gate 全绿后且需显式授权。
- [ ] GitHub Release — tag workflow，Final Gate 未绿时禁止发布。
- [ ] 目标生产服务器 INSTALL / Remote / AI-client / upgrade / rollback 人工验收 — 必须在真实目标环境执行。

## 7. 最终判断

```text
known v2 repository design work        0 remaining
known v2 implementation work           0 remaining
security/fault implementation          COMPLETE
performance harness implementation     COMPLETE
production/release tooling             COMPLETE
production documentation               COMPLETE
Draft PR                               OPEN (#25)
latest candidate CI                    BLOCKED externally (Billing)
RC Ready                               NO
formal Release                         NOT CREATED
production target acceptance           PENDING target environment
```

继续修改业务代码不会解决当前阻塞，只会制造新的 candidate。下一步应是解除 Issue #23 的外部 Billing 阻塞并执行 Final Gate；随后才进入受控的 Ready/merge/tag/release/生产验收流程。
