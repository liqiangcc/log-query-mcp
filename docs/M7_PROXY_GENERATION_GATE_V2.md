# Log Query MCP v2 M7 ProxyCommand Generation Consistency Gate

> 状态：Harness implemented / execution blocked before runner start  
> 日期：2026-08-10  
> Draft PR：#25

## 1. 目标

该 gate 验证 ProxyCommand 只改变远端 SSH stream 的建立方式，不改变 Stateful Query 的 snapshot / cursor / match_ref / generation pin 语义。

必须证明：

```text
cursor       = frozen candidate snapshots
match_ref    = source + file + pinned generation
get_context  = cache-only against the referenced generation
```

任何 ProxyCommand refresh、append、replacement 或其他 source 的活动都不能让已有 cursor / match_ref 漂移到新的 generation 或其他 source。

## 2. 独立 Gate

```text
tests/m7_proxy_generation_live.rs
.github/workflows/m7-proxy-generation.yml
```

测试使用两个独立 Proxy-backed source：

```text
proxy-generation-a → proxy-a.log
proxy-generation-b → proxy-b.log
```

两者都通过管理员配置的：

```text
/usr/bin/nc {host} {port}
```

连接同一只读 OpenSSH/SFTP fixture，但拥有独立 source_id、file_id、cache generation 和 match_ref。

## 3. Cursor Snapshot 不变量

Source A 初始内容：

```text
M7CURSOR one
M7CURSOR two
M7KEEP source-a-old-generation
```

步骤：

1. `max_results=1` 查询 `M7CURSOR`，得到第一页与 cursor。
2. 远端 append：`M7CURSOR appended-after-cursor`。
3. 使用旧 cursor 获取第二页。
4. 再发起一个不带 cursor 的 fresh query。

预期：

```text
old cursor page 2
→ only M7CURSOR two
→ appended line invisible

fresh query
→ one
→ two
→ appended-after-cursor
```

这证明 cursor 持有首次查询时的 candidate snapshots，而不是在翻页时重新同步当前远端文件。

## 4. Match Reference Generation Pin

在 Source A 和 Source B 分别创建 match_ref：

```text
A → M7KEEP source-a-old-generation
B → M7KEEP source-b-stable
```

随后完全替换 Source A 文件：

```text
M7NEW source-a-replacement-generation
```

fresh query 必须看到新 generation。

旧 A match_ref 仍必须指向 replacement 之前的 generation；B match_ref 仍必须指向 B 自己的 source/file/generation。

## 5. Cache-Only Context 证明

在创建新 generation 后，测试暂时移走 `known_hosts` 文件，使任何新的 SSH/ProxyCommand refresh 都无法通过 host verification。

随后调用：

```text
StatefulContextService::get_context(A old match_ref)
StatefulContextService::get_context(B match_ref)
```

两者仍必须成功，因为 context 应只读取 match_ref 持有的本地 generation pin。

断言：

```text
A context.source_id == proxy-generation-a
A context == M7KEEP source-a-old-generation

B context.source_id == proxy-generation-b
B context == M7KEEP source-b-stable

A.file_id != B.file_id
```

这同时证明：

```text
no generation drift
no source crossover
no network dependency for existing match_ref context
```

## 6. 安全边界

该 gate 不新增任何生产能力。

它只使用现有：

```text
ProxyCommand stream
SSH/SFTP read-only transport
Sync/Cache
Stateful Query
Stateful Context
```

没有新增 Shell、remote exec、write/upload/delete、AI-controlled command 或 arbitrary remote path。

## 7. 当前执行状态

GitHub 已识别 `M7 Proxy Generation` workflow。

候选：

```text
90c45a56820774208f42c6c198deda253c3016d9
```

PR run：

```text
workflow: M7 Proxy Generation
run:      31378377040
job:      proxy-generation-live
result:   failure
steps:    null
```

runner 未执行任何 step，与 Issue #23 的 GitHub Actions Billing / Spending Limit blocker 一致。

因此目前只能记录：

```text
generation-consistency harness   IMPLEMENTED
workflow recognition             CONFIRMED
actual cargo check/test          BLOCKED
PASS evidence                    NONE
```

## 8. 完成条件

- [ ] cursor snapshot freeze through ProxyCommand PASS。
- [ ] fresh query observes appended generation PASS。
- [ ] old match_ref survives Source A replacement PASS。
- [ ] Source A/B match_ref 不串 source/file PASS。
- [ ] `get_log_context` cache-only generation pin PASS。
- [ ] known_hosts 不可用时 existing match_ref context 仍 PASS。
- [ ] rustfmt / clippy / all-targets regression PASS。

在真实 gate 通过前，本项只能标记为 harness implemented，不能标记 production evidence complete。
