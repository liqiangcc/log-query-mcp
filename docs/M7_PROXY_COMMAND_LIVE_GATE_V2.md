# Log Query MCP v2 M7 ProxyCommand Live Gate

> 状态：Harness implemented / execution blocked before runner start  
> 日期：2026-08-10  
> Draft PR：#25  
> Implementation baseline：[`M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md`](./M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md)

## 1. 目的

M7 ProxyCommand 不能只依赖 config/unit test。最终必须证明真实链路：

```text
log-query-mcp
    ↓
ProxyCommand process
    ↓ stdin/stdout raw bytes
netcat stdio proxy helper
    ↓
OpenSSH server
    ↓
strict host-key verification
    ↓
password authentication
    ↓
SFTP
    ↓
read-only log range read
```

因此新增独立 live gate，而不是继续扩大原 M2-M6 SSH Transport workflow。

## 2. 关注分离

新增：

```text
tests/m7_proxy_command_live.rs
.github/workflows/m7-proxy-command.yml
```

M7 workflow 只负责 ProxyCommand transport 的真实验证；原 `ssh-research.yml` 继续负责 M2-M6 Direct SSH/SFTP、sync、query、security、multi-server、restart 和历史 concurrency gates。

这样避免一个 workflow 同时承担所有 transport milestone 的 fixture 与诊断责任。

## 3. 当前成功路径测试

`proxy_command_reaches_openssh_and_reads_sftp`：

1. 配置 `proxy.type=command`。
2. `program=/usr/bin/nc`。
3. argv 为完整占位符：

```json
["{host}", "{port}"]
```

4. `ProxyCommandStream` 启动：

```text
/usr/bin/nc 127.0.0.1 2240
```

5. child stdin/stdout 交给 `russh::client::connect_stream`。
6. 使用正常 `known_hosts`。
7. 使用现有 password authentication。
8. 建立 SFTP。
9. 从 `/home/logreader/logs/application.log` 执行 bounded range read。
10. 断言 offset 6、length 5 返回：

```text
world
```

这条测试用于证明 ProxyCommand 不是“配置能解析”，而是真的可以承载完整 SSH/SFTP transport。

## 4. Host Key 安全测试

`proxy_command_does_not_bypass_strict_host_key_verification` 使用相同 ProxyCommand 网络路径，但提供错误 host key。

预期：

```text
SshTransportError::HostKeyVerificationFailed
```

这证明：

```text
ProxyCommand != trust boundary
```

Host Key Verification 继续校验逻辑目标：

```text
connection.host
connection.port
```

而不是 helper process、本机进程或代理路径。

## 5. Workflow Fixture

独立 workflow：

```text
.github/workflows/m7-proxy-command.yml
```

fixture 包括：

- Ubuntu GitHub-hosted runner。
- `openssh-server`。
- `netcat-openbsd`。
- 独立 `logreader` 用户。
- password authentication。
- `internal-sftp`。
- read-only `application.log`。
- 独立 ed25519 host key。
- 正常 `known_hosts`。
- 错误 `bad_known_hosts`。
- `M7_PROXY_PROGRAM=/usr/bin/nc`。

运行前执行：

```text
cargo fmt --all -- --check
cargo check --locked --all-targets --all-features
```

然后运行：

```text
cargo test --locked --test m7_proxy_command_live -- --ignored --test-threads=1 --nocapture
```

## 6. 当前执行状态

提交 `18e908be91f19db1a0af67451a503d47b975b878` 后，GitHub 已正确识别并触发 `M7 ProxyCommand` workflow。

观察到：

```text
workflow: M7 ProxyCommand
run:      31371193438
job:      proxy-command-live
status:   completed
result:   failure
steps:    none
```

job 没有执行任何 runner step，与 Issue #23 记录的 GitHub Actions Billing / Spending Limit 外部 blocker 一致。

因此当前只能记录：

```text
live gate harness          IMPLEMENTED
workflow trigger           CONFIRMED
fixture execution          BLOCKED
cargo fmt/check evidence   NO
ProxyCommand SSH evidence  NO
host-key evidence          NO
PASS                       NO
```

不能把 workflow 的 `failure` 解释成代码测试失败，也不能把 harness 已实现解释成测试已通过。

## 7. Billing 恢复后的验收

必须重新执行同一 candidate 或更新后的最终 candidate，并确认：

- [ ] `cargo fmt` PASS。
- [ ] all-target cargo check PASS。
- [ ] ProxyCommand process 可以连接真实 OpenSSH。
- [ ] password authentication PASS。
- [ ] strict known_hosts PASS。
- [ ] SFTP range read PASS。
- [ ] wrong host key fail-closed PASS。
- [ ] workflow job 实际存在 runner steps。
- [ ] 没有 orphan ProxyCommand process。

## 8. 下一阶段

live success path harness 建立以后，M7-4 继续扩展 failure matrix：

```text
program not found
permission denied / spawn failure
proxy early exit
stdout EOF
stderr flood
connect timeout
cancellation
SSH handshake failure
authentication failure
proxy crash during active session
```

同时 M7-3 需要增加更稳定的 ProxyCommand failure classification，避免所有 ProxyCommand 启动/生命周期错误都折叠为普通 `ConnectFailed`。

## 9. 当前结论

M7 现在已经具备可重复执行的真实 ProxyCommand/OpenSSH gate，但由于外部 CI Billing blocker，还没有真实 PASS evidence。

RC readiness 仍然是：

```text
NO
```
