# ADR-0012：ProxyCommand 作为 SSH 底层字节流 Transport

- 状态：Accepted for v2 / M7
- 日期：2026-08-10

## 背景

Log Query MCP v2 已将 Remote Source 约束为只读 SSH/SFTP Transport：远程日志先通过 SFTP 同步到本地 generation cache，再由本地 Scanner / Query Engine 查询；AI-facing MCP API 不暴露 SSH、Shell 或任意远程路径。

当前 SSH Transport 直接连接 `host:port`。这要求运行 Log Query MCP 的网络命名空间本身能够访问目标 SSH Server。但在 WSL、容器、企业 VPN、特殊路由或堡垒网络中，常见情况是：宿主机可以访问目标服务器，而 WSL/容器无法直接访问。

典型场景：

```text
WSL log-query-mcp
      │
      ├── direct TCP ──X──> remote:22
      │
      └── Windows host helper ──> VPN / host network ──> remote:22
```

需要在不扩大 MCP 权限边界的前提下，让 SSH 底层连接可以通过管理员配置的代理进程建立。

## 决策

1. v2 / M7 支持可选 `ProxyCommand` 作为 SSH 底层字节流 Transport。
2. `ProxyCommand` 只负责启动管理员配置的本地程序，并将其 stdin/stdout 作为 SSH raw byte stream；它不是业务 API，也不是远程命令执行能力。
3. 没有 `proxy` 配置时继续使用 Direct TCP，保持现有配置和行为兼容。
4. 有 `proxy.type=command` 时，通过进程 stdin/stdout 获得实现 `AsyncRead + AsyncWrite` 的 stream，再交给 `russh` 的 stream-based connect API。
5. SSH Authentication、strict `known_hosts` Host Key Verification、SFTP、Remote Source、Sync Engine、Cache、Snapshot 和 Query Engine 均保持现有语义。
6. ProxyCommand 只能来自管理员静态配置。MCP Client / AI 不得提交或覆盖 `program`、`args`、`host`、`port` 或任意代理配置。
7. 实现必须直接使用 `program + argv[]` 启动进程，不通过项目内部构造的 `sh -c`、`bash -c`、`cmd /c`、`powershell -Command` 等 Shell 字符串执行。
8. v2 首期 Placeholder 只允许完整 argv 项中的 `{host}` 和 `{port}`；不支持 credential、username、source_id、remote path 或任意表达式。
9. SSH password、private key、private-key passphrase 和 SecretResolver 结果不得传给 ProxyCommand argv 或 stdout。
10. ProxyCommand stdout 必须是纯协议字节流；stderr 仅作为有界、去敏的内部诊断来源，不得原样返回给 AI。
11. Proxy Process 生命周期与 SSH Session 强绑定。连接超时、取消、SSH/SFTP 失败、正常关闭时必须 terminate/wait child，不能遗留 orphan process。
12. ProxyCommand 连接继续占用现有 `max_concurrent_ssh_connections` permit，不能绕过全局 SSH 并发限制。
13. ProxyCommand 失败时继续 fail-closed，不静默查询 stale cache，不制造假阴性。
14. ProxyCommand 不改变 Remote Source 的只读权限边界，也不能绕过 SFTP 进入 Remote Exec / Shell。
15. M7 实现后必须重新执行 Direct SSH regression、ProxyCommand live integration、security/fault matrix、WSL acceptance、large-file/concurrency regression 和 Final RC Gate。

## 配置方向

目标配置：

```json
{
  "connection_id": "test-server-01",
  "type": "ssh",
  "host": "10.20.30.40",
  "port": 22,
  "username": "log-reader",
  "auth": {
    "type": "password",
    "secret_ref": "TEST_SERVER_PASSWORD"
  },
  "host_key": {
    "known_hosts_file": "/home/user/.ssh/known_hosts"
  },
  "proxy": {
    "type": "command",
    "program": "ncat.exe",
    "args": ["{host}", "{port}"]
  }
}
```

`host` / `port` 仍然表示逻辑 SSH 目标，并继续用于 Host Key Verification；ProxyCommand 只是到该目标的底层传输路径。

## 为什么不建设通用命令代理

Log Query MCP 的职责是受控日志读取和查询，不是执行任意系统命令。若提供通用 `run_command` / `ssh_exec`：

- AI 权限会从“只读日志”扩大到“任意本地/远程命令”；
- Remote Exec 会绕过 SFTP-only 和本地 Cache 查询模型；
- 审计、Secret、命令注入和部署能力会与日志查询混杂；
- 违反现有 ADR-0008 的 Remote Exec 禁止边界。

因此 ProxyCommand 必须被限制为 SSH Transport 内部字节流能力。

## 后果

### 正面

- 支持 WSL → Windows Host → VPN / 内网 → SSH Server。
- 支持容器、隔离网络、堡垒网络和自定义 TCP Relay。
- 继续复用现有 SSH Authentication、Host Key Verification、SFTP、Cache 和 Query Engine。
- 不新增 AI-facing MCP Tool，不扩大远程文件/命令权限。
- Direct TCP 配置保持兼容。

### 代价

- 需要增加本地 child-process 生命周期和取消清理逻辑。
- 需要处理 stdout protocol purity、bounded stderr 和错误去敏。
- 需要增加跨平台/WSL acceptance 测试。
- Transport 路径变化后必须重跑性能、并发和最终发布门禁。

## 相关文档

- [`../PROXY_COMMAND_TRANSPORT_V2.md`](../PROXY_COMMAND_TRANSPORT_V2.md)
- [`../CONFIG_SCHEMA_V2.md`](../CONFIG_SCHEMA_V2.md)
- [`0008-use-ssh-sftp-without-remote-exec.md`](./0008-use-ssh-sftp-without-remote-exec.md)
- [`0011-use-russh-and-russh-sftp.md`](./0011-use-russh-and-russh-sftp.md)
