# Log Query MCP

面向 AI 问题排查的只读日志搜索 MCP 服务。

Log Query MCP 只向 AI 暴露受控的日志语义能力，不暴露 Shell、任意文件读取或 SSH Exec。管理员预先配置允许查询的来源，AI 只通过 `source_id` 搜索日志并读取有限上下文。

## 部署模式

支持 Local 与 Remote SSH 来源，并且可以在同一个查询中混合使用。Remote SSH 又支持 Direct TCP 和管理员配置的 ProxyCommand 两种底层连接方式。

### Local Source

适合 MCP 与日志位于同一台 Linux 服务器的场景：

```text
AI → Log Query MCP → openat2() → local logs
```

Local Source 保留 v1 的本地安全边界：来源白名单、`openat2()`、禁止 symlink/magiclink、禁止跨挂载点、只读取普通文件。

### Remote SSH Source

适合不希望在每台业务服务器安装 MCP/Agent 的开发、测试和多服务器场景：

```text
AI
 ↓
local Log Query MCP
 ↓
SSH + SFTP read-only
 ↓
remote log server
 ↓
incremental local cache
 ↓
existing scanner / query engine
```

Remote Source 的关键边界：

- SSH 只作为内部传输层，不是 MCP 工具。
- 只使用 SFTP `lstat/read_dir/read_range` 等只读操作，不执行远端命令。
- host、username、credential、remote root 都只能由管理员配置，AI 请求不能提交这些字段。
- Password 和 encrypted private key 都通过 `secret_ref` / `passphrase_secret_ref` 从运行环境解析；普通配置不保存明文密码。
- Host Key Verification 默认强制开启，使用管理员提供的 `known_hosts_file`。
- 远端日志先增量同步到本地 generation cache，再由现有 Scanner 查询；Query Engine 不直接访问 SSH。
- cursor 和 `match_ref` 固定到本地 cache snapshot/generation，日志轮转后旧引用在 TTL 内仍可稳定读取。
- Tail / FromNow 缓存范围不足时返回 `CACHE_SCOPE_EXCEEDED`，不会把不完整缓存误报成“没有匹配”。

#### Direct TCP 与 ProxyCommand

默认不配置 `proxy` 时，SSH 使用 Direct TCP：

```text
log-query-mcp → TCP → SSH/SFTP target
```

当 MCP 运行环境无法直接访问目标，但管理员控制的本地/宿主机 helper 可以访问目标时，可使用 ProxyCommand：

```text
log-query-mcp
  → spawn admin-configured program + argv[]
  → stdin/stdout raw byte stream
  → SSH handshake / strict known_hosts / auth / SFTP
  → remote logs
```

典型 WSL 场景：WSL 网络无法访问企业 VPN 内的 SSH Server，但 Windows 宿主机可以访问，此时可让 WSL 内的 `log-query-mcp` 启动 Windows `ncat.exe` 等纯 TCP helper。

示例：

```json
{
  "connection_id": "inventory-vpn-proxy-ssh",
  "type": "ssh",
  "host": "inventory-vpn.internal",
  "port": 22,
  "username": "log-reader",
  "auth": {
    "type": "password",
    "secret_ref": "LOG_QUERY_MCP_INVENTORY_PASSWORD"
  },
  "host_key": {
    "known_hosts_file": "/etc/log-query-mcp/known_hosts"
  },
  "proxy": {
    "type": "command",
    "program": "ncat.exe",
    "args": ["{host}", "{port}"]
  }
}
```

ProxyCommand 的边界是固定的：

- 只允许管理员静态配置 `program + args[]`。
- 不生成 Shell command string，不使用 `sh -c` / `powershell -Command` 拼接命令。
- MVP 只允许整个 argv 元素为 `{host}` 或 `{port}`。
- ProxyCommand 不接收 password、private-key passphrase、remote path 或 MCP 请求参数。
- stdout 是 SSH raw byte stream；Host Key Verification 仍针对逻辑目标 `host:port`。
- ProxyCommand 不增加 `run_command`、`ssh_exec`、upload/write/delete/deploy/restart 等能力。

## MCP 工具

v1/v2 的 AI-facing 工具面保持一致：

```text
list_log_sources
search_logs
get_log_context
```

核心能力：

- 按字面量关键字搜索一个或多个来源。
- 支持 `requestId`、`traceId`、异常名、错误码和业务 ID。
- 支持 RFC 3339 时间范围、分页和有限上下文。
- 支持 Local + Remote Source 混合查询。
- 限制扫描字节、结果数量、响应大小、上下文、SSH 连接和同步流量。
- 不接受客户端服务器路径，不执行 Shell，不修改日志。

