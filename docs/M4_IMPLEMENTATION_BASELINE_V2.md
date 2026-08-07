# Log Query MCP v2 M4 实现基线

> 状态：M4 SyncEngine implementation complete  
> 日期：2026-08-07  
> 分支：`feat/v2-m1-backend-config`  
> M4 最终代码 Gate 基线：`71a4b1b7e559345394867fd43407c180107c24a0`  
> SyncEngine 接入提交：`1189aba3a746fbc0424b66be7667e58dd8a426ae`  
> Remote Path 边界加固提交：`b16eab52b34e49f67b3642911652e7f074708d5c`  
> 正式 Rust Gate：run `31178915781`  
> 正式 Contracts Gate：run `31178915591`  
> 上游清单：[`REMOTE_SSH_CACHE_TODO_V2.md`](./REMOTE_SSH_CACHE_TODO_V2.md)  
> M3 基线：[`M3_IMPLEMENTATION_BASELINE_V2.md`](./M3_IMPLEMENTATION_BASELINE_V2.md)

## 1. M4 目标

M4 把 M2 的 SSH/SFTP 只读 Transport 与 M3 的 CacheStore 连接起来，把远程不断变化的日志安全转换成稳定、可恢复的本地 Cache Generation。

核心目标：

- 首次同步支持 `full` / `tail(bytes)` / `from_now`；
- 正常增长只下载增量 range；
- 正常 append 保持同一个 generation；
- truncate / replacement / continuity mismatch 创建新 generation；
- rotation/replacement 不允许把两个不同物理日志错误拼接；
- 任意同步失败不破坏最后有效 cache；
- 同步字节数受 `max_sync_bytes_per_query` 约束；
- Remote path 只能由管理员配置推导，不能从调用方注入绝对路径。

M4 不负责：

- Remote directory discovery；
- Remote Source 注册进 Query Engine；
- coverage 到 `CACHE_SCOPE_EXCEEDED` 的查询响应映射；
- cursor / `match_ref` 与 generation pin 的端到端生命周期；
- Local + Remote 混合查询。

这些职责属于 M5。

---

## 2. 架构

```text
LogSourceConfigV2
  root + controlled relative identifier
                │
                ▼
        RemoteSyncTarget
                │
                ▼
           SyncEngine
                │
        ┌───────┴────────┐
        │                │
        ▼                ▼
SshConnectionManager   CacheStore
        │            begin_generation
        ▼            begin_append
 SshReadTransport          │
 lstat + read_range        ▼
        │           stable generation
        └─────────┬────────┘
                  ▼
        continuity decision
```

同步核心只需要远端：

```text
lstat
read_range
```

不会引入：

```text
exec
shell
write
create
truncate
rename
remove
upload
```

生产实现使用 `SshReadTransport`；同步算法内部通过最小 `RemoteSyncReader` 接口解耦，因此 fault/rotation/continuity 场景可以用 deterministic fake reader 测试，而不扩大生产权限面。

---

## 3. RemoteSyncTarget 安全边界

M4 最终 API：

```text
RemoteSyncTarget::from_source(source, remote_identifier)
```

调用方不能额外传入 `remote_path`。

真实 SFTP path 只能由：

```text
source.root + remote_identifier
```

内部推导。

### 3.1 root

要求：

- Linux 绝对路径；
- 非空；
- 长度受限；
- 不含 control character。

### 3.2 remote identifier

要求：

- 必须是相对标识；
- 不允许绝对路径；
- 不允许 `.`；
- 不允许 `..`；
- 不允许空 path component；
- 不允许反斜杠；
- 不允许 control character；
- 长度受限。

因此 M5 以后只能把管理员配置 / discovery 得出的相对文件标识交给 M4，不能把 MCP 客户端提交的任意路径直接进入 SFTP Transport。

`Debug` 输出不会打印真实 remote path，只显示 `<configured-remote-path>`。

---

## 4. Bootstrap

### 4.1 `full`

```text
cached_range = [0, remote_size)
coverage     = Full
```

首次下载整个远程文件。

### 4.2 `tail(bytes)`

```text
start        = max(remote_size - bytes, 0)
cached_range = [start, remote_size)
coverage     = Tail(start)
```

只保存要求的尾部范围。

### 4.3 `from_now`

```text
cached_range = [remote_size, remote_size)
coverage     = FromNow(remote_size)
```

首次不缓存历史正文，只记录开始位置。

