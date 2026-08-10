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

测试：`missing_proxy_program_has_stable_classification`

```text
spawn
→ io::ErrorKind::NotFound
→ ProxyCommandNotFound
```

### 3.2 Permission Denied

测试：`non_executable_proxy_program_has_stable_classification`

Workflow 创建 mode `0644` 的普通文件并尝试作为 ProxyCommand program 启动，目标分类 `ProxyCommandPermissionDenied`。

### 3.3 Early Exit / stdout EOF

测试：`early_proxy_exit_is_classified_as_stream_failure`

使用 `/usr/bin/false` 立即退出，SSH handshake 无法完成，目标分类 `ProxyCommandStreamFailed`。

### 3.4 stderr Flood + Connect Timeout

测试：`stderr_flood_still_obeys_proxy_connect_timeout_and_reaps_child`

helper 写入 PID、向 stderr 写 128 KiB、不提供有效 SSH stream 并 sleep。

目标：

```text
capture <= 64 KiB
continue drain after limit
connect timeout still fires
ProxyCommandTimeout
child kill/reap
```

### 3.5 Cancellation + Semaphore Release

测试：`cancelling_proxy_connect_reaps_child_and_releases_global_permit`

```text
max_concurrent_ssh_connections = 1
stalling ProxyCommand acquires permit
→ abort open_reader
→ child reaped
→ second healthy ProxyCommand can open
```

### 3.6 Wrong Password Through ProxyCommand

测试：`wrong_password_through_proxy_preserves_authentication_error_and_reaps_child`

使用可追踪 PID 的真实 TCP byte-stream proxy，并通过独立 `secret_ref` 注入错误密码。

```text
ProxyCommand stream established
→ strict host key succeeds
→ SSH authentication fails
→ AuthenticationFailed
→ proxy child reaped
```

这证明认证边界仍属于 SSH Authentication，而不是 ProxyCommand Transport。

### 3.7 Active Proxy Crash / SFTP Failure

测试：`active_proxy_crash_breaks_sftp_reader_fail_closed`

controlled proxy 先完成：

```text
ProxyCommand → SSH → Authentication → SFTP → stat PASS
```

然后 CI-only trigger 让 proxy helper 在 active session 中退出：

```text
proxy exits
→ TCP closes
→ next SFTP operation fails
→ reader becomes broken
→ subsequent operation returns Broken
```

该 harness 同时覆盖 active proxy crash、active network disconnect、SFTP operation failure 和 fail-closed broken latch。

### 3.8 Direct + Proxy Mixed Isolation

测试：`stalled_proxy_does_not_break_active_direct_transport`

```text
max_concurrent_ssh_connections = 2
Proxy path occupies one permit and stalls
→ Direct path acquires second permit and reads SFTP
→ Proxy task cancelled/reaped
→ Direct path reads SFTP again
```

目标语义：

```text
shared global concurrency budget
+ per-connection failure isolation
```

## 4. Controlled Proxy Helper 边界

Failure workflow 的 controlled helper 只是测试 fixture：

```text
stdin/stdout ↔ TCP socket
PID file
CI-only trigger file
```

不执行 SSH command、remote shell、remote grep、remote filesystem business logic、credential resolution 或 MCP request handling。

## 5. Orphan Process 验证

测试内部通过 `/proc/<pid>` 观察 helper 是否退出。

Workflow 结束时检查：

```text
timeout.pid
cancel.pid
auth.pid
crash.pid
mixed.pid
```

只检查 ProxyCommand helper PID，不检查 OpenSSH fixture 的 `sshd.pid`。

## 6. 当前执行状态

最新 expanded harness candidate 曾达到：

```text
dd3310b7250824bbfc259c80bc16f30e2d2d52fd
```

其 PR workflow：

```text
workflow: M7 ProxyCommand Failures
run:      31373870218
job:      proxy-command-failures
result:   failure
steps:    null
```

runner 没有执行任何 step，与 Issue #23 的 GitHub Actions Billing / Spending Limit blocker 一致。

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

`steps=null` 不能作为 PASS，也不能解释为已知代码测试失败。

## 7. 尚未覆盖的 Fault / Integration

Transport fault harness 剩余重点：

```text
server restart through ProxyCommand
silent stale-cache fallback rejection at Sync/Backend layer
multi-remote query isolation above transport layer
```

成功链路仍需补 private/encrypted key、Sync/Cache/Query；目标环境仍需 WSL → Windows Host → Remote SSH acceptance 与 performance regression。

## 8. 安全结论

Failure Classification 和 fault injection 都只存在于 SSH Transport / CI 测试层，不新增 MCP Tool、AI-controlled command、credential placeholder、raw stderr/OS error response 或 remote shell。

```text
ProxyCommand = connection mechanism
not command-execution capability
```

## 9. 完成条件

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

在真实证据完成前，M7 Failure Matrix 不能标记 DONE，PR #25 继续保持 Draft。