明确不提供：

```text
ssh_exec
run_shell
remote_read(path)
write/upload/delete
remote deploy/restart
```

## 当前生产入口

- 正式服务：`log-query-mcp`，Streamable HTTP，默认 `http://127.0.0.1:8000/mcp`。
- 调试入口：`log-query-mcp-stdio`，stdout 仅输出 MCP stdio 协议，诊断写 stderr。
- 发布包：`log-query-mcp-v{version}-x86_64-unknown-linux-gnu.tar.gz`。
- 安装形态：`tar.gz + systemd`。

## 快速安装

```bash
VERSION=0.2.0
TARGET=x86_64-unknown-linux-gnu
BASE_URL="https://github.com/liqiangcc/log-query-mcp/releases/download/v${VERSION}"

curl -fL -O "${BASE_URL}/log-query-mcp-v${VERSION}-${TARGET}.tar.gz"
curl -fL -O "${BASE_URL}/SHA256SUMS"
sha256sum -c SHA256SUMS

tar -xzf "log-query-mcp-v${VERSION}-${TARGET}.tar.gz"
cd "log-query-mcp-v${VERSION}-${TARGET}"
sudo scripts/install.sh
```

安装脚本会写入：

| 类型 | 路径 |
|---|---|
| binary | `/opt/log-query-mcp/bin` |
| config | `/etc/log-query-mcp/config.json` |
| writable data/cache root | `/var/lib/log-query-mcp` |
| systemd unit | `/etc/systemd/system/log-query-mcp.service` |
| service user | `log-query-mcp` |

安装器为了向后兼容仍默认写入 v1 Local 示例。要使用 Remote Source，管理员显式切换为发布包内的 v2 示例并修改主机、来源和 Secret：

```bash
sudo cp examples/log-query-mcp.v2.remote.json /etc/log-query-mcp/config.json
sudo chown root:log-query-mcp /etc/log-query-mcp/config.json
sudo chmod 0640 /etc/log-query-mcp/config.json
```

v2 示例同时包含 Direct SSH 与 ProxyCommand/WSL 形式。使用 ProxyCommand 前必须把示例中的 `program` 改成目标环境中由管理员安装并批准的 helper。

完整步骤见 [生产安装指南](./docs/INSTALL.md)。

## 安全升级与回滚

发布包包含 `healthcheck.sh`、`upgrade.sh` 和 `rollback.sh`。升级会先校验包内 SHA256，备份当前 binaries、`BUILDINFO`、配置和 systemd unit，再使用同目录临时文件 + rename 原子替换运行文件。正常升级**不会覆盖现有生产配置**。

```bash
sudo scripts/upgrade.sh /path/to/log-query-mcp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
```

升级完成后不是只看进程存活：标准 `healthcheck.sh` 同时要求 `systemctl is-active` 和 MCP `/mcp` `initialize` 协议响应正确。restart 或协议健康检查失败时会自动从本次 backup 回滚。

也可以显式回滚：

```bash
sudo scripts/rollback.sh /var/lib/log-query-mcp/backups/<backup-dir>
```

回滚后同样必须通过 service + MCP protocol health check。

Release Gate 会执行隔离的 health-check 和 upgrade/rollback 演练，覆盖：正常升级、显式回滚、restart 失败自动回滚、损坏包在修改前拒绝、配置保持、tar.gz 输入，以及“进程活着但 MCP 协议错误”的失败场景。

## Remote 快速准备

Remote 模式推荐使用专用账号：

```text
log-reader
- no sudo
- read-only log permissions
- preferably SFTP-only / chroot where operationally practical
```

本地 MCP 主机必须准备：

- `/etc/log-query-mcp/known_hosts`
- Password 对应的环境变量 Secret，或只读 private key 文件
- `/var/lib/log-query-mcp/cache` 可写空间
- 若使用 ProxyCommand：管理员批准的 helper 可执行文件以及服务用户可执行权限

WSL + Windows helper 还需要确认：

- Windows executable interop 对运行 `log-query-mcp` 的服务身份可用；
- helper 通过 Windows/VPN 网络可以连接目标 `host:port`；
- Direct path 在需要 ProxyCommand 的验收场景中确实不可用；
- helper stdout 不输出 banner/debug 文本，只承载 TCP/SSH 字节流；
- helper 不接收 SSH Secret，credential 仍由 SSH 层处理。

