# Log Query MCP v2 M7 ProxyCommand Failure Matrix

> 状态：Expanded harness implemented / execution blocked before runner start  
> 日期：2026-08-10  
> Draft PR：#25  
> Implementation baseline：[`M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md`](./M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md)

## 1. 目标

M7 Failure Matrix 用于证明 ProxyCommand 不只是“能连接”，还需要在启动、认证、握手、超时、取消和 active-session 进程异常情况下保持：

```text
fail closed
no orphan process
no leaked SSH semaphore permit
no raw stderr / secret leakage
no change to host-key/auth trust boundary
failure isolation between Direct and Proxy transports
```

Failure Matrix 独立于正常成功链路 gate：

```text
.github/workflows/m7-proxy-command.yml
    → success / host-key path

.github/workflows/m7-proxy-command-failures.yml
    → process / auth / timeout / cancellation / active-session fault injection
```

正常功能验证与故障注入继续保持关注分离。

## 2. 稳定内部错误分类

当前 `SshTransportError` 的 ProxyCommand transport-only 分类：

```text
ProxyCommandNotFound
ProxyCommandPermissionDenied
ProxyCommandStartFailed
ProxyCommandStreamFailed
ProxyCommandTimeout
```

映射原则：

| 场景 | 分类 |
|---|---|
| helper 不存在 | `ProxyCommandNotFound` |
| helper 无执行权限 | `ProxyCommandPermissionDenied` |
| spawn/stdio 初始化失败 | `ProxyCommandStartFailed` |
| helper early exit / handshake byte stream 中断 | `ProxyCommandStreamFailed` |
| ProxyCommand + SSH handshake 超过统一 deadline | `ProxyCommandTimeout` |
| wrong host key | `HostKeyVerificationFailed` |
| wrong SSH credential | `AuthenticationFailed` |
| active session 中 transport 消失 | SFTP operation error → reader `Broken` |

ProxyCommand 不覆盖 SSH Host Identity 或 Authentication 的原始分类。

## 3. 当前 Harness 覆盖

### 3.1 Program Not Found

测试：

```text
missing_proxy_program_has_stable_classification
```

目标：

```text
spawn
→ io::ErrorKind::NotFound
→ ProxyCommandNotFound
```

不得返回 raw executable path 或 OS error 给 AI。

### 3.2 Permission Denied

测试：

```text
non_executable_proxy_program_has_stable_classification
```

Workflow 创建 mode `0644` 的普通文件并尝试作为 ProxyCommand program 启动。

目标分类：

```text
ProxyCommandPermissionDenied
```

### 3.3 Early Exit / stdout EOF

测试：

```text
early_proxy_exit_is_classified_as_stream_failure
```

使用 `/usr/bin/false` 立即退出，SSH handshake 无法完成。

目标分类：

```text
ProxyCommandStreamFailed
```

### 3.4 stderr Flood + Connect Timeout

测试：

```text
stderr_flood_still_obeys_proxy_connect_timeout_and_reaps_child
```

helper：

1. 写入 PID。
2. 向 stderr 写入 128 KiB。
3. 不提供有效 SSH stream。
4. sleep 30 秒。

Transport 目标：

```text
capture <= 64 KiB
continue drain after limit
never expose raw stderr
connect_timeout still fires
child kill/reap
```

### 3.5 Cancellation + Semaphore Release

测试：

```text
cancelling_proxy_connect_reaps_child_and_releases_global_permit
```

配置：

```text
max_concurrent_ssh_connections = 1
```

步骤：

```text
start stalling ProxyCommand
→ helper writes PID
→ abort open_reader task
→ ProxyCommandStream Drop
→ child disappears
→ open second healthy ProxyCommand connection
```

第二条连接成功意味着取消路径没有永久泄漏 global SSH semaphore permit。

### 3.6 Wrong Password Through ProxyCommand

测试：

```text
wrong_password_through_proxy_preserves_authentication_error_and_reaps_child
```

使用真实 TCP 字节转发 helper 建立 ProxyCommand stream，但通过独立 `secret_ref` 注入错误密码。

目标：

```text
ProxyCommand stream established
→ strict host key succeeds
→ SSH authentication fails
→ AuthenticationFailed
→ tracked proxy helper is reaped
```

这证明 ProxyCommand 不会把认证失败错误降级成普通 stream/connect failure，也不会在认证失败后遗留 helper process。

### 3.7 Active Proxy Crash / SFTP Failure

测试：

```text
active_proxy_crash_breaks_sftp_reader_fail_closed
```

Workflow 提供单进程 controlled TCP proxy helper：

```text
stdin/stdout ↔ TCP target
PID file
trigger file
```

