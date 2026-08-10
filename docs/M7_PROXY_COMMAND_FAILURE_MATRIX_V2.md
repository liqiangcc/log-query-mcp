# Log Query MCP v2 M7 ProxyCommand Failure Matrix

> 状态：Harness implemented / execution blocked before runner start  
> 日期：2026-08-10  
> Draft PR：#25  
> Implementation baseline：[`M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md`](./M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md)

## 1. 目标

M7 Failure Matrix 用于证明 ProxyCommand 不只是“能连接”，还需要在启动、握手、超时、取消和进程异常情况下保持：

```text
fail closed
no orphan process
no leaked SSH semaphore permit
no raw stderr / secret leakage
no change to host-key/auth trust boundary
```

Failure Matrix 独立于正常成功链路 gate：

```text
.github/workflows/m7-proxy-command.yml
    → success / host-key path

.github/workflows/m7-proxy-command-failures.yml
    → process / timeout / cancellation fault injection
```

这让正常功能验证和故障注入保持关注分离。

## 2. 稳定内部错误分类

当前 `SshTransportError` 增加 ProxyCommand transport-only 分类：

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
| helper early exit / SSH byte stream 中断 | `ProxyCommandStreamFailed` |
| ProxyCommand + SSH handshake 超过统一 deadline | `ProxyCommandTimeout` |
| wrong host key | `HostKeyVerificationFailed` |
| wrong SSH credential | `AuthenticationFailed` |

ProxyCommand 不覆盖 SSH Host Identity 或 Authentication 的原始分类。

## 3. 当前 Harness 覆盖

### 3.1 Program Not Found

测试：

```text
missing_proxy_program_has_stable_classification
```

目标：管理员配置了不存在的 `program` 时：

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

Workflow 创建普通文件：

```text
mode = 0644
```

然后作为 ProxyCommand program 启动。

目标分类：

```text
ProxyCommandPermissionDenied
```

### 3.3 Early Exit / stdout EOF

测试：

```text
early_proxy_exit_is_classified_as_stream_failure
```

使用：

```text
/usr/bin/false
```

helper 立即退出，stdout EOF，SSH handshake 无法完成。

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

1. 写入自身 PID。
2. 向 stderr 写入 128 KiB 数据。
3. 不提供有效 SSH stream。
4. sleep 30 秒。

Transport stderr collector：

```text
capture <= 64 KiB
continue drain after limit
never expose raw stderr
```

测试目标：

```text
stderr pipe 不阻塞 helper/transport
→ connect_timeout 触发
→ ProxyCommandTimeout
→ child 被 kill/reap
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
→ start_kill + wait/reap
→ helper process disappears
→ open second healthy ProxyCommand connection
```

如果第二条连接能够进入 SSH/SFTP，则证明取消路径没有永久占用 global SSH semaphore permit。

## 4. Orphan Process 验证

测试内部通过：

```text
/proc/<pid>
```

观察 helper 是否退出。

Workflow 结束时再次检查：

```text
timeout.pid
cancel.pid
```

只检查 ProxyCommand helper 的 PID 文件，不检查 `sshd.pid`。

曾发现初版 workflow 使用 `*.pid`，会把 OpenSSH fixture 的 `sshd.pid` 错认为 orphan helper。该误报已在提交 `3844653d991d34dc68f8985206d31a8b0a637443` 中修复。

## 5. 当前执行状态

GitHub 已正确识别 `M7 ProxyCommand Failures` workflow。

候选：

```text
efd07da701bc3e18a89c3204a797d32e01229982
```

观察到 PR run：

```text
workflow: M7 ProxyCommand Failures
run:      31372138200
job:      proxy-command-failures
result:   failure
steps:    null
```

runner 没有执行任何 step，与 Issue #23 的 GitHub Actions Billing / Spending Limit blocker 一致。

因此当前状态只能是：

```text
error classification code       IMPLEMENTED
failure tests                    IMPLEMENTED
failure workflow                 IMPLEMENTED
workflow recognition             CONFIRMED
actual cargo check/test          BLOCKED
failure evidence                 NO PASS EVIDENCE
```

## 6. 尚未覆盖的 Faults

下一轮优先补：

```text
wrong password through ProxyCommand
proxy crash during active SFTP session
active network disconnect
SFTP operation failure through ProxyCommand
Direct + Proxy mixed transport isolation
```

后续再覆盖：

```text
server restart
full/tail/from_now sync through proxy
incremental append / rotation / truncate
performance / concurrency regression
```

## 7. 安全结论

Failure Classification 只存在于内部 SSH Transport 错误层。

它不会新增：

```text
MCP Tool
AI-controlled command
raw stderr response
raw OS error response
credential placeholder
remote shell
```

因此 M7 仍保持：

```text
ProxyCommand = connection mechanism
not command-execution capability
```

## 8. 完成条件

Failure Matrix 完整完成前至少需要：

- [ ] 当前 program-not-found 测试真实 PASS。
- [ ] 当前 permission-denied 测试真实 PASS。
- [ ] 当前 early-exit/stdout-EOF 测试真实 PASS。
- [ ] 当前 stderr-flood/timeout 测试真实 PASS。
- [ ] 当前 cancellation/reap/semaphore 测试真实 PASS。
- [ ] wrong-password through proxy PASS。
- [ ] active proxy crash PASS。
- [ ] mixed Direct/Proxy isolation PASS。
- [ ] 所有 PID cleanup assertion PASS。
- [ ] 无 raw stderr / secret leakage。

在上述证据完成前，M7 Failure Matrix 不能标记 DONE，PR #25 继续保持 Draft。