示例配置：

- [v1 Local 示例](./examples/log-query-mcp.v1.json)
- [v2 Local + Direct Remote + ProxyCommand 示例](./examples/log-query-mcp.v2.remote.json)

## 快速验证

发布包内推荐直接执行：

```bash
sudo scripts/healthcheck.sh
```

它验证 systemd active 和 MCP `initialize`。人工协议请求也可以使用：

```bash
curl -sS http://127.0.0.1:8000/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{
    "jsonrpc":"2.0",
    "id":1,
    "method":"initialize",
    "params":{
      "protocolVersion":"2025-06-18",
      "capabilities":{},
      "clientInfo":{"name":"manual-smoke","version":"0.2.0"}
    }
  }'
```

AI 客户端使用 Streamable HTTP 配置：

```json
{
  "mcpServers": {
    "log-query-mcp": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:8000/mcp"
    }
  }
}
```

MCP Inspector 可用于人工验收工具列表和调用结果。参考 <https://modelcontextprotocol.io/docs/tools/inspector> 和 <https://github.com/modelcontextprotocol/inspector>。

## 生产约束

- 目标平台：Linux kernel `>= 5.6`，首批发布目标为 `x86_64-unknown-linux-gnu` glibc 动态链接二进制。
- 服务不内置客户端认证和 TLS。默认只监听 `127.0.0.1:8000`；暴露到非 loopback 时必须由内网 ACL、反向代理或上层网关负责认证、TLS 和访问控制。
- Local 文件访问：来源白名单 + `openat2()` + `RESOLVE_NO_XDEV` + 普通文件校验。
- Remote 文件访问：管理员配置的 SSH connection/source + Host Key Verification + SFTP read-only + regular-file-only + 本地 cache。
- ProxyCommand：管理员配置的本地 raw-stream adapter；不经过 Shell，不接收 credential，不改变逻辑 SSH target 的 host-key 身份。
- Remote 默认 `on_query` freshness；网络/认证/ProxyCommand 失败时 fail-closed，不静默使用 stale cache。
- cache 目录/文件分别按 0700/0600 管理，不保存 credential；内部路径使用 opaque IDs。
- 排序当前只支持 `oldest_first`。
- `match_ref` 和 cursor：单实例、服务端有状态、短期随机 token，重启后失效。
- 错误：稳定错误代码 + 去敏消息 + `retryable`，不暴露 Secret、远端绝对路径、cache 路径、ProxyCommand 完整 argv/stderr、backtrace 或底层系统调用文本。

## 性能边界

M6 已建立 Direct SSH 的可重复工程 benchmark，而不是产品 SLA。历史基线证明：

- unchanged refresh 只读取 64 KiB continuity window；
- append 只传新增 payload + bounded probes；
- cache local scan 不访问 SSH；
- 1 GiB full bootstrap 可以完成；
- 10 GiB logical log 可只缓存 64 MiB tail；
- 300 次连续 bounded range read 不泄漏 SFTP file handle。

M7 已新增 paired Direct/ProxyCommand performance harness，覆盖 5 次 connection setup、100 MiB/1 GiB/10 GiB-tail、incremental bounded transfer、300 Proxy range reads、2 Direct + 2 Proxy concurrency 和正常路径 helper 回收。当前 M7 workflow 仍因 GitHub Actions Billing 在 runner 启动前被阻断，因此尚无 M7 实测数字，不能把 M6 数字冒充 M7 结果。

详见 [M6 性能基线](./docs/M6_PERFORMANCE_BASELINE_V2.md) 与 [M7 Proxy Performance Gate](./docs/M7_PROXY_PERFORMANCE_GATE_V2.md)。

## Release Candidate 状态

M0-M6 历史实现/证据已完成；M7 ProxyCommand 的核心实现、功能/故障/同步/性能 harness 和 Release Integration 已进入候选分支。当前仍有两个不可省略的最终条件：

1. GitHub Actions Billing/Spending Limit 恢复后，让当前 candidate 的所有 gates 真正执行并 PASS；
2. 完成真实 `WSL → Windows Host helper → Remote SSH` 目标环境验收。

当前状态：

```text
M7 implementation/harness  IMPLEMENTED
release integration        IMPLEMENTED
latest candidate CI        BLOCKED externally
WSL target acceptance      PENDING
RC Ready                   NO
formal Release             NOT CREATED
```

