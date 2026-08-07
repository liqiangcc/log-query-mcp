# Log Query MCP v2 M3 实现基线

> 状态：M3 CacheStore implementation complete  
> 日期：2026-08-07  
> 分支：`feat/v2-m1-backend-config`  
> M3 代码 Gate 基线：`f7e0617141d2a88c48e44e2dee897c4342a31a85`  
> 核心业务实现提交：`d642022504b2190b89ac69dce6c01e0261997fd9`  
> 上游清单：[`REMOTE_SSH_CACHE_TODO_V2.md`](./REMOTE_SSH_CACHE_TODO_V2.md)

## 1. M3 目标

M3 建立一个可恢复、有界、多 generation、可保护活动 snapshot 的本地 CacheStore。

M3 不负责：

- SSH/SFTP 增量同步策略；
- continuity fingerprint 的计算策略；
- rotation/replacement 判定；
- Remote Source discovery；
- Query Engine 集成；
- cursor / match_ref 与 cache generation 的端到端绑定。

这些职责分别属于 M4 SyncEngine 和 M5 Remote Source 查询集成。

---

## 2. Cache Layout

缓存目录被视为内部数据库，不是远程目录镜像。

```text
<cache-root>/
├── catalog.json
└── sources/
    └── <opaque-source-id>/
        └── <opaque-file-id>/
            ├── manifest.json
            ├── generations/
            │   └── <generation-id>.log
            └── .staging/
                └── <internal-staging-id>.tmp
```

### 2.1 不透明 ID

`source_id`、`file_id`、`generation_id` 使用随机 UUID v4 的 32 位十六进制表示。

首次出现的：

```text
source_identifier -> source_id
remote_identifier -> file_id
```

会原子保存到 `catalog.json`，因此：

- 重启后 ID 稳定；
- 远程路径不会直接进入本地路径；
- 从远程路径不能直接推导缓存目录；
- `..`、绝对 remote identifier 等路径逃逸形式会被拒绝。

### 2.2 权限

Unix 下：

```text
directory = 0700
file      = 0600
```

读取已有缓存文件时同样拒绝 symlink / 非普通文件。

---

## 3. Catalog / Manifest

### 3.1 Catalog

当前版本：

```text
CACHE_CATALOG_VERSION = 1
```

Catalog 保存稳定的内部 ID 映射，并校验：

- source identifier 唯一；
- source ID 唯一；
- 同 source 下 remote identifier 唯一；
- file ID 全局唯一；
- opaque ID 格式有效。

### 3.2 Manifest

当前版本：

```text
CACHE_MANIFEST_VERSION = 1
```

每个文件 manifest 保存：

```text
source_identifier
source_id
file_id
remote_identifier
current_generation
generations[]
updated_at_unix_millis
```

每个 generation 保存：

```text
generation
remote_size
cached_range
remote_mtime_millis
last_sync_unix_millis
continuity_fingerprint
coverage
data_len
created_at_unix_millis
```

Coverage 支持：

```text
full
tail(start_offset)
from_now(start_offset)
```

Manifest 会检查 range、coverage、data length、current generation、重复 generation 等一致性约束。

---

## 4. 原子持久化协议

### 4.1 新 Generation

`begin_generation()` 用于 bootstrap、新物理日志、truncate、replacement 或 continuity mismatch 后的新 generation。

提交顺序：

```text
write staging
→ flush
→ fsync staging data
→ rename staging -> generations/<id>.log
→ fsync generations directory
→ build/validate manifest
→ write temporary manifest
→ fsync temporary manifest
→ rename temporary manifest -> manifest.json
→ fsync manifest parent directory
```

关键不变量：

```text
manifest 永远不会先指向未完成的数据文件
```

如果数据文件 rename 成功但 manifest 尚未提交时进程崩溃，重启 recovery 会把该文件识别为 orphan generation 并清理。

### 4.2 正常 Append

正常日志增长通过 `begin_append()` 延续当前 generation，不创建新的 generation ID。

新增数据先写独立 staging 文件。

commit 时：

```text
fsync append staging
→ 获取 CacheStore 元数据锁
→ 重新读取 manifest
→ 验证 current generation 未变化
→ 验证原 GenerationRecord 未变化
→ 验证 append range 精确连续
→ append 到当前 generation data
→ fsync generation data
→ 原子更新 manifest
```

如果 manifest 更新在当前进程内失败，会把 generation data 截回 append 前长度。

如果进程在：

```text
data append + fsync
        ↓ crash
manifest commit
```

之间崩溃，重启 recovery 会比较：

```text
actual_data_len > manifest.data_len
```

并把多余尾部截回 `manifest.data_len`。

反过来：

```text
actual_data_len < manifest.data_len
```

被视为缓存损坏并稳定报错，不静默伪造数据。

---

## 5. Generation 语义

CacheStore 提供两个显式写入入口：

```text
begin_generation()
begin_append()
```

