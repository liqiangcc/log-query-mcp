# Log Query MCP v2 Remote SSH/Cache 实施 TODO

> 状态：Repository implementation complete / Final RC CI blocked by GitHub Actions Billing  
> 日期：2026-08-07  
> 总方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)  
> 最终基线：[`M6_FINAL_BASELINE_V2.md`](./M6_FINAL_BASELINE_V2.md)

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

## 1. M0：契约与技术预研 — DONE

- [x] ADR-0007～0011。
- [x] v2 config / error schema。
- [x] v1 + v2 Contracts Gate。
- [x] `russh + russh-sftp + Tokio` 技术预研。
- [x] Password / private key / encrypted key / known_hosts 路径验证。
- [x] 明确 Remote Exec 不属于架构。

## 2. M1：Config + Source Backend — DONE

- [x] v1/v2 配置版本路由。
- [x] `SourceBackend` / `LocalBackend` 抽象。
- [x] v2 Local Source 可运行。
- [x] Local `openat2()` 边界保持。

基线：[`M1_IMPLEMENTATION_BASELINE_V2.md`](./M1_IMPLEMENTATION_BASELINE_V2.md)

## 3. M2：SSH/SFTP Transport — DONE

- [x] `SecretResolver`。
- [x] Password / private key / encrypted private key。
- [x] strict Host Key Verification。
- [x] connect / operation timeout。
- [x] global SSH semaphore。
- [x] SFTP read-only `stat/lstat/read_dir/read_range`。
- [x] broken-session / cancellation / network fault 语义。
- [x] deterministic file-handle shutdown。
- [x] 300 continuous bounded range-read regression。
- [x] 无 exec/shell/write/upload。

基线：[`M2_IMPLEMENTATION_BASELINE_V2.md`](./M2_IMPLEMENTATION_BASELINE_V2.md)

## 4. M3：CacheStore — DONE

- [x] opaque internal IDs。
- [x] 0700 directory / 0600 file。
- [x] catalog / manifest persistence。
- [x] staging + atomic generation commit。
- [x] append continuation + crash tail recovery。
- [x] snapshot fixed length。
- [x] multi-generation / pin-aware GC。
- [x] global/per-source quota。
- [x] corruption/orphan recovery tests。

基线：[`M3_IMPLEMENTATION_BASELINE_V2.md`](./M3_IMPLEMENTATION_BASELINE_V2.md)

## 5. M4：SyncEngine — DONE

- [x] `full / tail / from_now` bootstrap。
- [x] incremental append。
- [x] 64 KiB continuity fingerprint。
- [x] truncate / replacement / continuity mismatch → new generation。
- [x] sync byte budget。
- [x] sync failure preserves last valid cache。
- [x] real SFTP → SyncEngine → CacheStore live gate。

基线：[`M4_IMPLEMENTATION_BASELINE_V2.md`](./M4_IMPLEMENTATION_BASELINE_V2.md)

## 6. M5：Remote Query Integration — DONE

- [x] Remote explicit files + non-recursive directory discovery。
- [x] regular-file / suffix / ordering validation。
- [x] on-query refresh。
- [x] Remote → Cache → existing Scanner / Query Engine。
- [x] Local + Remote mixed query。
- [x] cursor generation/snapshot pin。
- [x] cursor continuation does not refresh Remote。
- [x] match_ref generation pin。
- [x] `get_log_context` normal path uses 0 SSH。
- [x] old match_ref remains stable across rotation within TTL。
- [x] incomplete cache returns `CACHE_SCOPE_EXCEEDED`。
- [x] runtime v2 Remote/Sync/Cache error contract。

基线：[`M5_IMPLEMENTATION_BASELINE_V2.md`](./M5_IMPLEMENTATION_BASELINE_V2.md)

## 7. M6-A～D：Security / Fault / Multi-Server / Cache — DONE

- [x] AI cannot submit host / username / credential / arbitrary remote path。
- [x] Remote symlink escape rejected。
- [x] read-only transport boundary proven。
- [x] errors/results redact Secret and infrastructure paths。
- [x] cache permissions / corruption / external-deletion / recovery evidence。
- [x] auth / host-key / timeout / disconnect / cancellation failure matrix。
- [x] server restart fail-closed + recovery。
- [x] two independent SSH servers from one local MCP。
- [x] Password on Server A + encrypted private key on Server B。
- [x] one-server failure does not contaminate the other server cache。
- [x] Local + Server A + Server B mixed query。

矩阵：[`M6_SECURITY_FAULT_MATRIX_V2.md`](./M6_SECURITY_FAULT_MATRIX_V2.md)

