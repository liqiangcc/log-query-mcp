# Log Query MCP v2 M7 ProxyCommand Implementation Baseline

> 状态：Core + expanded fault harness present / CI and live validation blocked  
> 日期：2026-08-10  
> Draft PR：#25  
> 设计：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> TODO：[`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)  
> ADR：[`adr/0012-use-proxy-command-as-ssh-stream-transport.md`](./adr/0012-use-proxy-command-as-ssh-stream-transport.md)  
> Failure Matrix：[`M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md`](./M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md)

## 1. 当前实现范围

M7 当前已具备：

- optional admin-only `SshConnectionConfig.proxy` / `type=command`。
- direct `program + args[]` spawn，无 Shell command string。
- only whole-argument `{host}` / `{port}` placeholders。
- `SshStreamConnector` 分离 Direct TCP 与 ProxyCommand。
- Direct / ProxyCommand 都进入 `russh::client::connect_stream`。
- ProxyCommand child stdin/stdout = SSH raw byte stream。
- strict Host Key Verification、Authentication、SFTP、Cache、Sync、Query 层不变。
- `kill_on_drop + start_kill + async wait/reap` child cleanup baseline。
- stderr 独立 drain，capture 上限 64 KiB，不返回 raw stderr。
- stable ProxyCommand startup/stream/timeout internal classifications。
- success live harness。
- expanded process/auth/timeout/cancellation/active-session/mixed-transport failure harness。

## 2. 关注分离

```text
ProxyCommand = local byte-stream adapter
SSH          = protocol / authentication / host identity
SFTP         = remote read-only file access
Cache        = stable local snapshot
Query Engine = search
MCP          = AI-facing log API
```

`src/transport/proxy_command.rs` 只负责本地 child process 与 stdin/stdout stream，不负责 remote shell、remote command、remote paths、credentials、cache、sync 或 query。

## 3. Failure Classification

稳定内部分类：

```text
ProxyCommandNotFound
ProxyCommandPermissionDenied
ProxyCommandStartFailed
ProxyCommandStreamFailed
ProxyCommandTimeout
```

边界：

- wrong host key → `HostKeyVerificationFailed`。
- wrong SSH credential → `AuthenticationFailed`。
- active session transport 消失 → SFTP operation fail，然后 reader latch 为 `Broken`。
- Direct TCP 仍使用原 Direct 错误。
- AI-facing error contract 不暴露 raw OS error/stderr，也没有新增 command input。

## 4. Lifecycle / stderr

`connect_timeout_millis` 继续覆盖 stream establishment + SSH handshake。

ProxyCommand stream Drop：

```text
abort stderr drain
→ start_kill child
→ async wait/reap when runtime available
→ kill_on_drop final guard
```

stderr：

```text
pipe drain independently
capture <= 64 KiB
continue draining beyond the cap
never expose raw stderr
```

## 5. Expanded Failure Harness

`.github/workflows/m7-proxy-command-failures.yml` 当前包含：

```text
program not found
permission denied
early exit / stdout EOF
stderr flood + connect timeout
timeout child reap
cancellation child reap
cancellation semaphore release
wrong password through real ProxyCommand
AuthenticationFailed preservation
auth-failure child reap
active ProxyCommand crash after SSH/SFTP establishment
SFTP failure + Broken latch after transport loss
stalled ProxyCommand + active Direct SSH isolation
workflow orphan-helper assertions
```

active-session crash 使用 controlled single-process TCP proxy fixture：

```text
stdin/stdout ↔ TCP target
PID file
CI-only trigger file
```

trigger 只用于测试故障注入，不属于生产配置或 MCP API。

## 6. Mixed Direct / Proxy Transport Evidence Harness

```text
max_concurrent_ssh_connections = 2
Proxy path occupies one permit and stalls
Direct path occupies second permit and reads SFTP successfully
Proxy task is cancelled/reaped
Direct path reads SFTP successfully again
```

目标语义：shared global concurrency budget + per-connection failure isolation。

仍需补 SourceRegistry / Query Engine 层的 Local + Direct + multiple Proxy mixed query evidence。

## 7. 主要新增提交

```text
08e6867  classify ProxyCommand startup failures
35d2322  classify ProxyCommand transport failures
3fd1ec7  initial ProxyCommand failure-matrix tests
efd07da  dedicated M7 failure workflow
3844653  fix orphan PID scope
de8349f  expand auth/crash/mixed transport tests
bb0ae0b  expand active failure CI fixture
b4d547c  expand Failure Matrix documentation
0d77ebb  update M7 TODO state
```

## 8. 当前验证状态

GitHub Actions Billing / Spending Limit 仍是外部 blocker。

expanded harness candidate `dd3310b7250824bbfc259c80bc16f30e2d2d52fd` 触发 `M7 ProxyCommand Failures` run `31373870218`，其 `proxy-command-failures` job 为：

```text
result = failure
steps  = null
```

runner 未执行任何 step。因此当前状态：

```text
implementation present                 YES
failure classification                 IMPLEMENTED
success live harness                   IMPLEMENTED
expanded failure harness               IMPLEMENTED
Direct+Proxy isolation harness         IMPLEMENTED
compile/rustfmt/clippy evidence         NO CURRENT PASS
failure harness execution              BLOCKED
Direct SSH regression                  NO NEW PASS EVIDENCE
ProxyCommand live SSH                  NOT VALIDATED
full mixed query                       NOT VALIDATED
WSL acceptance                         NOT VALIDATED
performance regression                 NOT VALIDATED
RC ready                               NO
```

`steps=null` 既不能视为已知代码失败，也不能视为 PASS。

## 9. 下一阶段

实现工作下一步从 Transport fault harness 转向上层集成：

1. 补 Local + Direct Remote + Proxy Remote 的 SourceRegistry / Query Engine mixed query。
2. 验证一个 Proxy remote failure 不影响其他 Local/Direct/Proxy source。
3. 补 server restart through ProxyCommand 与 stale-cache fail-closed 回归。
4. 补 success gate 的 private/encrypted key 与 Sync/Cache/Query 链路。
5. Billing 恢复后运行全部真实 gates。
6. 最后执行 WSL → Windows Host → Remote SSH acceptance 与性能回归。

## 10. 当前完成定义

```text
M7 design                         DONE
ADR                               DONE (0012)
config schema/runtime             DONE
stream abstraction                IMPLEMENTED / CI BLOCKED
ProxyCommand core connector       IMPLEMENTED / CI BLOCKED
child cleanup baseline            IMPLEMENTED / CI BLOCKED
bounded stderr drain              IMPLEMENTED / CI BLOCKED
failure classification            IMPLEMENTED / CI BLOCKED
ProxyCommand live harness         IMPLEMENTED / EXECUTION BLOCKED
failure matrix harness            EXPANDED / EXECUTION BLOCKED
Direct+Proxy transport isolation  IMPLEMENTED / EXECUTION BLOCKED
full mixed query                  TODO
WSL acceptance                    TODO
performance regression            TODO
release docs/final gates          TODO
RC ready                          NO
```

真实 gates 通过前，不应把 M7 标记 production-ready，也不应把 PR #25 转为 Ready。
