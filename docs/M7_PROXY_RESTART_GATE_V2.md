# Log Query MCP v2 M7 ProxyCommand Restart / Stale-Cache Gate

> 状态：Harness implemented / execution blocked before runner start  
> 日期：2026-08-10  
> Draft PR：#25  
> TODO：[`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)

## 1. 目标

本 gate 验证 ProxyCommand 不改变现有 Sync / Cache fail-closed 语义。

核心不变量：

```text
allow_stale_on_error = false
```

意味着远端刷新失败时：

```text
last valid cache may remain stored locally
but query must fail explicitly
and must not return stale cache as a successful result
```

同时，远端恢复后必须能重新通过 ProxyCommand 完成 SSH/SFTP、同步新增内容并恢复查询。

## 2. 关注分离

本 gate 验证：

```text
ProxyCommand
→ SSH
→ SFTP
→ Sync
→ Cache
→ StatefulQueryService
```

它不增加新的 MCP Tool，也不改变 ProxyCommand 本身的权限模型。

测试文件：

```text
tests/m7_proxy_restart_live.rs
```

独立 workflow：

```text
.github/workflows/m7-proxy-restart.yml
```

## 3. 三阶段证据链

### Phase 1 — Bootstrap through ProxyCommand

远端文件初始内容：

```text
M7_RESTART_BASE before-restart
```

查询链路：

```text
/usr/bin/nc {host} {port}
→ OpenSSH
→ SFTP
→ full bootstrap
→ local generation cache
→ query PASS
```

断言：

- query 返回初始行；
- current cache generation 精确包含初始内容。

### Phase 2 — Server outage must fail closed

停止真实 OpenSSH server，但保留 Phase 1 的本地 cache。

随后在远端文件追加：

```text
M7_RESTART_AFTER after-restart
```

此时执行 on-query refresh。

期望：

```text
ProxyCommand cannot establish usable SSH stream
→ remote refresh fails
→ ToolErrorCode::RemoteUnavailable
```

关键断言：

- 查询不能成功；
- 不能返回旧 cache 的 `M7_RESTART_BASE` 作为成功结果；
- 最后一个有效 cache generation 仍保留，用于恢复而不是用于 silent stale fallback。

### Phase 3 — Restart and recovery

重新启动同一 OpenSSH server，然后再次查询。

期望：

```text
ProxyCommand reconnects
→ SSH/SFTP succeeds
→ Sync observes appended content
→ cache advances
→ query returns both old and new lines
```

最终 cache：

```text
M7_RESTART_BASE before-restart
M7_RESTART_AFTER after-restart
```

## 4. 与 M6 Direct Restart Gate 的关系

M6 已证明 Direct TCP 路径具备同样的 restart / stale-cache fail-closed 行为。

M7 gate 的目的不是重新设计 Cache 或 Sync，而是证明替换底层连接建立方式后，上层不变量不回退：

```text
Direct TCP       ─┐
                  ├→ SSH/SFTP → Sync/Cache/Query semantics unchanged
ProxyCommand     ─┘
```

## 5. 当前执行状态

GitHub 已正确识别 `M7 Proxy Restart` workflow。

候选：

```text
c0f9b819dd94190397dde9cd60e89a19ddd7cd50
```

PR run：

```text
workflow: M7 Proxy Restart
run:      31374965163
job:      proxy-restart-live
result:   failure
steps:    null
```

runner 没有执行任何 step，与 Issue #23 的 GitHub Actions Billing / Spending Limit blocker 一致。

因此当前只能记录：

```text
restart harness                   IMPLEMENTED
stale-cache fail-closed harness   IMPLEMENTED
recovery harness                  IMPLEMENTED
workflow recognition              CONFIRMED
actual cargo/test execution       BLOCKED
PASS evidence                     NONE
```

## 6. 完成条件

- [ ] Phase 1 bootstrap through ProxyCommand PASS。
- [ ] Phase 2 server outage returns `REMOTE_UNAVAILABLE` PASS。
- [ ] Phase 2 does not silently serve stale cache PASS。
- [ ] Last valid cache remains intact during outage PASS。
- [ ] Phase 3 restart/reconnect PASS。
- [ ] Appended remote content is synchronized after recovery PASS。
- [ ] No orphan ProxyCommand process / leaked SSH permit。
- [ ] No regression in existing Direct restart behavior。

在真实 runner 证据完成前，本 gate 不能标记 DONE，PR #25 继续保持 Draft。
