# Log Query MCP v2 M7 ProxyCommand Failure Matrix

> 状态：Expanded harness + mixed-query isolation implemented / execution blocked before runner start  
> 日期：2026-08-10  
> Draft PR：#25  
> Implementation baseline：[`M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md`](./M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md)

## 1. 目标

M7 Failure Matrix 用于证明 ProxyCommand 不只是“能连接”，还需要在启动、认证、握手、超时、取消、active-session 进程异常和上层 query isolation 情况下保持：

```text
fail closed
no orphan process
no leaked SSH semaphore permit
no raw stderr / secret leakage
no change to host-key/auth trust boundary
failure isolation between Direct and Proxy transports
healthy sources remain independently queryable
```

测试按关注分离拆分：

```text
.github/workflows/m7-proxy-command.yml
    → success / host-key path

.github/workflows/m7-proxy-command-failures.yml
    → process / auth / timeout / cancellation / active-session fault injection

.github/workflows/m7-mixed-query.yml
    → SourceRegistry / Sync / Cache / Query Engine mixed-source isolation
```

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

## 3. Transport Failure Harness

`tests/m7_proxy_command_failures.rs` 当前覆盖：

### 3.1 Program Not Found

`missing_proxy_program_has_stable_classification`

```text
spawn
→ io::ErrorKind::NotFound
→ ProxyCommandNotFound
```

### 3.2 Permission Denied

`non_executable_proxy_program_has_stable_classification`

mode `0644` 的普通文件作为 program，目标分类 `ProxyCommandPermissionDenied`。

### 3.3 Early Exit / stdout EOF

`early_proxy_exit_is_classified_as_stream_failure`

`/usr/bin/false` 立即退出，SSH handshake 无法完成，目标分类 `ProxyCommandStreamFailed`。

### 3.4 stderr Flood + Connect Timeout

`stderr_flood_still_obeys_proxy_connect_timeout_and_reaps_child`

helper 写 PID、stderr 128 KiB、无有效 SSH stream 并 sleep。

目标：

```text
capture <= 64 KiB
continue drain after limit
connect timeout still fires
ProxyCommandTimeout
child kill/reap
```

### 3.5 Cancellation + Semaphore Release

`cancelling_proxy_connect_reaps_child_and_releases_global_permit`

```text
max_concurrent_ssh_connections = 1
stalling ProxyCommand acquires permit
→ abort open_reader
→ child reaped
→ second healthy ProxyCommand can open
```

### 3.6 Wrong Password Through ProxyCommand

`wrong_password_through_proxy_preserves_authentication_error_and_reaps_child`

```text
ProxyCommand stream established
→ strict host key succeeds
→ SSH authentication fails
→ AuthenticationFailed
→ proxy child reaped
```

### 3.7 Active Proxy Crash / SFTP Failure

`active_proxy_crash_breaks_sftp_reader_fail_closed`

```text
ProxyCommand → SSH → Authentication → SFTP → stat PASS
→ CI-only trigger terminates proxy
→ next SFTP operation fails
→ reader becomes Broken
→ subsequent operation returns Broken
```

### 3.8 Direct + Proxy Transport Isolation

`stalled_proxy_does_not_break_active_direct_transport`

```text
max_concurrent_ssh_connections = 2
Proxy path occupies one permit and stalls
→ Direct path acquires second permit and reads SFTP
→ Proxy task cancelled/reaped
→ Direct path reads SFTP again
```

目标语义：shared global concurrency budget + per-connection failure isolation。

## 4. Mixed Query Source Isolation

新增：

```text
tests/m7_mixed_query_live.rs
.github/workflows/m7-mixed-query.yml
```

### 4.1 Healthy mixed query

同一个 `StatefulQueryService` 配置：

```text
Local Source
Direct Remote
ProxyCommand Remote
```

一次 `search` 请求必须经过：

```text
Direct/Proxy → SSH/SFTP → Sync → local generation cache
Local source ───────────────────────────────────────────┐
Remote cached snapshots ────────────────────────────────┼→ SourceRegistry → Query Engine
```

并返回三个 source 的结果。

### 4.2 Failed Proxy source isolation

额外配置：

```text
failed Proxy Remote
program = /usr/bin/false
```

先单独查询 failed Proxy source：

```text
Proxy stream fails
→ remote sync/query fails explicitly
→ ToolErrorCode::RemoteUnavailable
```

然后使用同一个 query service 查询：

```text
Local + Direct Remote + healthy Proxy Remote
```

必须仍返回三个健康 source 的结果。

这个 harness 证明的是 source/session isolation；它不改变现有请求语义为 partial-results 模式。如果客户端把失败 source 与健康 source 放进同一个原子请求，仍遵循现有 fail-closed contract。

## 5. Controlled Proxy Helper 边界

Failure workflow 的 controlled helper 只是测试 fixture：

```text
stdin/stdout ↔ TCP socket
PID file
CI-only trigger file
```

不执行 SSH command、remote shell、remote grep、remote filesystem business logic、credential resolution 或 MCP request handling。

## 6. Orphan Process 验证

Failure workflow 检查：

```text
timeout.pid
cancel.pid
auth.pid
crash.pid
mixed.pid
```

只检查 ProxyCommand helper PID，不检查 OpenSSH `sshd.pid`。

## 7. 当前执行状态

GitHub 已正确识别三条 M7 gates。

最新 mixed-query candidate：

```text
bea09b5eb91bdd6ab26312bc57c6c150b8f45994
```

PR workflow：

```text
workflow: M7 Mixed Query
run:      31374390180
job:      mixed-query-live
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
mixed-query source isolation harness   IMPLEMENTED
workflow recognition                   CONFIRMED
actual cargo check/test                BLOCKED
failure evidence                       NO PASS EVIDENCE
```

`steps=null` 不能作为 PASS，也不能解释为已知代码测试失败。

## 8. 尚未覆盖的 Fault / Integration

下一轮重点：

```text
server restart through ProxyCommand
allow_stale_on_error=false regression after Proxy failure
cursor/match_ref generation consistency through Proxy source
```

成功链路仍需补 private/encrypted key、full/tail/from_now、incremental append/rotation/truncate；目标环境仍需 WSL → Windows Host → Remote SSH acceptance 与 performance regression。

## 9. 安全结论

Failure Classification、fault injection 和 mixed-query harness 都没有新增：

```text
MCP Tool
AI-controlled command
raw stderr response
raw OS error response
credential placeholder
remote shell
partial-results semantic change
```

```text
ProxyCommand = connection mechanism
not command-execution capability
```

## 10. 完成条件

- [ ] program-not-found / permission / early-exit 真实 PASS。
- [ ] stderr-flood/timeout/cancellation 真实 PASS。
- [ ] wrong-password/auth cleanup 真实 PASS。
- [ ] active proxy crash/SFTP broken-state 真实 PASS。
- [ ] mixed Direct/Proxy transport isolation 真实 PASS。
- [ ] Local + Direct + Proxy mixed query 真实 PASS。
- [ ] failed Proxy source isolation 真实 PASS。
- [ ] server restart through Proxy PASS。
- [ ] stale-cache fail-closed PASS。
- [ ] 所有 PID cleanup assertion PASS。
- [ ] 无 raw stderr / secret leakage。

在真实证据完成前，M7 Failure Matrix 不能标记 DONE，PR #25 继续保持 Draft。