## 8. M6-E：Performance — IMPLEMENTATION COMPLETE

Large-file evidence：

- [x] 100 MiB full bootstrap。
- [x] 1 GiB full bootstrap。
- [x] 10 GiB logical Tail(64 MiB) bootstrap。
- [x] unchanged continuity probe。
- [x] 1 MiB append。
- [x] 100 MiB append。
- [x] local cache scan = 0 remote bytes。
- [x] 300 continuous range-read handle regression。
- [x] global SSH session limit evidence。

Concurrency：

- [x] single-server 4-query concurrency benchmark harness implemented。
- [x] dual-server concurrent query benchmark harness implemented。
- [x] harness wired into real two-OpenSSH `SSH Transport` workflow。
- [ ] newest live elapsed metrics — **BLOCKED by GitHub Actions Billing before runner start**。

基线：[`M6_PERFORMANCE_BASELINE_V2.md`](./M6_PERFORMANCE_BASELINE_V2.md)

## 9. M6-F：Production Readiness — DONE

### Production docs

- [x] README Local + Remote + security + performance boundary。
- [x] INSTALL v1 Local / v2 Remote / Secret / known_hosts / cache / safe upgrade。
- [x] OPERATIONS Remote errors / host-key rotation / cache capacity / recovery。
- [x] PRODUCTION_CHECKLIST distinguishes automated evidence from target-server acceptance。
- [x] RELEASE_READINESS_V2 release contract。
- [x] M6_FINAL_BASELINE_V2 final implementation status。

### Release package

- [x] Package contains binaries / v1+v2 examples / systemd / production docs。
- [x] `BUILDINFO`。
- [x] internal `SHA256SUMS`。
- [x] outer archive `SHA256SUMS`。
- [x] `validate_release_package.sh` verifies package shape + inner/outer checksums + BUILDINFO/version consistency。
- [x] Release workflow package job defaults to `contents: read`。
- [x] only tag publish job receives `contents: write`。
- [x] publish depends on verified package job。

### Upgrade / rollback

- [x] `upgrade.sh` verifies checksum before mutation。
- [x] backup binaries / BUILDINFO / config / unit。
- [x] normal upgrade preserves production config。
- [x] same-directory temporary + rename replacement。
- [x] restart / health check。
- [x] automatic rollback after post-mutation failure。
- [x] explicit `rollback.sh`。
- [x] isolated test covers success / explicit rollback / restart failure / corrupt package / tar input。
- [x] isolated upgrade/rollback test executed locally and passed。
- [x] package validator valid/corrupt cases executed locally and behaved fail-closed。

Release contract：[`RELEASE_READINESS_V2.md`](./RELEASE_READINESS_V2.md)

## 10. Final Gate — EXTERNALLY BLOCKED

历史 M1～M6 relevant Rust / Contracts / SSH / Performance Gates 已经有成功证据。

最新 candidate 重新验证时，GitHub Actions 在 runner 启动前返回：

```text
The job was not started because recent account payments have failed
or your spending limit needs to be increased.
```

因此当前状态必须区分：

```text
repository implementation       COMPLETE
local release-script validation PASS
latest candidate Actions        BLOCKED by GitHub Billing
RC Ready                        NO
```

Billing 恢复后只剩验证动作，不再需要设计/实现新的 v2 功能：

- [ ] candidate Rust Gate PASS。
- [ ] candidate Contracts Gate PASS。
- [ ] candidate SSH live Gate PASS，记录 concurrency metrics。
- [ ] candidate Release package/upgrade Gate PASS。
- [ ] 如相关性能代码未变化，可引用 large-file run；否则重跑 M6 Performance。
- [ ] 确认没有 unexplained critical failure。

## 11. 不属于“仓库剩余实现”的后续动作

以下是独立发布/环境动作，不应在本 TODO 中被自动执行：

- [ ] 创建 PR（需单独授权）。
- [ ] 合并 main（需单独授权）。
- [ ] 创建 `v{version}` tag（需单独授权，且 Final Gate 必须先绿）。
- [ ] 发布 GitHub Release（tag workflow，需 Final Gate）。
- [ ] 目标生产服务器 INSTALL/Remote/AI-client/upgrade/rollback 人工验收。

## 12. 最终判断

v2 的仓库实现工作已经完成。当前唯一阻止“RC Ready / 正式 Release”的事项是 **GitHub Actions Billing 导致最新 candidate 无法执行 Final Gate**，以及随后需要明确授权的 PR/merge/tag/release/生产部署动作。
