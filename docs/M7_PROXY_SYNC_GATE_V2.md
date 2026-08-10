# Log Query MCP v2 M7 ProxyCommand Sync Semantics Gate

> 状态：Harness implemented / execution blocked before runner start  
> 日期：2026-08-10  
> Draft PR：#25

## 1. 目标

验证 ProxyCommand 只替换 SSH stream 的建立方式，不改变 M4 已冻结的 SyncEngine / Cache generation 语义。

独立覆盖：

```text
full bootstrap
incremental append
tail(bytes)
from_now
truncate
same-path rotation / replacement
```

## 2. 独立 Gate

```text
tests/m7_proxy_sync_live.rs
.github/workflows/m7-proxy-sync.yml
```

真实链路：

```text
/usr/bin/nc {host} {port}
→ SSH/password auth/known_hosts
→ SFTP lstat + read_range
→ SyncEngine
→ CacheStore
```

该 gate 不测试 SSH key authentication；密钥认证由独立 `M7 Proxy Auth` gate 负责。

## 3. Full + Incremental Append

```text
initial full bootstrap
→ NewGeneration(InitialBootstrap)
→ CacheCoverage::Full

remote append
→ SyncAction::Appended
→ generation id unchanged
→ only appended payload is written
```

目标缓存内容：

```text
first
second
```

## 4. Tail

初始远端文件包含历史内容与尾部：

```text
old-history
TAIL one
```

配置 `tail(bytes)` 只缓存 `TAIL one`。

预期：

```text
CacheCoverage::Tail { start_offset }
→ historical prefix is outside queryable coverage
→ subsequent append stays on same generation
```

## 5. From Now

首次同步时：

```text
cached bytes = 0
coverage = FromNow { start_offset = remote_size }
```

历史正文不能进入 cache payload。

随后 remote append：

```text
FROMNOW new
```

预期只缓存新增内容，并保持原 generation。

## 6. Truncate

```text
full bootstrap of larger file
→ remote file shrinks
→ NewGeneration(RemoteTruncated)
→ old generation is not appended to
```

新 current generation 只包含 truncate 后内容。

## 7. Same-Path Rotation / Replacement

测试模拟：

```text
sync-rotate.log
→ rename to sync-rotate.log.1
→ create a new sync-rotate.log with same byte length but different content
```

因为 size 相同，不能依赖 size shrink；旧 continuity fingerprint 必须检测替换：

```text
NewGeneration(ContinuityMismatch)
```

这证明 ProxyCommand 不削弱 continuity fingerprint 语义。

## 8. 安全边界

Sync 仍只使用：

```text
lstat
read_range
```

不引入 remote exec、shell、write/upload/delete 或 AI-controlled remote path。

ProxyCommand 仍只负责 raw byte stream。

## 9. 当前执行状态

候选 `9abb48c20801ffb0fce63ada609716652f37d88d` 已触发：

```text
workflow: M7 Proxy Sync
run:      31378855371
job:      proxy-sync-live
result:   failure
steps:    null
```

runner 未执行任何 step，与 Issue #23 的 Billing blocker 一致。

当前只能记录：

```text
full/append harness       IMPLEMENTED
tail harness              IMPLEMENTED
from_now harness          IMPLEMENTED
truncate harness          IMPLEMENTED
rotation harness          IMPLEMENTED
workflow recognition      CONFIRMED
actual execution          BLOCKED
PASS evidence             NONE
```

## 10. 完成条件

- [ ] full bootstrap PASS。
- [ ] incremental append PASS。
- [ ] tail coverage/pass PASS。
- [ ] from_now coverage/pass PASS。
- [ ] truncate generation rollover PASS。
- [ ] same-path rotation continuity mismatch PASS。
- [ ] cache content/generation assertions PASS。
- [ ] rustfmt / clippy / all-targets regression PASS。

真实 gate 通过前，本项只能标记 harness implemented。
