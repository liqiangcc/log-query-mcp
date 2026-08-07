# Log Query MCP v2 M4 实现基线

> 状态：M4 SyncEngine implementation complete  
> 日期：2026-08-07  
> 分支：`feat/v2-m1-backend-config`  
> M4 最终代码 Gate 基线：`65f0d47f0f09c31952059bb3e01cdda908133076`  
> SyncEngine 接入：`1189aba3a746fbc0424b66be7667e58dd8a426ae`  
> Remote Path 边界加固：`b16eab52b34e49f67b3642911652e7f074708d5c`  
> Continuity 加固：`e1ed09bd080998d075416900c42d152ae2a77bda`  
> Real SFTP M4 集成测试：`2958d68afaa3e80d9fcd491cb1b50b7e1242b9ba`  
> Real SFTP Gate 接线：`f7147ddfc5977b75103fb5ac438c1315848c0e58`  
> 正式 Rust Gate：run `31180209710`  
> 正式 Contracts Gate：run `31180209975`  
> 正式 SSH/SFTP Live Gate：run `31180209929`  
> 上游清单：[`REMOTE_SSH_CACHE_TODO_V2.md`](./REMOTE_SSH_CACHE_TODO_V2.md)  
> M3 基线：[`M3_IMPLEMENTATION_BASELINE_V2.md`](./M3_IMPLEMENTATION_BASELINE_V2.md)

## 1. M4 目标

M4 把 M2 的只读 SSH/SFTP Transport 与 M3 CacheStore 连接起来，把不断变化的远程日志转换成稳定、可恢复、有 generation 边界的本地缓存。

M4 已完成：

- `full` / `tail(bytes)` / `from_now` bootstrap；
- 正常增长只持久化新增 range；
- 正常 append 保持同一个 generation；
- truncate / replacement / continuity mismatch 创建新 generation；
- 64 KiB SHA-256 continuity fingerprint；
- 同步失败保持最后有效 cache；
- `max_sync_bytes_per_query` 对所有远程读取生效；
- Remote path 只能从管理员配置推导；
- fake-reader fault tests + 真实 OpenSSH/SFTP integration test。

M4 不负责 Remote discovery、Query Engine 集成、coverage 查询错误映射以及 cursor / `match_ref` 的 generation 生命周期。这些职责属于 M5。

---

## 2. 架构边界