为了后续判断“同一物理文件是否继续增长”，`from_now` 首次同步允许读取当前位置之前最多 64 KiB 作为 continuity verification anchor；这些字节只用于 fingerprint，不进入 cache coverage，也不会被 Query Engine 当成可查询历史。

---

## 5. Continuity Fingerprint

固定窗口：

```text
64 KiB
```

算法：

```text
SHA-256
```

持久化格式：

```text
sha256-v1:<start>:<end>:<hex>
```

fingerprint 只覆盖旧同步边界附近的小窗口，不计算完整日志 hash。

目标不是证明整个远程文件永远未变化，而是回答同步时最关键的问题：

> 当前远程文件在上一次同步边界前的尾部，是否仍然与已经缓存的 generation 连续？

如果不能证明连续，则 fail-safe：不 append 到旧 generation。

---

## 6. 同步状态机

观察远程文件时先使用 `lstat`，并要求：

```text
file_type == Regular
size      != None
```

symlink / directory / other / unknown 都拒绝同步。

### 6.1 首次出现

```text
无 current generation
→ bootstrap
→ NewGeneration(InitialBootstrap)
```

### 6.2 Cache 状态异常

如果：

```text
current.cached_range.end != current.remote_size
```

说明当前 cache 元数据不能作为安全 append 基础：

```text
→ NewGeneration(CacheStateMismatch)
```

### 6.3 Remote size 变小

```text
remote_size < old_remote_size
→ NewGeneration(RemoteTruncated)
```

典型场景：truncate、rotation 后新的同名文件更小。

### 6.4 size 不变

如果：

```text
size 相同 && mtime 相同
```

则：

```text
Unchanged
```

不执行 `read_range`。

如果：

```text
size 相同 && mtime 改变
```

MVP 采用保守策略：

```text
→ NewGeneration(MetadataChangedWithoutGrowth)
```

即使实际内容可能恰好相同，也不冒险把它继续视为旧物理日志。

### 6.5 size 增大

先读取上次保存的 fingerprint window：

```text
remote[old_fingerprint.start .. old_fingerprint.end]
```

并与 manifest 中 fingerprint 比较。

如果 fingerprint 缺失/格式无效：

```text
→ NewGeneration(ContinuityUnavailable)
```

如果 fingerprint mismatch：

```text
→ NewGeneration(ContinuityMismatch)
```

如果 fingerprint match：

```text
→ 只下载 [old_remote_size, new_remote_size)
→ Appended
→ generation id 不变
```

这能覆盖“truncate 后迅速重新增长到比旧 size 更大”的危险场景：单独比较 size 会误判为 append，continuity fingerprint 会阻止错误拼接。

---

## 7. Incremental Append 提交顺序

正常 append：

```text
1. lstat observed metadata
2. 验证旧 continuity fingerprint
3. pin current generation snapshot
4. begin_append() 创建 staging
5. 仅下载 [old_remote_size, observed_remote_size)
6. 使用 old cached tail + 新增 bytes 构造新 fingerprint
7. 从远程重新读取新 fingerprint window 复核
8. final lstat
9. StagedAppend.commit()
10. 原子更新 manifest
```

如果第 1～8 步任意失败：

```text
StagedAppend drop
→ staging 被清理
→ current manifest 不变
→ 最后有效 generation 不变
```

如果进程在实际 append 数据后、manifest commit 前崩溃，则由 M3 recovery 按 manifest 中已提交 `data_len` 截掉额外尾部。

---

## 8. Stable Snapshot 语义

M4 的“同步到最新”定义为：

> 建立一个在本次同步观察边界上的稳定 Snapshot。

如果远程文件在同步过程中继续增长，只要已经观察的边界仍然连续且未被 truncate，M4 可以提交到本次开始时观察到的 `remote_size`；更晚的新字节在下一次 `on_query` sync 再增量下载。

如果同步过程中远程文件缩小，或者 fingerprint window 不再一致，则本次同步失败，不提交不稳定结果。

---

## 9. Sync Byte Budget

`max_sync_bytes_per_query` 约束的是：

```text
所有 remote bytes read
```

不仅包括实际写入 cache 的日志数据，也包括 continuity verification probe。

例如 append：

```text
旧 fingerprint probe
+ 新增 range
+ 新 fingerprint verification
<= max_sync_bytes_per_query
```

超过额度时返回：

```text
SyncError::SyncLimitExceeded
```

不会发布半完成 generation。

---

## 10. Cache Capacity

在开始写入候选 generation 前，M4 会先检查当前候选 cached range：

```text
candidate_len <= cache.max_bytes_per_source
candidate_len <= cache.max_bytes
```

