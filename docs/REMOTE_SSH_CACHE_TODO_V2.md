# Log Query MCP v2 Remote SSH/Cache 实施 TODO

> 状态：M0-M6 historical implementation complete / M7 implementation + harness + release integration complete / Final RC not ready  
> 日期：2026-08-10  
> 总方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)  
> ProxyCommand 方案：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> M7 TODO：[`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)  
> M7 基线：[`M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md`](./M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md)  
> M6 最终基线：[`M6_FINAL_BASELINE_V2.md`](./M6_FINAL_BASELINE_V2.md)  
> Draft PR：#25  
> External CI blocker：Issue #23

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
- ProxyCommand 只能作为管理员配置的 SSH raw byte-stream Transport，不能成为通用命令执行能力。
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

## 2. M6 Security / Fault / Multi-Server — HISTORICAL DONE

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

M7 修改了 SSH Transport，因此这些历史证据仍是重要回归基线，但不能替代当前 candidate 的 Direct/M7 live Gate。

## 3. M6 Performance — HISTORICAL BASELINE COMPLETE

- [x] 100 MiB full bootstrap。
- [x] 1 GiB full bootstrap。
- [x] 10 GiB logical Tail(64 MiB) bootstrap。
- [x] unchanged continuity probe = 64 KiB。
- [x] append only transfers payload + bounded probes。
- [x] local cache scan = 0 remote bytes。
- [x] 300 continuous range-read handle regression。
- [x] single-server 4-query benchmark harness。
- [x] dual-server concurrent-query benchmark harness。
- [x] historical large-file evidence recorded。

基线：[`M6_PERFORMANCE_BASELINE_V2.md`](./M6_PERFORMANCE_BASELINE_V2.md)

M7 已新增 Direct/Proxy paired performance gate；M6 elapsed 数字不作为 M7 PASS threshold。

## 4. M6 Production Readiness — PRE-M7 BASELINE COMPLETE

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
- [x] rollback restores previous state and must pass protocol health。
- [x] isolated lifecycle tests cover successful upgrade / explicit rollback / restart failure / corrupt package / tar input。

M7 Release Integration 已在此基础上补充 ProxyCommand schema/example/docs/package contract。

## 5. M7 ProxyCommand — IMPLEMENTED / VALIDATION BLOCKED

### Design / config

- [x] `PROXY_COMMAND_TRANSPORT_V2.md`。
- [x] ADR-0012。
- [x] machine JSON Schema `ProxyCommandConfig`。
- [x] Rust config/runtime validator。
- [x] Direct + Proxy shared stream connector。
- [x] whole-argv `{host}` / `{port}` placeholders。

### Process / security

- [x] direct `program + argv[]` spawn，无 Shell command string。
- [x] stdin/stdout raw SSH stream。
- [x] bounded stderr。
- [x] timeout/cancellation/kill/reap lifecycle baseline。
- [x] stable ProxyCommand startup/stream/timeout internal errors。
- [x] strict known_hosts/auth/SFTP 保持在 Proxy 层之上。
- [x] no new MCP Tool / Exec / write / upload / arbitrary path。

### Functional harnesses

- [x] real ProxyCommand → OpenSSH → SFTP success path。
- [x] wrong host key fail-closed。
- [x] password/private/encrypted-key auth。
- [x] full/tail/from_now/incremental/rotation/truncate Sync。
- [x] startup/permission/EOF/stderr flood/timeout/cancellation/auth/crash failure matrix。
- [x] child cleanup + semaphore release。
- [x] Direct + Proxy transport isolation。
- [x] Local + Direct + Proxy mixed query。
- [x] failed Proxy source isolation。
- [x] server restart / stale-cache fail-closed / recovery。
- [x] cursor/match_ref/generation pin consistency。

### Performance harness

- [x] Direct/Proxy 5-session setup paired measurement。
- [x] 100 MiB / 1 GiB / 10 GiB-tail paired profiles。
- [x] bounded unchanged/incremental transfer assertions。
- [x] Proxy 300 range reads。
- [x] 2 Direct + 2 Proxy concurrency。
- [x] normal-path helper orphan checks。
- [x] environment/metrics/time/disk artifact wiring。
- [ ] actual M7 performance metrics — **BLOCKED by Billing**。

### Release Integration

- [x] README Direct/Proxy/WSL/security guidance。
- [x] INSTALL service-identity/helper/systemd-hardening guidance。
- [x] OPERATIONS diagnostics/error categories/lifecycle guidance。
- [x] PRODUCTION_CHECKLIST Proxy/WSL/performance acceptance。
- [x] v2 example contains Direct + Proxy connections/sources。
- [x] release package includes v2 machine schema and M7 delivery docs。
- [x] release validator checks Direct+Proxy example, placeholders and machine schema。
- [x] `rc_check.sh` includes ProxyCommand release-contract non-live check。

详细阶段与验收项见 [`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)。

