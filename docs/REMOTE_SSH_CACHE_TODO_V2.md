# Log Query MCP v2 Remote SSH/Cache 实施 TODO

> 状态：M5 complete / Ready for M6  
> 日期：2026-08-07  
> 总方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)  
> 配置契约：[`CONFIG_SCHEMA_V2.md`](./CONFIG_SCHEMA_V2.md)  
> SSH/SFTP 预研：[`SSH_SFTP_TECHNICAL_RESEARCH_V2.md`](./SSH_SFTP_TECHNICAL_RESEARCH_V2.md)  
> 当前代码 Gate 基线：`53b15786510ebca63f30343f8f34b0c43fbb4edc`

## 0. 实施原则

所有实现继续遵守以下冻结边界：

- 不新增 `ssh_exec` / Shell / 任意文件读取 MCP 工具。
- MCP 工具继续保持 `list_log_sources`、`search_logs`、`get_log_context`。
- Local Source 的 v1 `openat2()` 安全语义不回退。
- Remote Source 只通过 SSH/SFTP 获取管理员配置的日志。
- Remote Source 必须先形成稳定本地 Snapshot，再进入查询引擎。
- 默认不静默使用 stale cache。
- 缓存覆盖不完整时不得返回假阴性的空结果。
- v1 配置和契约必须继续可用。
- SSH 是内部 Transport，不是业务 API。
- Cache generation 是 Remote Query 的稳定数据边界。

---

# 已完成里程碑

关闭的详细 checklist 不再在总 TODO 中重复维护；每个里程碑的实现语义、测试矩阵和 Gate 证据以对应 baseline 文档为准。

## M0：契约与技术预研 — DONE

- [x] ADR-0007～0011 冻结。
- [x] v2 config / error schema。
- [x] v1 + v2 Contracts Gate。
- [x] `russh` + `russh-sftp` 技术预研与真实 OpenSSH fixture。
- [x] 明确不需要 Remote Exec。

主要文档：

