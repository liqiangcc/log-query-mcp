# Log Query MCP v2 M5 实现基线

> 状态：M5 Remote Source query integration complete  
> 日期：2026-08-07  
> 分支：`feat/v2-m1-backend-config`  
> M5 代码 Gate 基线：`53b15786510ebca63f30343f8f34b0c43fbb4edc`  
> Foundation：`6ba341b4b466a3ec020c701c6d6d5641d4219e15`  
> match_ref generation pin：`bfaa008212a49946530004ea276321d73bf6f00d`  
> coverage / v2 error contract：`0149e88db0568f9511153b3dc5f7dc5e1187b3d1`  
> 上游：[`M4_IMPLEMENTATION_BASELINE_V2.md`](./M4_IMPLEMENTATION_BASELINE_V2.md)  
> 总清单：[`REMOTE_SSH_CACHE_TODO_V2.md`](./REMOTE_SSH_CACHE_TODO_V2.md)

## 1. M5 目标

M5 将 M2 的 SSH/SFTP Transport、M3 的 CacheStore、M4 的 SyncEngine 接入现有查询链，使现有 MCP 查询语义能够无感处理 Local Source 和 Remote SSH Source。

最终查询路径：

```text
MCP / Stateful Query
        │
        ▼
SourceRegistry
        │
        ├── LocalBackend
        │      └── SafeFile / openat2
        │
        └── RemoteBackend
               │
               ├── SFTP discovery
               ├── SyncEngine::sync
               └── CacheStore generation
                         │
                         ▼
                    SnapshotFile
                         │
                         ▼
                 existing scanner/query
```

M5 没有增加新的 AI-facing MCP tool。

仍然只有：

```text
list_log_sources
search_logs
get_log_context
```

SSH、同步、缓存、generation 都是服务内部实现细节。

---

## 2. Source Backend 集成

### 2.1 SnapshotFile

查询层不再要求底层一定是 `SafeFile`，而统一读取：

```text
SnapshotFile
├── Local(SafeFile)
└── Remote(PinnedGeneration)
```

`SnapshotFile` 实现：

```text
Read + Seek
```

Scanner、Query Engine、Context Reader 因此不需要知道数据来自：

```text
Linux 本地文件
或
SSH/SFTP → Local Cache
```

### 2.2 Local 安全模型保持不变

LocalBackend 继续使用原有：

```text
SafeRoot
openat2()
RESOLVE_NO_XDEV
regular-file validation
```

M5 没有为了支持 Remote Source 降低 v1 Local 文件安全边界。

---

## 3. Remote Source Registry

`version=2 + backend=ssh` 现在可以真正建立运行时 RemoteBackend。

Registry 构造阶段：

```text
解析配置
验证 connection/source/cache/limits
创建 CacheStore
创建共享 SshConnectionManager
创建 SyncEngine
注册 RemoteBackend
```

但不会在启动阶段主动连接远程服务器。

因此：

```text
MCP startup
!=
remote SSH connect
```

远端连接只在实际查询 Remote Source 时按需发生。

---

## 4. Remote Source Discovery

M5 支持：

### 4.1 Explicit Files

管理员配置：

```json
{
  "files": ["application.log"]
}
```

RemoteBackend 通过 SFTP `lstat` 验证目标是普通文件。

### 4.2 Directory Discovery

管理员配置：

```json
{
  "directories": [
    {
      "path": "archive",
      "recursive": false,
      "include_suffixes": [".log"]
    }
  ]
}
```

执行：

```text
SFTP read_dir
→ 安全文件名校验
→ suffix filter
→ lstat
→ regular file only
→ stable ordering
```

### 4.3 MVP recursive 语义

Remote v2 MVP 明确只支持：

```text
recursive = false
```

配置 `recursive=true` 时不会静默改变语义，而是明确拒绝。

### 4.4 数量限制

Remote discovery 同时受：

```text
max_scan_files_per_query
max_remote_files_per_source
```

约束。

不会因为一个远程目录包含大量文件而无界创建 SFTP/stat/sync 工作。

---

## 5. 查询前刷新

Remote Source 第一页查询流程：

```text
validate request
      ↓
resolve source
      ↓
SFTP discover
      ↓
SyncEngine::sync
      ↓
freeze Cache generation snapshot
      ↓
existing scanner
      ↓
existing query engine
```

当前 freshness 策略为：

```text
on_query
```

即每次新的查询在建立 Query Snapshot 前检查远端状态。

Cursor continuation 不执行远端刷新。

---

## 6. 共享 SSH 并发边界

Remote discovery 和 SyncEngine 共享同一个：

```text
SshConnectionManager
```

因此二者共同受：

```text
max_concurrent_ssh_connections
```

限制。

多文件 refresh 使用受控并发，而不是无界并发连接服务器。

当前仍采用 M2 冻结的：

```text
独立 SSH Session
+
共享 Semaphore
```