测试先完成：

```text
ProxyCommand
→ SSH handshake
→ Authentication
→ SFTP
→ stat PASS
```

然后创建 trigger file，使 proxy helper 在 active session 中退出。

目标：

```text
proxy process exits
→ target TCP connection closes
→ next SFTP operation fails
→ reader marks itself broken
→ subsequent operation returns Broken
```

这同时覆盖：

```text
active ProxyCommand crash
active network disconnect
SFTP operation failure through ProxyCommand
fail-closed reader state
```

### 3.8 Direct + Proxy Mixed Isolation

测试：

```text
stalled_proxy_does_not_break_active_direct_transport
```

配置：

```text
max_concurrent_ssh_connections = 2
ProxyCommand connection = intentionally stalled
Direct connection       = healthy OpenSSH
```

步骤：

```text
Proxy path acquires one global permit and stalls
→ Direct path acquires second permit
→ Direct SFTP stat succeeds
→ cancel stalled Proxy path
→ proxy child reaped
→ Direct SFTP stat succeeds again
```

该测试证明：

```text
Direct and Proxy share the global concurrency budget
but transport failure state is isolated per connection
```

Proxy cancellation不能破坏已经建立的 Direct SSH/SFTP session。

## 4. Controlled Proxy Helper 边界

Failure workflow 中的 controlled helper 只是测试 fixture：

```text
stdin/stdout ↔ TCP socket
```

它不执行：

```text
SSH command
remote shell
remote grep
filesystem access
credential resolution
MCP request handling
```

trigger file 只用于测试环境故障注入，不进入生产配置模型。

## 5. Orphan Process 验证

测试内部通过 `/proc/<pid>` 观察 helper 是否退出。

Workflow 结束时再次检查：

```text
timeout.pid
cancel.pid
auth.pid
crash.pid
mixed.pid
```

只检查 ProxyCommand helper PID，不检查 OpenSSH fixture 的 `sshd.pid`。

历史上 `*.pid` 误匹配 `sshd.pid` 的问题已在提交 `3844653d991d34dc68f8985206d31a8b0a637443` 修复。

## 6. 当前执行状态

新 Failure Matrix harness 已提交，但 GitHub Actions Billing / Spending Limit blocker 尚未解除。

因此当前只能记录：

```text
error classification code             IMPLEMENTED
program/spawn failure harness          IMPLEMENTED
timeout/cancellation harness           IMPLEMENTED
auth failure + child cleanup harness   IMPLEMENTED
active crash/SFTP failure harness      IMPLEMENTED
Direct+Proxy isolation harness         IMPLEMENTED
workflow orphan assertions             IMPLEMENTED
actual cargo check/test                BLOCKED
failure evidence                       NO PASS EVIDENCE
```

任何 `steps=null` 的 run 都不能作为 PASS，也不能解释为已知代码测试失败。

## 7. 尚未覆盖的 Fault / Integration

Failure Matrix transport 层剩余重点已明显减少，后续主要是：

```text
server restart through ProxyCommand
silent stale-cache fallback rejection at Sync/Backend layer
multi-remote query isolation above transport layer
```

成功链路仍需补：

```text
private key auth
encrypted private key + passphrase
full/tail/from_now sync
incremental append / rotation / truncate
Remote Query through cache
```

目标环境仍需：

```text
WSL → Windows Host ProxyCommand → Remote SSH acceptance
performance / concurrency regression
```

## 8. 安全结论

Failure Classification 和 fault injection 都只存在于 SSH Transport / CI 测试层。

它们不会新增：

```text
MCP Tool
AI-controlled command
raw stderr response
raw OS error response
credential placeholder
remote shell
```

因此架构仍保持：

```text
ProxyCommand = connection mechanism
not command-execution capability
```

## 9. 完成条件

Failure Matrix 完整完成前至少需要：

- [ ] program-not-found 真实 PASS。
- [ ] permission-denied 真实 PASS。
- [ ] early-exit/stdout-EOF 真实 PASS。
- [ ] stderr-flood/timeout 真实 PASS。
- [ ] cancellation/reap/semaphore 真实 PASS。
- [ ] wrong-password/auth cleanup 真实 PASS。
- [ ] active proxy crash/SFTP broken-state 真实 PASS。
- [ ] mixed Direct/Proxy isolation 真实 PASS。
- [ ] 所有 PID cleanup assertion PASS。
- [ ] 无 raw stderr / secret leakage。
- [ ] 上层 stale-cache fail-closed 证据仍成立。

在上述证据完成前，M7 Failure Matrix 不能标记 DONE，PR #25 继续保持 Draft。