语义：

```text
正常 append
    -> 保持 current generation

truncate
replacement
continuity mismatch
新物理日志
    -> 新 generation
```

M3 只提供安全的存储原语。

由 M4 SyncEngine 根据远程 metadata 和 continuity fingerprint 决定具体走 append 还是 new generation。

旧 generation 不会被新 generation 直接覆盖。

---

## 6. Snapshot / Pin

`pin_generation()` 和 `pin_current_generation()` 返回 `PinnedGeneration`。

Pin 同时完成两个职责：

1. 增加 generation 的活动引用计数，阻止 GC 删除；
2. 固定 snapshot 的 `data_len`。

即使同一个 generation 后续继续 append，旧的 `PinnedGeneration` 也只能读取 pin 时记录的长度：

```text
pin at len = 100
remote/cache append to len = 150
old pin still sees [0, 100)
new pin sees [0, 150)
```

Seek 同样不能越过 pinned snapshot 的长度。

因此 append 不会破坏已有 Query Snapshot 的稳定性。

M5 集成时，cursor / match_ref / Query Snapshot 必须通过该 pin 机制持有所引用 generation；M3 不提前修改现有 Query Engine。

---

## 7. Recovery

`CacheStore::open()` 会执行 recovery。

Recovery 当前覆盖：

- catalog schema/version 校验；
- manifest schema/version 校验；
- catalog / manifest identity 一致性；
- staging orphan 清理；
- manifest 不存在时的 orphan generation 清理；
- manifest 未引用的 generation 清理；
- generation 文件必须为普通文件；
- generation 文件长度小于 manifest 时稳定失败；
- generation 文件长度大于 manifest 时按已提交长度回滚；
- restart 后恢复并读取有效 generation。

损坏 manifest 不会被当作空缓存继续运行。

---

## 8. Quota / GC

GC planner 当前实现：

```text
retention
max_generations_per_file
max_bytes_per_source
max_bytes
```

保护规则：

```text
current generation -> protected
pinned generation  -> protected
```

清理顺序遵循旧 generation 优先，并且不会为了满足 quota 删除 current/pinned generation。

当所有可删除 generation 都清理后仍无法满足限制时返回：

```text
CacheStoreError::CacheLimitExceeded
```

这对应对外错误语义：

```text
CACHE_LIMIT_EXCEEDED
```

GC 的持久化顺序为：

```text
先原子更新 manifest，移除 generation 引用
→ 再删除 generation data file
```

因此 GC 中途崩溃最多留下 orphan data，不会让 manifest 指向已被提前删除的数据。

---

## 9. M3 测试覆盖

当前 CacheStore / GC 单元测试覆盖至少包括：

```text
opaque ID + 私有权限
restart recovery
abandoned staging
corrupt manifest detection
normal append keeps generation
old pinned snapshot cannot see later append
uncommitted append tail rollback
pinned old generation survives GC
quota cannot evict protected current generation
GC planner protected generation behavior
invalid remote identifier / path escape
coverage/range validation
```

在临时验证 runner 上，append 变更先通过：

```text
cargo fmt --all -- --check
cargo check --locked --all-targets --all-features
cargo test --locked cache::
```

之后才提交业务代码。

临时写权限 workflow 和 patch script 均已从分支删除。

---

## 10. M3 Gate 结论

### Cache 可重启恢复

通过。

### Cache 写入原子

通过。

新 generation 使用 staging + fsync + rename + atomic manifest。

append 使用 staging + data fsync + atomic manifest，并支持 crash tail rollback。

### 活动 generation 不被 GC 错误删除

通过存储层 pin 机制验证。

端到端 cursor / match_ref 生命周期绑定留给 M5，因为 M3 不应提前耦合 Query Engine。

---

## 11. CI Gate

最终只读分支形态已通过：

```text
Rust
- cargo fmt --check
- cargo clippy --locked --all-targets --all-features -- -D warnings
- cargo test --locked --all-targets --all-features
- release binaries build

Contracts
- v1/v2 contract checks
```

M3 代码 Gate：

```text
Rust run:      31176472987
Contracts run: 31176473059
```

---

## 12. 下一阶段：M4 SyncEngine

M4 的职责是把 M2 SSH/SFTP Transport 和 M3 CacheStore 连接起来：

```text
Remote SFTP
    ↓
metadata / read_range
    ↓
SyncEngine
    ├── bootstrap full
    ├── bootstrap tail(bytes)
    ├── bootstrap from_now
    ├── no-change detection
    ├── incremental append
    ├── continuity fingerprint
    ├── truncate detection
    ├── replacement / rotation detection
    └── failure recovery
    ↓
CacheStore
```

M4 必须继续保持：

```text
同步失败不破坏最后有效 cache
rotation 不把不同物理日志拼接
正常查询路径只下载新增 range
```

在 M4 完成前，不把 Remote Source 接入现有 Query Engine。