没有引入连接池。

---

## 7. Query Snapshot 与 Cursor

### 7.1 SourceFileSnapshot

Remote Snapshot 现在包含：

```text
source_id
file_id
relative identifier
cache generation identity
size_at_snapshot
coverage
generation pin
```

其中 generation UUID 被编码进现有内部 `FileIdentity`，因此不需要修改外部 MCP result/schema。

### 7.2 Cursor 固定 generation + length

第一页建立：

```text
Query Snapshot
```

Cursor 持有该 Snapshot。

之后即使远程服务器：

```text
append 新日志
rotate
replace
```

已有 Cursor：

```text
不会重新访问 SSH
不会重新 refresh
不会看到 Snapshot 之后的新日志
```

只有一个新的 `search_logs` 查询才会建立新的 Remote Snapshot。

### 7.3 Cursor 请求绑定保持原语义

Continuation 必须与首请求保持相同 QueryBinding，包括分页相关参数。

M5 没有放宽现有 cursor anti-mismatch 规则。

---

## 8. Generation Pin

### 8.1 GenerationPin

CacheStore 增加 cloneable generation lease：

```text
GenerationPin
```

内部使用共享生命周期。

最后一个 Pin Drop 后，generation 才重新具备 GC 资格。

### 8.2 Cursor Pin

Remote `SourceFileSnapshot` 持有 generation pin。

所以只要 Cursor 仍有效：

```text
generation 不允许被 GC
```

### 8.3 match_ref Pin

`MatchReferenceStore` 现在可以保存：

```text
MatchReferenceData
+
GenerationPin
```

外部 `match_ref` 仍然是原来的 opaque token。

没有向客户端暴露：

```text
cache path
generation UUID
remote absolute path
SSH connection
```

match_ref 过期或被容量淘汰时，StoredMatch 被删除，Pin 随之释放。

---

## 9. get_log_context：0 SSH

Remote match_ref 通过内部 file identity 找到精确的 Cache generation。

正常调用：

```text
get_log_context(match_ref)
        │
        ▼
MatchReferenceStore
        │
        ▼
Pinned Cache Generation
        │
        ▼
ContextReader
```

不需要：

```text
SSH connect
SFTP stat
remote refresh
```

真实 SFTP Gate 已验证：

1. 在旧 generation 上生成 match_ref；
2. 远程文件被替换并建立新 generation；
3. 临时移走 `known_hosts`，使任何新的 SSH 建连都无法通过 Host Key 验证；
4. `get_log_context(old match_ref)` 仍然成功读取旧 generation。

因此 M5 已经实际证明 context 路径为本地 Cache-only。

---

## 10. Coverage 正确性

### 10.1 Full

以下视为完整 coverage：

```text
Local Source
Remote Full
Tail(start_offset = 0)
FromNow(start_offset = 0)
```

可以正常执行查询。

### 10.2 Partial Tail / FromNow

如果：

```text
cached_range.start > 0
```

当前 M5 不能证明本地缓存覆盖完整历史查询范围。

因此保守返回：

```text
CACHE_SCOPE_EXCEEDED
```

而不是：

```text
results = []
```

这避免 AI 把“本地缓存里没有”错误解释为“服务器日志里没有”。

### 10.3 后续可优化但不能降低正确性

未来可以在有可靠时间索引/时间边界证明时允许：

```text
历史缓存不完整
但请求时间范围完全位于缓存 coverage 内
```

当前 M5 不做这种推断。

---

## 11. v2 Runtime Error Contract

M5 将此前已冻结在 `tool-error-v2.schema.json` 的 Remote/Cache 错误码真正接入运行时。

包括：

```text
REMOTE_UNAVAILABLE
REMOTE_AUTH_FAILED
HOST_KEY_VERIFICATION_FAILED
REMOTE_FILE_CHANGED
SYNC_FAILED
CACHE_SCOPE_EXCEEDED
CACHE_LIMIT_EXCEEDED
CACHE_CORRUPTED
```

典型映射：

```text
SSH password/key authentication failed
→ REMOTE_AUTH_FAILED

known_hosts / host key mismatch
→ HOST_KEY_VERIFICATION_FAILED

connect / operation timeout / broken SSH/SFTP
→ REMOTE_UNAVAILABLE

remote file changes during sync
→ REMOTE_FILE_CHANGED

remote target invalid for synchronization
→ SYNC_FAILED

partial cache cannot prove query coverage
→ CACHE_SCOPE_EXCEEDED

cache quota exhausted
→ CACHE_LIMIT_EXCEEDED

manifest/layout/generation inconsistency
→ CACHE_CORRUPTED
```

这些错误不会包含 Secret。

---

## 12. Local + Remote 混合查询

同一个 Query 可以选择：

```text
Local Source
+
Remote Source
```

两者进入统一 Candidate/Snapshot/Scanner 语义。

真实 Gate 已验证：

