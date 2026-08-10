# Log Query MCP v2 M7 ProxyCommand Implementation Baseline

> 状态：Core + fault + mixed-query + restart harness present / CI and live validation blocked  
> 日期：2026-08-10  
> Draft PR：#25  
> 设计：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> TODO：[`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)  
> Failure Matrix：[`M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md`](./M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md)  
> Restart Gate：[`M7_PROXY_RESTART_GATE_V2.md`](./M7_PROXY_RESTART_GATE_V2.md)  
> ADR：[`adr/0012-use-proxy-command-as-ssh-stream-transport.md`](./adr/0012-use-proxy-command-as-ssh-stream-transport.md)

## 1. 当前实现范围

M7 当前已经具备：

- admin-only `SshConnectionConfig.proxy` / `type=command`。
- direct `program + args[]` spawn，无 Shell command string。
- only whole-argument `{host}` / `{port}` placeholders。
- `SshStreamConnector` 分离 Direct TCP 与 ProxyCommand。
- Direct / ProxyCommand 都进入 `russh::client::connect_stream`。
- ProxyCommand child stdin/stdout = SSH raw byte stream。
- strict Host Key Verification、Authentication、SFTP、Cache、Sync、Query 层不变。
- fail-closed child cleanup + bounded stderr drain。
- stable ProxyCommand startup/stream/timeout internal classifications。
- success live harness。
- expanded process/auth/timeout/cancellation/active-session failure harness。
- Direct + Proxy transport isolation harness。
- SourceRegistry / StatefulQueryService 的 Local + Direct + Proxy mixed-query harness。
- ProxyCommand server-restart / stale-cache fail-closed / recovery harness。

## 2. 关注分离

```text
ProxyCommand = local byte-stream adapter
SSH          = protocol / authentication / host identity
SFTP         = remote read-only file access
Sync         = remote-to-local synchronization
Cache        = stable local snapshot
Query Engine = search
MCP          = AI-facing log API
```

M7 没有让 ProxyCommand 参与 remote path、credential、cache、sync 或 query 业务逻辑。Transport fault、mixed query、restart/stale-cache 分别使用独立 test/workflow。

## 3. Failure / Lifecycle Baseline

稳定内部分类：

```text
ProxyCommandNotFound
ProxyCommandPermissionDenied
ProxyCommandStartFailed
ProxyCommandStreamFailed
ProxyCommandTimeout
```

边界保持：

- wrong host key → `HostKeyVerificationFailed`。
- wrong credential → `AuthenticationFailed`。
- active transport loss → SFTP failure，reader latch `Broken`。
- raw OS error / stderr 不进入 AI-facing error。

Failure harness 已覆盖 startup、early EOF、stderr flood、timeout、cancellation、child reap、semaphore release、wrong-password、active proxy crash 和 Direct+Proxy isolation。

## 4. Mixed Query Integration Baseline

独立：

```text
tests/m7_mixed_query_live.rs
.github/workflows/m7-mixed-query.yml
```

目标链路：

```text
Local source ────────────────┐
Direct SSH → SFTP → Cache ───┼→ SourceRegistry → StatefulQueryService → search
Proxy → SSH → SFTP → Cache ──┘
```

Harness 已实现：

- one request 返回 Local + Direct + Proxy 三个 source；
- bad Proxy source 显式 `REMOTE_UNAVAILABLE`；
- bad Proxy failure 后同一个 query service 仍能查询 Local + Direct + healthy Proxy。

## 5. Restart / Stale-Cache Baseline

新增：

```text
tests/m7_proxy_restart_live.rs
.github/workflows/m7-proxy-restart.yml
docs/M7_PROXY_RESTART_GATE_V2.md
```

三阶段语义：

```text
Phase 1
ProxyCommand → SSH/SFTP → bootstrap cache → query PASS

Phase 2
stop sshd
→ on-query refresh fails
→ REMOTE_UNAVAILABLE
→ allow_stale_on_error=false prevents stale-success response
→ last valid generation remains stored locally

Phase 3
restart sshd
→ ProxyCommand reconnects
→ append detected/synchronized
→ cache advances
→ query recovers
```

这证明 ProxyCommand 只替换连接建立方式，不改变 M6 已冻结的 Sync / Cache fail-closed 不变量。

## 6. 主要 M7 提交

```text
08e6867  classify ProxyCommand startup failures
35d2322  classify ProxyCommand transport failures
3fd1ec7  initial failure-matrix tests
efd07da  dedicated failure workflow
de8349f  expand auth/crash/mixed transport tests
bb0ae0b  expand active failure CI fixture
ec7dcba  add M7 mixed-query live tests
bea09b5  add M7 Mixed Query workflow
71d7021  add ProxyCommand restart live tests
c0f9b81  add M7 Proxy Restart workflow
ce66dd3  document restart/stale-cache gate
```

## 7. 当前验证状态

GitHub Actions Billing / Spending Limit 仍是外部 blocker。

最新 restart harness candidate：

```text
c0f9b819dd94190397dde9cd60e89a19ddd7cd50
```

GitHub 已识别：

```text
workflow = M7 Proxy Restart
run      = 31374965163
job      = proxy-restart-live
result   = failure
steps    = null
```

runner 未执行任何 step。

因此当前必须记录：

```text
implementation present                 YES
failure classification                 IMPLEMENTED
success live harness                   IMPLEMENTED
expanded failure harness               IMPLEMENTED
Direct+Proxy isolation harness         IMPLEMENTED
full mixed-query harness               IMPLEMENTED
restart/stale-cache harness             IMPLEMENTED
compile/rustfmt/clippy evidence         NO CURRENT PASS
M7 workflow execution                  BLOCKED
Direct SSH regression                  NO NEW PASS EVIDENCE
ProxyCommand live SSH                  NOT VALIDATED
mixed query                            NOT VALIDATED
restart/stale-cache                    NOT VALIDATED
WSL acceptance                         NOT VALIDATED
performance regression                 NOT VALIDATED
RC ready                               NO
```

`steps=null` 既不能视为已知代码失败，也不能视为 PASS。

## 8. 下一阶段

Transport、fault、mixed-query、restart/stale-cache 的 harness 已基本闭合。下一步优先：

1. 补 Proxy source 的 cursor / match_ref generation consistency。
2. 扩成功 gate：private key / encrypted key + passphrase。
3. 扩 Sync success：full / tail / from_now / incremental / rotation / truncate through ProxyCommand。
4. 记录 ProxyCommand connection setup / throughput / concurrency 回归。
5. Billing 恢复后执行 Rust / Contracts / Direct / M7 success / failure / mixed-query / restart gates。
6. 最后执行真实 WSL → Windows Host → Remote SSH acceptance。

## 9. 当前完成定义

```text
M7 design                         DONE
ADR                               DONE (0012)
config schema/runtime             DONE
stream abstraction                IMPLEMENTED / CI BLOCKED
ProxyCommand core connector       IMPLEMENTED / CI BLOCKED
child cleanup + stderr            IMPLEMENTED / CI BLOCKED
failure classification            IMPLEMENTED / CI BLOCKED
ProxyCommand live harness         IMPLEMENTED / EXECUTION BLOCKED
failure matrix harness            EXPANDED / EXECUTION BLOCKED
Direct+Proxy transport isolation  IMPLEMENTED / EXECUTION BLOCKED
full mixed query                  IMPLEMENTED / EXECUTION BLOCKED
restart/stale-cache harness        IMPLEMENTED / EXECUTION BLOCKED
cursor/match_ref proxy evidence   TODO
WSL acceptance                    TODO
performance regression            TODO
release docs/final gates          TODO
RC ready                          NO
```

真实 gates 通过前，不应把 M7 标记 production-ready，也不应把 PR #25 转为 Ready。
