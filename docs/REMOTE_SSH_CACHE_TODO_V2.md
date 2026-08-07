# Log Query MCP v2 Remote SSH/Cache 实施 TODO

> 状态：Ready for implementation planning  
> 日期：2026-08-07  
> 总方案：[`REMOTE_SSH_CACHE_DESIGN_V2.md`](./REMOTE_SSH_CACHE_DESIGN_V2.md)  
> 配置契约：[`CONFIG_SCHEMA_V2.md`](./CONFIG_SCHEMA_V2.md)

## 0. 实施原则

所有实现必须遵守以下冻结边界：

- 不新增 `ssh_exec` / Shell / 任意文件读取 MCP 工具。
- MCP 工具继续保持 `list_log_sources`、`search_logs`、`get_log_context`。
- Local Source 的 v1 `openat2()` 安全语义不回退。
- Remote Source 只通过 SSH/SFTP 获取管理员配置的日志。
- Remote Source 必须先形成稳定本地 Snapshot，再进入查询引擎。
- 默认不静默使用 stale cache。
- 缓存覆盖不完整时不得返回假阴性的空结果。
- v1 配置必须继续可用。

---

# M0：契约与技术预研

## M0.1 ADR

- [x] ADR-0007：Remote Source 通过本地 Cache 接入查询引擎。
- [x] ADR-0008：只使用 SSH/SFTP，不提供 Remote Exec。
- [x] ADR-0009：Cache Generation + Query Snapshot。
- [x] ADR-0010：On-query Sync + 显式 Bootstrap/Coverage。
- [ ] ADR 评审后将状态从 `Proposed for v2` 更新为 `Accepted for v2`。

## M0.2 Schema

- [x] 增加 `schemas/log-query-mcp-config-v2.schema.json`。
- [x] 增加 `schemas/tool-error-v2.schema.json`。
- [x] 增加 `docs/CONFIG_SCHEMA_V2.md`。
- [ ] 为 v2 Schema 增加合法/非法 fixture。
- [ ] 扩展 `scripts/validate_contracts.py` 验证 v1 + v2。
- [ ] Contracts CI 同时验证 v1/v2 Schema。

## M0.3 SSH 技术预研

在写业务代码前完成并记录：

- [ ] Rust SSH/SFTP 客户端库选型。
- [ ] 验证 Password Authentication。
- [ ] 验证 Private Key Authentication。
- [ ] 验证 encrypted private key + passphrase。
- [ ] 验证 `known_hosts` / Host Key Verification。
- [ ] 验证 SFTP `stat/lstat/readdir/open/seek/read`。
- [ ] 验证连接超时、读超时、断线取消行为。
- [ ] 验证 Tokio async 集成方式，确认是否存在 blocking API。
- [ ] 验证服务器断开后资源能可靠释放。
- [ ] 记录依赖 License、维护状态、平台兼容性和 unsafe 边界。

### M0 Gate

只有以下条件全部满足才能进入 M1：

```text
ADR 决策无冲突
v2 Schema 可机器验证
SSH 技术路径可行
不需要 Remote Exec
不破坏 unsafe_code = forbid 的项目原则
```

---

# M1：配置模型与 Source Backend 抽象

目标：**不接 SSH，先让现有 Local Source 通过新的抽象工作。**

## M1.1 配置版本路由

- [ ] `config.rs` 增加严格的 v1 / v2 配置解析入口。
- [ ] `version=1` 继续解析现有结构。
- [ ] `version=2` 解析 `connections/backend/cache/sync`。
- [ ] 未知 version 拒绝启动。
- [ ] 未知字段继续拒绝。
- [ ] 增加 connection/source 跨引用运行时校验。
- [ ] 增加 `max_bytes_per_source <= max_bytes` 等关系校验。

## M1.2 Backend 抽象

建议引入：

```text
src/backend/
├── mod.rs
└── local.rs
```

职责：

- [ ] 定义查询所需的 Source/Snapshot 抽象，而不是定义通用 FileSystem API。
- [ ] LocalBackend 封装现有 `safe_fs` / `source_discovery`。
- [ ] Query Engine 不直接读取 `backend.type`。
- [ ] Scanner 继续只处理稳定本地文件。

## M1.3 回归

- [ ] v1 全量单元测试通过。
- [ ] v1 contract fixture 结果不变。
- [ ] Local Source 性能没有显著回退。
- [ ] `cargo fmt --all -- --check`。
- [ ] `cargo clippy --locked --all-targets --all-features -- -D warnings`。
- [ ] `cargo test --locked --all-targets --all-features`。

### M1 Gate

```text
Local Source 已完全通过 Backend 抽象
Remote 代码仍为 0
所有 v1 测试通过
```

---

# M2：SSH Transport

目标：实现一个**只提供日志读取所需能力**的内部 SSH/SFTP Transport。

建议结构：

```text
src/transport/
├── mod.rs
└── ssh.rs
```

## M2.1 SecretResolver

- [ ] 定义 `SecretResolver` trait / 等价抽象。
- [ ] MVP 支持 `secret_ref -> environment variable`。
- [ ] Secret 不进入 Debug / Display 输出。
- [ ] Secret 不进入 MCP 错误。
- [ ] Secret 不写入 cache manifest。
- [ ] 增加 missing secret / invalid secret 测试。

