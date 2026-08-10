# Log Query MCP v2 Remote SSH/Cache 实施 TODO

> 状态：M0-M6 implementation complete / M7 ProxyCommand planned / Final RC not ready  
> 日期：2026-08-10  
> 总方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)  
> ProxyCommand 方案：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> M7 TODO：[`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)  
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
- ProxyCommand 若启用，也只能作为 SSH raw byte stream Transport，不能成为通用命令执行能力。
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

## 4. M6-F Production Readiness — DONE FOR PRE-M7 BASELINE

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
- [x] README / INSTALL / OPERATIONS / PRODUCTION_CHECKLIST / RELEASE_READINESS / M6_FINAL aligned with pre-M7 v2 baseline。
- [x] Draft PR #25 updated with pre-M7 production-readiness scope。

Release contract：[`RELEASE_READINESS_V2.md`](./RELEASE_READINESS_V2.md)

> M7 修改 SSH Transport 后，上述 release/readiness 文档和验证证据必须重新对齐，不能直接把 pre-M7 结果作为最终 RC。

## 5. M7 ProxyCommand — DESIGN ACCEPTED / IMPLEMENTATION PENDING

目标：支持 WSL / Container 等环境通过宿主机或管理员指定 helper 建立 SSH 底层字节流，同时不扩大 Log Query MCP 的命令权限。

设计与决策：

- [x] `PROXY_COMMAND_TRANSPORT_V2.md` 已提交。
- [x] ADR-0012 已接受：ProxyCommand 只作为 SSH Stream Transport。
- [x] `CONFIG_SCHEMA_V2.md` 已定义目标 ProxyCommand 配置契约。
- [x] 独立 `PROXY_COMMAND_TODO_V2.md` 已建立。

实现入口：

- [ ] 修正 ProxyCommand 设计文档中的历史 ADR 编号引用为 ADR-0012。
- [ ] 更新机器 JSON Schema。
- [ ] 更新 Rust config/runtime validator。
- [ ] 抽取 Direct/Proxy 共用 SSH stream connector。
- [ ] 实现 ProxyCommand process stdin/stdout stream。
- [ ] 实现 timeout/cancellation/child cleanup/bounded stderr/redaction。
- [ ] Direct SSH 全量 regression。
- [ ] ProxyCommand SSH/SFTP live gate。
- [ ] Multi-server + failure isolation。
- [ ] WSL → Windows Host → Remote SSH acceptance。
- [ ] large-file / concurrency / process-leak regression。
- [ ] README / INSTALL / OPERATIONS / release docs 对齐。
- [ ] 完整 Final RC Gate 重跑。

详细阶段与验收项见 [`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)。

## 6. Final Gate — NOT READY

### 6.1 既有外部阻塞

pre-M7 candidate 的 GitHub Actions 仍存在 Billing/Spending Limit 外部阻塞。历史候选运行在 runner 启动前被拒绝，这不是已知代码失败，但也不能标记为 PASS。

Canonical external blocker：Issue #23。

### 6.2 M7 引入后的新边界

在接受 M7 ProxyCommand 后，即使 Billing 恢复，也不能直接把旧 candidate 标成 RC READY。

原因：

```text
M7 changes SSH transport/config/security/process lifecycle
↓
old candidate evidence is not the final candidate
↓
M7 implementation must complete
↓
new candidate must rerun all critical gates
```

最终必须完成：

- [ ] M7 implementation complete。
- [ ] `bash scripts/rc_check.sh` on final candidate PASS。
- [ ] candidate Rust Gate PASS。
- [ ] candidate Contracts Gate PASS。
- [ ] candidate Direct + Proxy SSH live Gate PASS。
- [ ] candidate M6/M7 security/fault Gate PASS。
- [ ] candidate WSL acceptance evidence recorded。
- [ ] candidate single/dual/mixed concurrency metrics recorded。
- [ ] candidate Release Gate PASS（shell syntax / smoke / protocol health / lifecycle / package validation）。
- [ ] M7 transport change 后重跑相关 large-file Performance。
- [ ] 确认没有 unexplained critical failure，然后才可把 RC 标成 Ready。

## 7. 发布 / 环境动作

- [x] Draft PR #25 已创建，继续保持 Draft。
- [ ] Mark PR Ready / merge main — **M7 + Final Gate 全绿后且需显式授权**。
- [ ] 创建 `v{Cargo.toml version}` tag — Final Gate 全绿后且需显式授权。
- [ ] GitHub Release — tag workflow，Final Gate 未绿时禁止发布。
- [ ] 目标生产服务器 INSTALL / Remote / ProxyCommand / AI-client / upgrade / rollback 人工验收 — 必须在真实目标环境执行。

## 8. 当前判断

```text
M0-M6 design/implementation             COMPLETE
pre-M7 security/fault implementation    COMPLETE
pre-M7 performance harness              COMPLETE
pre-M7 production/release tooling       COMPLETE
M7 ProxyCommand design                  COMPLETE
M7 ADR/config target contract           COMPLETE
M7 implementation                       PENDING
M7 WSL acceptance                       PENDING
final candidate regression              PENDING
GitHub Actions Billing                  BLOCKED externally
Draft PR                                OPEN (#25)
RC Ready                                NO
formal Release                          NOT CREATED
production target acceptance            PENDING target environment
```

当前正确下一步不再是直接等待 Billing：应先完成 M7 ProxyCommand 的实现与验证准备；Billing 恢复后在最终 M7 candidate 上执行完整 Final Gate，然后再进入受控的 Ready / merge / tag / release / 生产验收流程。