超过时返回：

```text
SyncError::CacheCapacityExceeded
```

M3 已提供完整 GC / retention / generation protection 机制。

M4 不在同步路径中自行实现 cursor / `match_ref` 生命周期 GC；这些 token 的 generation pin 生命周期在 M5 Query Snapshot 集成时完成。

---

## 11. Failure Semantics

### SSH / SFTP failure

包括：

```text
connect failure
auth failure
host key failure
operation timeout
SFTP read failure
broken transport
```

直接向上传递稳定错误，不使用 stale cache，因为 v2 MVP：

```text
allow_stale_on_error = false
```

已有有效 cache 保持不变。

### Remote file deletion

当前 M2 Transport 不对所有 SFTP server error 细分 `ENOENT`；文件删除会表现为 transport/SFTP sync error。

语义仍然是：

```text
本次 refresh 失败
最后有效 cache 不被替换
不静默返回 stale data
```

### Local staging / disk failure

在 manifest commit 前失败不会发布新 generation。

### Crash recovery

继承 M3：

- orphan staging 会在 recovery 清理；
- 未被 manifest 引用的 generation 文件会被回收；
- append data 比 manifest 长时会截回已提交长度；
- data 比 manifest 声明短时 fail-closed，不静默修复。

---

## 12. Rotation / Replacement 判定表

| 场景 | M4 行为 |
| --- | --- |
| 无变化 | `Unchanged`，0 range download |
| 正常增长 + fingerprint match | append，同 generation |
| size 变小 | 新 generation |
| 同 size + mtime 变化 | 保守创建新 generation |
| size 增大 + old fingerprint mismatch | 新 generation |
| fingerprint 不可用 | 新 generation |
| 同步过程中 fingerprint 改变 | 本次 sync 失败 |
| 同步过程中 file shrink | 本次 sync 失败 |
| `application.log -> application.log.1` | 两个相对 identifier 分别拥有稳定 file/cache identity，M5 discovery 负责发现 |
| 新 `application.log` 创建 | 同名 replacement 由 metadata + fingerprint/size 判定，不与旧 generation 错误拼接 |

---

## 13. 测试覆盖

M4 deterministic tests 已覆盖：

- `full` bootstrap；
- `tail(bytes)` bootstrap；
- `from_now` bootstrap 后正常 append；
- metadata 无变化时 0 次 range download；
- 正常 append 仅持久化新增 range；
- truncate 创建新 generation 且保留旧 generation；
- 同 size + mtime 变化视为 replacement；
- truncate 后快速增长由 fingerprint mismatch 检出；
- append 中途 read failure 不改变 current generation；
- bootstrap 超出 sync byte limit 不发布 manifest；
- RemoteSyncTarget 只能从配置 root 推导路径；
- `../secret.log` 被拒绝；
- `/etc/passwd` 被拒绝。

正式项目 Gate：

```text
cargo fmt --all -- --check                          PASS
cargo clippy --locked --all-targets --all-features PASS
  -- -D warnings
cargo test --locked --all-targets --all-features   PASS
cargo build --release --locked --bins              PASS
Contracts v1 + v2                                  PASS
```

对应：

```text
Rust      run 31178915781  PASS
Contracts run 31178915591  PASS
```

---

## 14. M4 Gate

```text
正常查询路径只同步增量                       PASS
无变化时不下载正文 range                    PASS
rotation/replacement 不会错误拼接日志       PASS
truncate + rapid growth 可检测               PASS
full/tail/from_now bootstrap 可表达          PASS
同步失败不破坏最后有效 cache                PASS
sync byte budget 可稳定失败                  PASS
remote path 不能由调用方任意注入             PASS
不存在 remote exec/write                     PASS
```

**M4 Gate 通过，可以进入 M5。**

---

## 15. M5 入口

M5 的正确接入流程应保持：

```text
resolve configured Remote Source
        ↓
discover / resolve controlled relative file
        ↓
RemoteSyncTarget::from_source(...)
        ↓
SyncEngine::sync()
        ↓
freeze CacheStore generation + length
        ↓
existing scanner / QueryEngine
        ↓
bind cursor + match_ref to Query Snapshot
```

M5 必须继续保持：

- Query Engine 不直接访问 SSH；
- Scanner 不直接访问 SSH；
- 后续 cursor page 不重新 refresh；
- `get_log_context` 正常情况下不发 SSH 请求；
- incomplete coverage 必须返回明确 scope error，不能伪装为空结果；
- 活动 cursor / `match_ref` pin 的 generation 不得被 GC。