## 6. WSL Acceptance — PENDING REAL TARGET

必须形成可追溯的真实证据：

```text
WSL Direct target path unavailable
    +
Windows Host/VPN path available
    +
log-query-mcp service identity launches approved Windows helper
    +
ProxyCommand -> SSH -> strict known_hosts -> auth -> SFTP
    +
list_log_sources/search_logs/get_log_context PASS
    +
normal/failure/cancel helper cleanup PASS
```

- [ ] direct path 确认不可用。
- [ ] Windows Host/VPN path 可用。
- [ ] service identity Windows executable interop PASS。
- [ ] strict host-key/auth/SFTP PASS。
- [ ] 三个 MCP 工具 PASS。
- [ ] helper lifecycle / error redaction PASS。

## 7. Final Gate — NOT READY

当前必须完成：

- [ ] `bash scripts/rc_check.sh` on final candidate PASS。
- [ ] candidate Rust Gate PASS。
- [ ] candidate Contracts Gate PASS。
- [ ] candidate Direct SSH Gate PASS。
- [ ] candidate M7 ProxyCommand success/auth/sync/failure/mixed/restart/generation Gates PASS。
- [ ] candidate M7 Proxy Performance PASS + metrics/artifact recorded。
- [ ] candidate Release/package/lifecycle PASS。
- [ ] WSL acceptance evidence recorded。
- [ ] no unexplained critical failure。

GitHub Actions Billing/Spending Limit 仍是外部 blocker，Issue #23 为 canonical tracking issue。`steps=null` 既不是 code PASS，也不是已知 code failure。

## 8. 发布 / 环境动作

- [x] Draft PR #25 已创建，继续保持 Draft。
- [ ] Mark PR Ready — **全部 Final Gate + WSL acceptance 后，且需显式授权**。
- [ ] merge main — Ready/review 后且需显式授权。
- [ ] 创建 `v{Cargo.toml version}` tag — Final Gate 全绿后且需显式授权。
- [ ] GitHub Release — tag workflow，Final Gate 未绿时禁止发布。
- [ ] 目标生产服务器 INSTALL / Remote / ProxyCommand / AI-client / upgrade / rollback 人工验收。

## 9. 当前判断

```text
M0-M6 historical implementation       COMPLETE
M6 historical evidence                COMPLETE / reference only
M7 ProxyCommand design/config/core    IMPLEMENTED
M7 functional/fault/query harnesses   IMPLEMENTED
M7 performance harness                IMPLEMENTED
M7 release integration                IMPLEMENTED
M7 current-candidate execution        BLOCKED externally
M7 WSL acceptance                     PENDING target environment
GitHub Actions Billing                BLOCKED externally
Draft PR                              OPEN (#25)
RC Ready                              NO
formal Release                        NOT CREATED
production target acceptance          PENDING
```

当前下一步应从“继续扩实现”切换到**真实 WSL acceptance 准备与 Final Gate 执行**。在 Billing 恢复、所有 candidate gates 真正 PASS、WSL evidence 完成之前，不应把 PR 标 Ready、merge、tag 或 release。