```text
LogSourceConfigV2
  root + controlled relative identifier
                │
                ▼
        RemoteSyncTarget
                │
                ▼
           SyncEngine
          /          \
         /            \
SshConnectionManager  CacheStore
        │          begin_generation
        │          begin_append
        ▼               │
 SshReadTransport       ▼
 lstat + read_range  stable generation
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

生产实现使用 `SshReadTransport`；同步算法通过私有最小 `RemoteSyncReader` 接口解耦，因此 rotation、continuity、断线和额度耗尽可以 deterministic 测试，而不扩大生产权限面。

---

## 3. Remote Path 安全边界

M4 API：

```text
RemoteSyncTarget::from_source(source, remote_identifier)
```

调用方不能传任意 `remote_path`。真实 SFTP path 只能由：

```text
source.root + remote_identifier
```

内部推导。

`remote_identifier` 必须是受控相对标识，拒绝：

- 绝对路径；
- `.` / `..`；
- 空 path component；
- 反斜杠；
- control character；
- 超长标识。

因此后续 M5 不能把 MCP 客户端提交的任意路径直接送入 SSH/SFTP 层。

`Debug` 不打印真实 remote path，只显示 `<configured-remote-path>`。

---

## 4. Bootstrap

### `full`

```text
cached_range = [0, remote_size)
coverage     = Full
```

### `tail(bytes)`

```text
start        = max(remote_size - bytes, 0)
cached_range = [start, remote_size)
coverage     = Tail(start)
```

### `from_now`

```text
cached_range = [remote_size, remote_size)
coverage     = FromNow(remote_size)
```

`from_now` 首次不缓存历史正文，但允许读取同步边界之前最多 64 KiB 建立 continuity anchor。这些字节只用于 fingerprint，不属于可查询 coverage。

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

fingerprint 只覆盖上一次同步边界附近的小窗口，不计算整个日志文件 hash。

关键原则：

> `size` 和 `mtime` 只能作为辅助 metadata，不能单独证明当前远程文件仍是同一物理日志。

只要已有 current generation 且远程 size 没有变小，M4 都会先验证保存的 continuity fingerprint，再决定 `Unchanged` 还是 `Appended`。

这意味着即使：

```text
size 相同
mtime 相同
```

但内容已经被同名 replacement 替换，只要旧同步边界 fingerprint 不一致，就会创建新 generation，而不会错误返回 `Unchanged`。

---

## 6. 同步状态机

远程文件先通过 `lstat` 检查：

```text
file_type == Regular
size      != None
```

symlink / directory / other / unknown 均 fail-closed。

### 首次出现

```text
无 current generation
→ bootstrap
→ NewGeneration(InitialBootstrap)
```

### Cache 状态不可信

```text
current.cached_range.end != current.remote_size
→ NewGeneration(CacheStateMismatch)
```

### Remote size 变小

```text
remote_size < old_remote_size
→ NewGeneration(RemoteTruncated)
```

### Remote size 不小于旧 size

先验证旧 fingerprint：

```text
remote[old_fingerprint.start .. old_fingerprint.end]
```

若 fingerprint 缺失/格式无效：

```text
→ NewGeneration(ContinuityUnavailable)
```

若 fingerprint mismatch：

```text
→ NewGeneration(ContinuityMismatch)
```

若 fingerprint match 且 size 相同：

```text
→ Unchanged
→ cached_bytes_written = 0
```

注意：`Unchanged` 并不代表 0 次远程读取；它会执行小范围 continuity probe，但不会下载/写入正文 payload。

若 fingerprint match 且 size 增大：

```text
→ 仅下载 [old_remote_size, new_remote_size)
→ Appended
→ generation id 不变
```

`mtime` 会继续记录在 manifest 中，但不再作为 continuity authority。mtime 改变而内容 fingerprint 不变时，不会无谓轮换 generation。

---

## 7. Incremental Append 提交顺序

```text
1. lstat observed metadata
2. 验证旧 continuity fingerprint
3. pin current generation snapshot
4. begin_append() 创建 staging
5. 仅下载 [old_remote_size, observed_remote_size)
6. old cached tail + 新增 bytes 形成新 fingerprint
7. 远程重新读取新 fingerprint window 复核
8. final lstat
9. StagedAppend.commit()
10. 原子更新 manifest
```

第 1～8 步任意失败：

```text
staging 被丢弃
current manifest 不变
最后有效 generation 不变
```

若进程在数据 append 后、manifest commit 前崩溃，M3 recovery 会按 manifest 的已提交 `data_len` 截掉未提交尾部。

---

## 8. Stable Snapshot

M4 的“同步到最新”定义为：

> 建立一个在本次同步观察边界上的稳定 Snapshot。

远程文件在同步过程中继续增长时，只要已经观察的边界仍连续且没有 shrink，本次可提交到开始时观察的 `remote_size`；更晚的新字节由下一次 `on_query` refresh 增量同步。

如果同步过程中文件 shrink 或 fingerprint verification 不一致，本次 sync 失败，不发布不稳定结果。

---

## 9. Sync Byte Budget

`max_sync_bytes_per_query` 统计所有 remote bytes read，包括：

```text
continuity probe
+ 新增 payload range
+ 新 fingerprint verification
```

因此即使 `Unchanged`，continuity probe 也消耗预算。

超过额度：

```text
SyncError::SyncLimitExceeded
```

不会发布半完成 generation。

---

## 10. Rotation / Replacement 判定

| 场景 | M4 行为 |
| --- | --- |
| size 相同 + fingerprint match | `Unchanged`，不写 cache payload |
| size 相同 + fingerprint mismatch | 新 generation |
| mtime 变化 + fingerprint match | 保持当前 generation |
| 正常增长 + fingerprint match | append，同 generation |
| size 变小 | 新 generation |
| size 增大 + old fingerprint mismatch | 新 generation |
| fingerprint 不可用 | 新 generation |
| truncate 后快速重新增长 | fingerprint mismatch 检出，新 generation |
| 同步过程中 fingerprint 改变 | 本次 sync 失败 |
| 同步过程中 file shrink | 本次 sync 失败 |
| `application.log -> application.log.1` | 不同相对 identifier 拥有独立 cache identity；M5 discovery 负责发现 |
| 新 `application.log` 创建 | 由 size + continuity fingerprint 判定，不与旧物理日志错误拼接 |

---

## 11. Failure Semantics

SSH/SFTP connect/auth/host-key/timeout/read failure 直接返回稳定错误；v2 MVP 中：

```text
allow_stale_on_error = false
```

所以失败不会静默返回 stale cache，但已有最后有效 cache 也不会被破坏。

Remote file 删除目前会表现为 SFTP/transport sync error；语义仍然是 refresh 失败、最后有效 generation 不变。

本地 staging / disk failure 在 manifest commit 前不会发布候选 generation。

Crash recovery 继承 M3：

- 清理 orphan staging；
- 回收未被 manifest 引用的 generation；
- data 比 manifest 长时截回已提交长度；
- data 比 manifest 短时 fail-closed。

---

## 12. 测试证据

### Deterministic M4 tests

覆盖：

- full bootstrap；
- tail bootstrap；
- from_now bootstrap + append；
- unchanged 必须验证 continuity；
- 正常 append 只写新增 payload；
- truncate 新 generation；
- same-size same-mtime replacement 仍能检测；
- mtime 改变但内容未变不会误 rotation；
- truncate 后快速增长由 fingerprint mismatch 检出；
- append read failure 保留旧 generation；
- sync byte limit failure 不发布 manifest；
- remote path 从 configured root 推导；
- `../secret.log` / `/etc/passwd` 被拒绝。

Continuity hardening Gate：

```text
M4 Continuity Hardening Once
run 31179774011
12 passed, 0 failed
```

### Real OpenSSH/SFTP Gate

永久测试：

```text
tests/m4_sync_live.rs
```

真实链路：

```text
OpenSSH sshd
→ password auth + known_hosts
→ SFTP
→ SshReadTransport
→ SyncEngine
→ CacheStore
```

验证：

```text
initial full bootstrap
→ remote append
→ incremental sync
→ same generation
→ unchanged refresh
→ cached content == first\nsecond\n
```

正式 SSH/SFTP Gate：

```text
SSH Transport run 31180209929  PASS
```

同一 Gate 还重新执行 M2 production SSH/SFTP live tests，并验证 research POC 仍兼容。

---

## 13. 最终正式 Gate

```text
cargo fmt --all -- --check                          PASS
cargo clippy --locked --all-targets --all-features PASS
  -- -D warnings