在本地 Linux 环境可先执行全部非 live-SSH 仓库 Gate：

```bash
bash scripts/rc_check.sh
```

`rc_check.sh` 现在还验证 v2 release example 同时保留 Direct SSH 与 ProxyCommand，并要求 release package 包含 v2 machine schema 和 M7 ProxyCommand 交付文档。该命令不能替代真实 Direct/Proxy SSH live Gate，也不能替代目标 WSL/生产服务器验收。

## 文档索引

- [生产安装指南](./docs/INSTALL.md)
- [生产运维指南](./docs/OPERATIONS.md)
- [生产验收清单](./docs/PRODUCTION_CHECKLIST.md)
- [v2 Release Readiness](./docs/RELEASE_READINESS_V2.md)
- [ProxyCommand Transport 设计](./docs/PROXY_COMMAND_TRANSPORT_V2.md)
- [M7 ProxyCommand 实现基线](./docs/M7_PROXY_COMMAND_IMPLEMENTATION_BASELINE_V2.md)
- [M7 ProxyCommand Failure Matrix](./docs/M7_PROXY_COMMAND_FAILURE_MATRIX_V2.md)
- [M7 Proxy Auth Gate](./docs/M7_PROXY_AUTH_GATE_V2.md)
- [M7 Proxy Sync Gate](./docs/M7_PROXY_SYNC_GATE_V2.md)
- [M7 Proxy Restart Gate](./docs/M7_PROXY_RESTART_GATE_V2.md)
- [M7 Proxy Generation Gate](./docs/M7_PROXY_GENERATION_GATE_V2.md)
- [M7 Proxy Performance Gate](./docs/M7_PROXY_PERFORMANCE_GATE_V2.md)
- [M6 Final Baseline](./docs/M6_FINAL_BASELINE_V2.md)
- [M6 性能基线](./docs/M6_PERFORMANCE_BASELINE_V2.md)
- [M6 安全/故障矩阵](./docs/M6_SECURITY_FAULT_MATRIX_V2.md)
- [v2 Remote SSH + Cache 设计](./docs/REMOTE_SSH_CACHE_DESIGN_V2.md)
- [v2 Remote 实施 TODO](./docs/REMOTE_SSH_CACHE_TODO_V2.md)
- [生产发布迭代计划](./docs/ITERATION_PLAN.md)
- [v2 SSH/SFTP 发布执行手册](./docs/V2_RELEASE_EXECUTION.md)
- [v1 实现基线](./docs/IMPLEMENTATION_BASELINE_V1.md)
- [v1 MCP API](./docs/MCP_API_V1.md)
- [v1 错误模型](./docs/ERROR_MODEL_V1.md)
- [v2 配置 Schema 说明](./docs/CONFIG_SCHEMA_V2.md)
- [架构决策记录](./docs/adr/README.md)
- [MCP 工具机器 Schema](./schemas/mcp-tools-v1.schema.json)
- [v2 工具错误机器 Schema](./schemas/tool-error-v2.schema.json)
- [v1 服务配置机器 Schema](./schemas/log-query-mcp-config-v1.schema.json)
- [v2 服务配置机器 Schema](./schemas/log-query-mcp-config-v2.schema.json)
- [完整需求文档](./REQUIREMENTS.md)

## 开发验证

完整本地 Final Candidate（不含真实 SSH/Proxy live/生产环境）：

```bash
bash scripts/rc_check.sh
```

或分步执行：

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --release --locked --bins
python3 scripts/validate_contracts.py
bash tests/healthcheck_test.sh
bash tests/upgrade_rollback_test.sh
```

发布包 dry-run：

```bash
cargo build --release --locked --bins --target x86_64-unknown-linux-gnu
bash scripts/package_release.sh --target x86_64-unknown-linux-gnu --out-dir dist --require-docs
bash scripts/validate_release_package.sh dist/log-query-mcp-v0.2.0-x86_64-unknown-linux-gnu.tar.gz dist/SHA256SUMS
```

## 当前不包含

- `newest_first` 实际扫描。
- 正则表达式或复杂查询语言。
- 压缩日志和实时 follow。
- Kubernetes、Loki、Elasticsearch。
- 多实例共享 cursor / `match_ref`。
- Remote Exec、部署、文件上传或远端写入。
- ProxyCommand 动态 MCP 参数、Shell command string 或 credential/env 注入。
- 自动根因分析和代码修复。

## License

暂未指定。