- [`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)
- [`CONFIG_SCHEMA_V2.md`](./CONFIG_SCHEMA_V2.md)
- [`SSH_SFTP_TECHNICAL_RESEARCH_V2.md`](./SSH_SFTP_TECHNICAL_RESEARCH_V2.md)

## M1：配置模型与 Source Backend 抽象 — DONE

- [x] v1/v2 配置版本路由。
- [x] LocalBackend / Snapshot 抽象。
- [x] Query Engine 不直接读取 backend type。
- [x] Local v1 `openat2()` 安全边界保持不变。
- [x] 全量回归 Gate 通过。

基线：[`M1_IMPLEMENTATION_BASELINE_V2.md`](./M1_IMPLEMENTATION_BASELINE_V2.md)

## M2：SSH Transport — DONE

- [x] `SecretResolver`。
- [x] Password auth。
- [x] Private key / encrypted key auth。
- [x] 强制 Host Key Verification。
- [x] connect / operation timeout。
- [x] 全局连接并发限制。
- [x] 只读 SFTP `stat/lstat/read_dir/read_range`。
- [x] 网络断开、错误凭据、host-key mismatch、权限、超时、大文件 range read 实机测试。
- [x] 不存在 exec / shell / write / upload。

基线：[`M2_IMPLEMENTATION_BASELINE_V2.md`](./M2_IMPLEMENTATION_BASELINE_V2.md)

## M3：CacheStore — DONE

- [x] 内部 ID 驱动的 cache layout。
- [x] 0700 directory / 0600 file。
- [x] versioned manifest/catalog。
- [x] staging + atomic commit。
- [x] restart recovery。
- [x] multi-generation。
- [x] global/per-source quota。
- [x] retention / max generations / pin-aware GC。
- [x] corruption / orphan staging / partial append recovery tests。

基线：[`M3_IMPLEMENTATION_BASELINE_V2.md`](./M3_IMPLEMENTATION_BASELINE_V2.md)

## M4：SyncEngine — DONE

- [x] `full` / `tail(bytes)` / `from_now` bootstrap。
- [x] incremental append。
- [x] 64 KiB SHA-256 continuity fingerprint。
- [x] truncate / replacement / continuity mismatch 新 generation。
- [x] sync byte budget。
- [x] 同步失败不破坏最后有效 cache。
- [x] 真实 SFTP → SyncEngine → CacheStore Gate。

基线：[`M4_IMPLEMENTATION_BASELINE_V2.md`](./M4_IMPLEMENTATION_BASELINE_V2.md)

## M5：Remote Source Query Integration — DONE

- [x] Remote explicit files。
- [x] Remote non-recursive directory discovery。
- [x] suffix filter / regular-file validation / stable ordering。
- [x] Remote on-query refresh。
- [x] Remote → Cache → existing Scanner / Query Engine。
- [x] Local + Remote mixed query。
- [x] 受控并发 refresh。
- [x] Cursor 固定 generation + snapshot length。
- [x] Cursor continuation 不重新 refresh Remote。
- [x] `match_ref` 持有 generation pin。
- [x] `get_log_context` Remote 正常路径 0 SSH。
- [x] rotation 后旧 `match_ref` TTL 内仍可读取旧 generation。
- [x] Tail / FromNow incomplete coverage 返回 `CACHE_SCOPE_EXCEEDED`。
- [x] 不完整 cache 不允许制造假阴性。
- [x] v2 Remote / Sync / Cache error contract 接入运行时。
- [x] 真实 OpenSSH/SFTP Query end-to-end Gate。

基线：[`M5_IMPLEMENTATION_BASELINE_V2.md`](./M5_IMPLEMENTATION_BASELINE_V2.md)

M5 最终正式 Gate：

```text
Rust          run 31186919582  PASS
Contracts     run 31186920077  PASS
SSH Transport run 31186920027  PASS
```

---

# M6：安全、性能与生产验收 — NEXT

M6 原则：**不重写 M1～M5 核心架构，只补生产证据、故障矩阵、性能基线和运维文档。**

## M6.1 Security Tests

必须证明 AI / MCP 客户端无法：

- [ ] 提交任意 host。
- [ ] 提交 SSH username/password。
- [ ] 提交 `secret_ref` 或覆盖管理员 connection。
- [ ] 提交远程绝对路径。
- [ ] 使用 `..`、`.`、空 component、反斜杠或控制字符逃逸 source root。
- [ ] 通过 symlink 访问未授权位置。
- [ ] 调用 Shell / Remote Exec。
- [ ] 上传、修改、删除服务器文件。
- [ ] 从错误消息获得 secret / key material / password。
- [ ] 从 `list_log_sources` 获得 connection host、username、secret_ref 或 cache path。
- [ ] 通过 cursor / `match_ref` 获取 remote absolute path / cache absolute path / generation UUID。

### M6.1 Exit

```text
AI-facing input surface contains no SSH credential/path control
remote transport remains read-only
negative security matrix PASS
public errors/results contain no sensitive infrastructure data
```

## M6.2 Cache Security

- [ ] Cache root / source / file / generation directory 权限测试。
- [ ] Cache data file 权限测试。
- [ ] Manifest / catalog 不含 Secret。
- [ ] Manifest 不保存不必要的 remote absolute path。
- [ ] Cache 文件名不泄露 remote path。
- [ ] 错误 / Debug / Display 不泄露 cache absolute path。
- [ ] recovery / corruption error redaction 测试。
- [ ] 外部非法删除 active generation 时稳定 fail-closed，不读取错误文件。

### M6.2 Exit

```text
cache permission boundary PASS
manifest/catalog secret scan PASS
active generation external-deletion fault PASS
```

## M6.3 Multi-Server Acceptance

M5 架构已支持多个 `connection_id`，M6 增加至少两个独立 OpenSSH fixture：

- [ ] Server A + Server B 同一查询。
- [ ] 两台服务器不同日志同时返回。
- [ ] 全局 SSH semaphore 对两台服务器共同生效。
- [ ] Server A 不可用时不会污染 Server B cache。
- [ ] 不同 connection 的 source/file identity 不冲突。
- [ ] Local + Server A + Server B 混合查询。
- [ ] Password 与 Private Key 可分别用于不同服务器。

### M6.3 Exit

```text
one local MCP
→ two independent SSH servers
→ stable mixed query semantics
```

## M6.4 Fault Injection

已有 M2～M5 测试先纳入统一 failure matrix，再只补缺口：

- [ ] SSH handshake 中断。
- [ ] auth failure。
- [ ] host key mismatch / missing known_hosts。
- [ ] auth 后立即断线。
- [ ] SFTP read 中途断线。
- [ ] operation timeout / cancellation。
- [ ] server restart。
- [ ] remote file rotation during sync。
- [ ] remote file truncate during sync。
- [ ] same-size replacement during sync。
- [ ] cache manifest corruption。
- [ ] cache generation data corruption / deletion。
- [ ] cache quota exhausted。
- [ ] staging write interrupted。
- [ ] MCP kill / restart with orphan staging fixture。
- [ ] Tail / FromNow incomplete-query behavior。

要求：

- [ ] 无 silent success。
- [ ] 无 false empty result。
- [ ] 最后有效 generation 不被失败 sync 破坏。
- [ ] staging/recovery 可重复执行。
- [ ] retryable 分类准确。
- [ ] 公共错误不泄露 secret/path/backtrace。

## M6.5 Performance Benchmarks

建立**可重复 benchmark harness**，不把 benchmark 机器绝对耗时写成产品 SLA。

至少覆盖数据规模：

```text
100MB
1GB
10GB
```

场景：

- [ ] cold full bootstrap。
- [ ] tail bootstrap。
- [ ] cache hit。
- [ ] unchanged continuity probe。
- [ ] 1MB append sync。
- [ ] 100MB append sync。
- [ ] 本地 cache scan。
- [ ] 单服务器并发查询。
- [ ] 双服务器并发查询。

记录：

```text
fixture size
bootstrap/sync bytes transferred
wall time
search time
CPU
peak RSS
cache disk usage
result count
runner / filesystem / Rust version
```

Benchmark 必须验证的工程性质：

- [ ] 第二次 unchanged query 不重复下载完整日志。
- [ ] append 只传输新增 payload + bounded fingerprint probes。
- [ ] cache hit 查询不走 SSH。
- [ ] 10GB 场景不会要求把完整日志加载进内存。
- [ ] 并发受配置上限约束，不产生无界 SSH session。

## M6.6 Production Docs

- [ ] 更新 `README.md`：Local + Remote 两种部署模式。
- [ ] 更新 `docs/INSTALL.md`：Remote prerequisites / secret_ref / known_hosts。
- [ ] 更新/新增 `docs/OPERATIONS.md`：cache、错误、恢复、容量、升级。
- [ ] 增加 Remote Source 完整配置示例。
- [ ] 增加创建 `log-reader` 只读账号建议。
- [ ] 增加 SFTP-only / chroot hardening 示例。
- [ ] 增加 known_hosts 初始化与 host-key rotation 说明。
- [ ] 增加 Cache 容量规划。
- [ ] 增加 Remote 错误排查表。
- [ ] 增加“不支持 Remote Exec/Deploy”的边界说明。

---

# M6 推荐执行顺序

```text
M6-A 现有安全/故障能力盘点
 ↓
M6-B 缺失的 deterministic negative/fault tests
 ↓
M6-C two-server live acceptance
 ↓
M6-D cache security / redaction gate
 ↓
M6-E benchmark harness + 100MB/1GB/10GB evidence
 ↓
M6-F production docs
 ↓
M6 Final Gate
```

优先复用 M2～M5 已经存在的 failure tests，不重复造第二套实现。

---

# 发布前最终验收

v2 不得发布，除非全部满足：

- [ ] v1 配置和测试完全兼容。
- [ ] 单 MCP 可查询至少两台独立 SSH Server。
- [ ] 远程服务器无需安装 MCP/Agent。
- [ ] Password auth 可用。
- [ ] Private key auth 可用。
- [ ] Host Key Verification 强制有效。
- [ ] 无 Remote Exec 能力。
- [ ] AI 无法提交 SSH connection/credential/arbitrary path。
- [ ] 正常路径为增量同步，不是重复全量下载。
- [ ] rotation/truncate/replacement 不会拼错日志。
- [ ] stale / incomplete cache 不会制造假阴性。
- [ ] `get_log_context` 可以从旧 generation 稳定读取。
- [ ] cursor 分页 Snapshot 稳定。
- [ ] Cache quota 和 GC 有测试。
- [ ] 同步崩溃可恢复。
- [ ] Cache/错误/结果不泄露 Secret 与基础设施敏感信息。
- [ ] 100MB / 1GB / 10GB benchmark evidence 完成。
- [ ] Production operations docs 完成。
- [ ] `fmt + clippy -D warnings + test + release build + contracts + SSH live` 全部通过。

---

# 当前推荐动作

```text
M0 Contract                  DONE
 ↓
M1 Backend abstraction       DONE
 ↓
M2 SSH Transport             DONE
 ↓
M3 CacheStore                DONE
 ↓
M4 SyncEngine                DONE
 ↓
M5 Query integration         DONE
 ↓
M6 Production hardening      NEXT
```

**下一步：执行 M6-A，先建立安全/故障覆盖矩阵，明确“已覆盖 / 缺口 / 需要新增测试”，然后只实现真正的缺口。**