## M2.2 ConnectionManager

- [ ] SSH connect。
- [ ] Password auth。
- [ ] Private key auth。
- [ ] Host Key Verification。
- [ ] `known_hosts` 不存在时拒绝连接。
- [ ] host key changed 时返回 `HOST_KEY_VERIFICATION_FAILED`。
- [ ] connect timeout。
- [ ] operation timeout。
- [ ] keepalive。
- [ ] idle connection cleanup。
- [ ] 全局最大连接数限制。

## M2.3 SFTP 最小能力

只实现：

```text
stat/lstat
readdir
open read-only
seek/read range
```

明确不实现：

```text
write
create
truncate
rename
remove
mkdir
exec
shell
scp upload
```

## M2.4 Transport 测试

建立测试 SSH/SFTP 环境，覆盖：

- [ ] 正确 Password。
- [ ] 错误 Password。
- [ ] 正确 Private Key。
- [ ] 错误 Private Key。
- [ ] Host Key 正确。
- [ ] Host Key 改变。
- [ ] 无权限文件。
- [ ] 不存在文件。
- [ ] 网络断开。
- [ ] 超时。
- [ ] 大文件 range read。

### M2 Gate

```text
SSH/SFTP 可稳定读取受控文件范围
不存在远程命令执行能力
Host Key 验证不能绕过
```

---

# M3：CacheStore

目标：建立可恢复、有界、支持多 generation 的本地缓存。

建议结构：

```text
src/cache/
├── mod.rs
├── store.rs
├── manifest.rs
├── generation.rs
└── gc.rs
```

## M3.1 Cache Layout

- [ ] Cache 路径只由内部 ID 生成。
- [ ] 不把远程绝对路径直接拼接为本地路径。
- [ ] directory 权限 `0700`。
- [ ] file 权限 `0600`。
- [ ] source/file/generation 使用稳定不透明 ID。

## M3.2 Manifest

至少保存：

```text
source_id
file_id
remote relative identifier
generation
remote size
cached range
remote mtime / available metadata
last_sync_at
continuity fingerprint
coverage
```

- [ ] Manifest schema/version。
- [ ] 原子写临时文件。
- [ ] fsync/rename 策略评估。
- [ ] 损坏 manifest 可检测。
- [ ] MCP 重启后可恢复有效缓存。

## M3.3 Generation

- [ ] append 继续当前 generation。
- [ ] truncate 创建新 generation。
- [ ] replacement 创建新 generation。
- [ ] continuity mismatch 创建新 generation。
- [ ] generation 不直接覆盖旧数据。

## M3.4 Quota / GC

- [ ] global cache quota。
- [ ] per-source quota。
- [ ] retention。
- [ ] max generations/file。
- [ ] 活动 Snapshot 不得删除。
- [ ] 活动 cursor 不得删除。
- [ ] 活动 `match_ref` 不得删除。
- [ ] 达到 quota 且无法安全 GC 时返回 `CACHE_LIMIT_EXCEEDED`。

### M3 Gate

```text
缓存可重启恢复
缓存写入原子
旧 generation 不会被活动 token 错误删除
```

---

# M4：SyncEngine

目标：把远程不断变化的日志安全转换为本地稳定 generation。

建议：

```text
src/cache/sync.rs
```

## M4.1 Bootstrap

- [ ] `full`。
- [ ] `tail(bytes)`。
- [ ] `from_now`。
- [ ] Manifest 记录缓存 coverage。
- [ ] Bootstrap 中断不能污染有效 generation。
- [ ] Bootstrap 超过 `max_sync_bytes_per_query` 时稳定失败。

## M4.2 Incremental Append

- [ ] 远程 metadata 比较。
- [ ] 无变化不下载。
- [ ] append 只下载新增 range。
- [ ] 下载到 staging file。
- [ ] continuity 验证成功后 commit。
- [ ] 更新 manifest。

## M4.3 Continuity Fingerprint

- [ ] 确定 fingerprint window 大小。
- [ ] 保存旧 offset 附近 fingerprint。
- [ ] 下一次 sync 读取远程对应窗口复核。
- [ ] mismatch 不允许 append 到旧 generation。
- [ ] fingerprint 不能依赖完整日志 hash。

## M4.4 Rotation / Replacement

覆盖：

```text
size 变小
同名文件替换
truncate 后快速增长
application.log -> application.log.1
新的 application.log 创建
文件删除
```

- [ ] 能保留旧 generation。
- [ ] 能建立新 generation。
- [ ] 不把两个不同物理日志错误拼接。

## M4.5 Failure Recovery

- [ ] SSH 中断。
- [ ] SFTP read 中断。
- [ ] 本地磁盘满。
- [ ] 进程在 staging 阶段崩溃。
- [ ] 进程在 manifest commit 前崩溃。
- [ ] 重启后清理 orphan staging files。

### M4 Gate

```text
正常查询路径只同步增量
rotation 不会错误拼接日志
任何同步失败都不破坏最后有效 cache
```

