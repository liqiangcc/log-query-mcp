# Log Query MCP v2 M7 ProxyCommand Implementation Baseline

> 状态：Core + failure-classification implementation present / CI and live validation blocked  
> 日期：2026-08-10  
> Draft PR：#25  
> 设计：[`PROXY_COMMAND_TRANSPORT_V2.md`](./PROXY_COMMAND_TRANSPORT_V2.md)  
> TODO：[`PROXY_COMMAND_TODO_V2.md`](./PROXY_COMMAND_TODO_V2.md)  
> ADR：[`adr/0012-use-proxy-command-as-ssh-stream-transport.md`](./adr/0012-use-proxy-command-as-ssh-stream-transport.md)  
> Failure Matrix：[`M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md`](./M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md)

## 1. 当前实现范围

M7 已从设计阶段进入实际 Transport + Fault Model 实现阶段。

当前代码已经具备：

- v2 配置中的可选 `SshConnectionConfig.proxy`。
- `proxy.type=command`。
- `program + args[]` 直接进程启动模型，不构造 Shell command string。
- 仅允许完整 argv 项 `{host}` / `{port}` Placeholder。
- `SshStreamConnector` 将 Direct TCP 与 ProxyCommand 底层 stream 分离。
- Direct TCP 与 ProxyCommand 都通过 `russh::client::connect_stream` 建立 SSH。
- ProxyCommand 通过 `tokio::process::Command` 启动管理员配置的本地 helper。
- child stdin/stdout 作为 SSH raw byte stream。
- strict Host Key Verification、Authentication、SFTP、Cache、Sync、Query 层保持独立。
- `kill_on_drop(true)` + `start_kill()` + Tokio `wait()` reaper 的 fail-closed child cleanup baseline。
- stderr 独立异步 drain，内存 capture 上限 64 KiB，超限后继续 drain 但不继续增长。
- captured stderr 不写日志、不返回 Public Error。
- ProxyCommand 启动/stream/timeout 已有稳定内部 transport 分类。

## 2. 关注分离

实现继续保持：

```text
ProxyCommand = local byte-stream adapter
SSH          = protocol / authentication / host identity
SFTP         = remote read-only file access
Cache        = stable local snapshot
Query Engine = search
MCP          = AI-facing log API
```

`src/transport/proxy_command.rs` 只负责：

```text
validated config
      ↓
local child process
      ↓
stdin / stdout
      ↓
AsyncRead + AsyncWrite
```

它不负责 remote shell、remote command、remote path selection、log search、cache、sync、credential resolution 或 host-key policy。

## 3. Failure Classification

ProxyCommand 现在不会把所有失败都折叠为普通 `ConnectFailed`。

稳定内部 Transport 分类包括：

```text
ProxyCommandNotFound
ProxyCommandPermissionDenied
ProxyCommandStartFailed
ProxyCommandStreamFailed
ProxyCommandTimeout
```

边界保持：

- 配置错误仍然是 `InvalidConfiguration`。
- Host Key mismatch 仍然是 `HostKeyVerificationFailed`。
- SSH Authentication 失败仍然是 `AuthenticationFailed`。
- Direct TCP 仍使用原 `ConnectFailed` / `ConnectTimeout`。
- AI-facing Tool Error contract 没有增加 ProxyCommand 可控字段，也没有暴露 raw OS error/stderr。

## 4. Timeout / Lifecycle 当前语义

`connect_timeout_millis` 继续作为单一建连 deadline，覆盖 ProxyCommand stream establishment + SSH handshake。

ProxyCommand child 被 stream 所有：

```text
ProxyCommandStream
├── ChildStdin
├── ChildStdout
├── Child
└── stderr drain task
```

stream Drop 时：

```text
abort stderr task
→ start_kill child
→ Tokio runtime 可用时 spawn wait/reap task
→ kill_on_drop 作为最终 guard
```

Failure Matrix harness 已增加 timeout/cancellation 下的 PID 观测，并验证 cancellation 后全局 SSH semaphore 应可继续被正常 Proxy connection 使用。真实证据仍需 runner 执行。

## 5. stderr 边界

stdout 是 SSH protocol stream，绝不进入日志系统。

stderr：