cargo test --locked --all-targets --all-features   PASS
cargo build --release --locked --bins              PASS
Contracts v1 + v2                                  PASS
Real OpenSSH/SFTP production transport             PASS
Real SFTP -> SyncEngine -> CacheStore              PASS
```

对应：

```text
Rust          run 31180209710  PASS
Contracts     run 31180209975  PASS
SSH Transport run 31180209929  PASS
```

---

## 14. M4 Gate

```text
正常增长只持久化增量 payload                    PASS
Unchanged 仍验证 continuity                     PASS
mtime 不作为 continuity authority               PASS
same-size/same-mtime replacement 可检测          PASS
rotation/replacement 不错误拼接日志              PASS
truncate + rapid growth 可检测                   PASS
full/tail/from_now bootstrap 可表达              PASS
同步失败不破坏最后有效 cache                    PASS
sync byte budget 可稳定失败                     PASS
remote path 不能由调用方任意注入                PASS
不存在 remote exec/write                        PASS
真实 OpenSSH/SFTP -> SyncEngine -> CacheStore    PASS
```

**M4 Gate 通过，可以进入 M5。**

---

## 15. M5 入口

M5 应保持：

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

M5 必须继续满足：

- Query Engine 不直接访问 SSH；
- Scanner 不直接访问 SSH；
- 后续 cursor page 不重新 refresh；
- `get_log_context` 正常情况下不发 SSH 请求；
- incomplete coverage 必须返回明确 scope error，不能伪装为空结果；
- 活动 cursor / `match_ref` pin 的 generation 不得被 GC。