```text
Local log  → MIXED local
Remote log → MIXED remote
```

一次查询可以同时返回两侧结果。

---

## 13. 真实 OpenSSH/SFTP Gate

永久 Gate：

```text
.github/workflows/ssh-research.yml
```

M5 实机测试：

```text
tests/m5_remote_query_live.rs
```

代码基线：

```text
53b15786510ebca63f30343f8f34b0c43fbb4edc
```

### 13.1 Rust Gate

Run：

```text
31186919582
```

结果：

```text
cargo fmt                PASS
cargo clippy -D warnings PASS
cargo test               PASS
release build            PASS
```

### 13.2 Contracts Gate

Run：

```text
31186920077
```

结果：

```text
v1/v2 contracts PASS
```

### 13.3 SSH / SFTP Live Gate

Run：

```text
31186920027
```

结果：

```text
locked production compile                 PASS
M2 real SSH/SFTP transport matrix         PASS
M4 real incremental sync                  PASS
M5 real remote query flow                 PASS
research POC compatibility                PASS
```

M5 live flow 具体覆盖：

```text
Remote explicit-file search
Cursor snapshot stability
New-query incremental refresh
Remote directory discovery
Suffix filtering
Local + Remote mixed query
Tail partial coverage rejection
FromNow partial coverage rejection
Rotation/replacement generation switch
Old match_ref after rotation
get_log_context with SSH deliberately unavailable
```

---

## 14. M5 安全边界

M5 没有引入：

```text
ssh_exec
shell
remote grep
arbitrary remote path
upload
write
delete
sudo
service restart
```

RemoteBackend 只使用 M2 已冻结的 read-only Transport：

```text
stat
lstat
read_dir
read_range
```

远程路径仍然只来自管理员配置和受控 directory discovery。

AI 不能提交：

```text
host
port
username
password
secret_ref
remote absolute path
```

---

## 15. M5 已完成能力

```text
Remote explicit files             ✅
Remote non-recursive directories  ✅
Suffix filtering                  ✅
Regular-file validation           ✅
Stable ordering                   ✅
On-query synchronization          ✅
Controlled concurrent refresh     ✅
Local + Remote mixed query        ✅
Cursor cache snapshot             ✅
Cursor no-refresh continuation    ✅
match_ref generation pin          ✅
Context cache-only read           ✅
Rotation-safe old match_ref       ✅
Partial coverage protection       ✅
v2 Remote/Cache error codes       ✅
Permanent real-SFTP query Gate    ✅
```

---

## 16. 留给 M6 的事项

以下不阻塞 M5，但必须在生产发布前完成。

### 16.1 Security Matrix

系统化证明客户端无法：

```text
提交 host/credential/path
利用 path traversal
利用 symlink 越权
调用 Shell
修改远端日志
从错误中获取 Secret
```

### 16.2 多 Remote Server 实机矩阵

M5 架构已经按 `connection_id` 支持多连接，但当前永久 live gate 使用一台 OpenSSH fixture。

M6 应增加至少两台独立 SSH fixture，验证：

```text
Server A + Server B
共享全局并发限制
单服务器失败隔离
混合结果稳定性
```

### 16.3 非法删除 generation

需要 fault test 明确证明：

```text
cursor/match_ref 仍有效
但底层 generation 被外部非法删除
```

系统稳定返回：

```text
CACHE_CORRUPTED / FILE_CHANGED
```

而不是读取错误文件。

### 16.4 Coverage 优化

当前 partial coverage 采用 fail-closed 策略。

M6 或后续版本可以研究：

```text
sparse time → offset index
historical segment cache
query time range coverage proof
```

在证据足够时减少 `CACHE_SCOPE_EXCEEDED`。

### 16.5 性能

M5 Gate 证明功能正确，但没有完成：

```text
100MB
1GB
10GB
```

规模 benchmark。

该项属于 M6。

---

## 17. M5 最终结论

M5 完成后，运行时架构已经从：

```text
AI
 ↓
MCP
 ↓
Local Logs
```

正式演进为：

```text
                         Log Query MCP
                               │
                 ┌─────────────┴─────────────┐
                 │                           │
          LocalBackend                RemoteBackend
                 │                           │
           SafeRoot/openat2          SSH/SFTP discovery
                                             │
                                             ▼
                                         SyncEngine
                                             │
                                             ▼
                                         CacheStore
                                             │
                 ┌───────────────────────────┘
                 │
                 ▼
             SnapshotFile
                 │
                 ▼
        Existing Scanner / Query
                 │
                 ▼
              MCP / AI
```

核心安全与一致性原则保持：

```text
SSH = read-only transport
Cache = local durable snapshot
Query = local operation
Cursor = immutable query snapshot
match_ref = opaque token + server-side generation pin
Remote partial cache = fail closed
```

因此 M5 可以正式关闭，下一阶段进入 M6：安全、故障、性能和生产验收。