- 使用 pipe 独立排空，避免 helper 因 stderr pipe 写满而阻塞。
- 最多在内存中保留 64 KiB。
- 超过上限后继续排空但不继续增长。
- 不写日志、不进入 Public Error、不返回 MCP Client。

Failure Matrix 使用 128 KiB stderr flood helper，证明目标行为是“仍受 connect timeout 控制并回收 child”，而不是被 stderr pipe 卡死。

## 6. 已落地的主要提交

```text
8f9c6c3  ProxyCommand machine JSON Schema
3c3a1a1  Rust ProxyCommand config/runtime validation
8abd4bd  Direct SSH stream abstraction
39aec36  ProxyCommand process stream adapter
538b500  bounded stderr + child lifecycle hardening
08e6867  classify ProxyCommand startup failures
35d2322  classify ProxyCommand transport failures
3fd1ec7  ProxyCommand failure-matrix tests
efd07da  dedicated M7 ProxyCommand failure workflow
3844653  scope orphan-process check to Proxy helper PIDs only
```

## 7. Live / Failure Harness

已经存在两条独立 M7 gates：

```text
M7 ProxyCommand
M7 ProxyCommand Failures
```

成功链路 gate 覆盖：

```text
ProxyCommand → OpenSSH → known_hosts → password auth → SFTP read
wrong host key → fail closed
```

Failure gate 当前覆盖 harness：

```text
program not found
permission denied / non-executable helper
early exit / stdout EOF
stderr flood + connect timeout
cancellation + child reap + semaphore release
```

剩余主要 fault evidence：

```text
authentication failure through ProxyCommand
active-session proxy crash / network disconnect
SFTP failure through ProxyCommand
mixed Direct + Proxy isolation
```

## 8. 当前验证状态

GitHub Actions 仍受 Issue #23 的 Billing / Spending Limit 外部阻塞影响。

`M7 ProxyCommand Failures` 已被 GitHub 正确识别并触发。候选 `efd07da701bc3e18a89c3204a797d32e01229982` 的 PR run `31372138200` 中，job `proxy-command-failures` 结束为 failure，但 `steps=null`：runner 没有执行任何一步。

因此当前必须记录为：

```text
implementation present           YES
failure classification           IMPLEMENTED
success live harness             IMPLEMENTED
failure-matrix harness           IMPLEMENTED (partial matrix)
compile evidence                 NO
rustfmt/clippy evidence          NO
failure-matrix execution         BLOCKED
Direct SSH regression            NO NEW PASS EVIDENCE
ProxyCommand live SSH            NOT VALIDATED
WSL acceptance                   NOT VALIDATED
RC ready                         NO
```

这些 workflow failure 不能解释为代码测试失败，也不能解释为 PASS。

## 9. 下一阶段

下一步重点：

1. 补 authentication failure through ProxyCommand。
2. 补 active-session proxy crash / disconnect，并验证 reader fail/broken 与 child cleanup。
3. 建立 Direct + Proxy mixed transport/isolation gate。
4. 验证同一 global SSH semaphore 同时约束 Direct/Proxy。
5. Billing 恢复后真正执行 Success + Failure 两条 M7 gates。
6. 最后执行真实 WSL → Windows Host → Remote SSH acceptance 与性能回归。

## 10. 当前完成定义

```text
M7 design                         DONE
ADR                               DONE (0012)
config schema/runtime             DONE
config fixtures                   DONE
stream abstraction                IMPLEMENTED / CI BLOCKED
ProxyCommand core connector       IMPLEMENTED / CI BLOCKED
child cleanup baseline            IMPLEMENTED / CI BLOCKED
bounded stderr drain              IMPLEMENTED / CI BLOCKED
failure classification            IMPLEMENTED / CI BLOCKED
ProxyCommand live harness         IMPLEMENTED / EXECUTION BLOCKED
failure matrix harness            PARTIAL / EXECUTION BLOCKED
mixed Direct+Proxy isolation      TODO
WSL acceptance                    TODO
performance regression            TODO
release docs/final gates          TODO
RC ready                          NO
```

在真实 gates 通过以前，不应把 M7 标记为 production-ready，也不应把 PR #25 转为 Ready。