---

# M5：Remote Source 查询集成

目标：让现有 MCP 工具无感支持 Remote Source。

## M5.1 Source Discovery

- [ ] Remote explicit files。
- [ ] Remote directory discovery。
- [ ] suffix filter。
- [ ] recursive=false MVP 语义与 v1 一致。
- [ ] SFTP `lstat` 检查。
- [ ] 仅普通文件。
- [ ] 文件数量限制。
- [ ] 稳定 file_id / stable ordering。

## M5.2 `search_logs`

实现流程：

```text
validate
→ resolve source
→ ensure fresh
→ freeze cache snapshot
→ existing scanner
→ query engine
```

- [ ] Local + Remote 混合查询。
- [ ] 多个 Remote Server 查询。
- [ ] 受控并发 refresh。
- [ ] Snapshot 固定 generation + length。
- [ ] 新增日志不改变已有 cursor 结果。

## M5.3 Coverage

- [ ] full coverage 可正常查询历史。
- [ ] tail coverage 能判断查询超出范围。
- [ ] from_now coverage 能判断历史缺失。
- [ ] 不完整时返回 `CACHE_SCOPE_EXCEEDED`。
- [ ] 不允许以空结果代替 coverage error。

## M5.4 `match_ref`

- [ ] 绑定 source/file/generation/offset。
- [ ] 不包含远程路径。
- [ ] `get_log_context` 正常情况下 0 次 SSH 请求。
- [ ] 远程文件轮转后旧 `match_ref` 在 TTL 内仍能读取旧 generation。

## M5.5 cursor

- [ ] 绑定完整 Query Snapshot。
- [ ] 后续页不重新刷新远程源。
- [ ] 后续页不看到 Snapshot 之后新增的日志。
- [ ] generation 被非法删除时稳定返回错误，不读错误文件。

### M5 Gate

同一固定日志数据集：

```text
LocalBackend 查询结果
==
SSH Remote → Cache 查询结果
```

在搜索、排序、分页和上下文语义上保持一致。

---

# M6：安全、性能与生产验收

## M6.1 Security Tests

必须证明 AI 无法：

- [ ] 提交任意 host。
- [ ] 提交 SSH username/password。
- [ ] 提交远程绝对路径。
- [ ] 路径 `..` 逃逸。
- [ ] 通过 symlink 访问未授权位置。
- [ ] 调用 Shell。
- [ ] 上传/修改/删除服务器文件。
- [ ] 从错误消息获得 secret。
- [ ] 从 `list_log_sources` 获得连接敏感信息。

## M6.2 Cache Security

- [ ] Cache 目录权限测试。
- [ ] Manifest 不含 Secret。
- [ ] Cache 文件名不泄露不必要的远程绝对路径。
- [ ] Log redaction 测试。

## M6.3 Performance Benchmarks

至少测试：

```text
100MB
1GB
10GB
```

场景：

- [ ] cold full bootstrap。
- [ ] tail bootstrap。
- [ ] cache hit。
- [ ] 1MB append sync。
- [ ] 100MB append sync。
- [ ] 单服务器并发查询。
- [ ] 多服务器并发查询。

记录：

```text
首次查询耗时
增量同步耗时
下载 bytes
cache hit latency
search latency
CPU
memory
cache disk usage
```

## M6.4 Fault Injection

- [ ] SSH handshake 中断。
- [ ] auth 后立即断线。
- [ ] SFTP 中途断线。
- [ ] server restart。
- [ ] remote file rotation during sync。
- [ ] remote file truncate during sync。
- [ ] cache disk full。
- [ ] MCP kill -9 during sync。

## M6.5 Production Docs

- [ ] 更新 README。
- [ ] 更新 INSTALL。
- [ ] 更新 OPERATIONS。
- [ ] 增加 Remote Source 配置示例。
- [ ] 增加创建 `log-reader` 只读账号建议。
- [ ] 增加 SFTP-only / chroot hardening 示例。
- [ ] 增加 known_hosts 初始化说明。
- [ ] 增加 Cache 容量规划。
- [ ] 增加 Remote 错误排查表。

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
- [ ] 正常路径为增量同步，不是重复全量下载。
- [ ] rotation/truncate/replacement 不会拼错日志。
- [ ] stale cache 不会制造假阴性。
- [ ] `get_log_context` 可以从旧 generation 稳定读取。
- [ ] cursor 分页 Snapshot 稳定。
- [ ] Cache quota 和 GC 有测试。
- [ ] 同步崩溃可恢复。
- [ ] `fmt + clippy + test + contracts` 全部通过。

---

# 推荐实施顺序

严格按照：

```text
M0 Contract
 ↓
M1 Backend abstraction
 ↓
M2 SSH Transport
 ↓
M3 CacheStore
 ↓
M4 SyncEngine
 ↓
M5 Query integration
 ↓
M6 Production hardening
```

不要直接从：

```text
search_logs → SSH
```

开始开发。

最重要的架构控制点是先完成 M1：**让现有 Local Source 通过新的 Backend/Snapshot 抽象运行且所有 v1 测试保持通过，然后再接入 SSH。**
