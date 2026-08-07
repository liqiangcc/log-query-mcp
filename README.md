# Log Query MCP

面向 AI 问题排查的只读日志搜索 MCP 服务。

Log Query MCP 只向 AI 暴露受控的日志语义能力，不暴露 Shell、任意文件读取或 SSH Exec。管理员预先配置允许查询的来源，AI 只通过 `source_id` 搜索日志并读取有限上下文。

## 部署模式

支持两种来源模式，并且可以在同一个查询中混合使用。

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
VERSION=0.1.0
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

完整步骤见 [生产安装指南](./docs/INSTALL.md)。

## 安全升级与回滚

发布包包含 `upgrade.sh` 和 `rollback.sh`。升级会先校验包内 SHA256，备份当前 binaries、`BUILDINFO`、配置和 systemd unit，再使用同目录临时文件 + rename 原子替换运行文件。正常升级**不会覆盖现有生产配置**。

```bash
sudo scripts/upgrade.sh /path/to/log-query-mcp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
```

升级后的 restart/health check 失败时会自动从本次 backup 回滚。也可以显式回滚：

```bash
sudo scripts/rollback.sh /var/lib/log-query-mcp/backups/<backup-dir>
```

Release Gate 会执行隔离的 upgrade/rollback 演练，覆盖：正常升级、显式回滚、restart 失败自动回滚、损坏包在修改前拒绝、配置保持和 tar.gz 输入。

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

示例配置：

- [v1 Local 示例](./examples/log-query-mcp.v1.json)
- [v2 Local + Remote 示例](./examples/log-query-mcp.v2.remote.json)

## 快速验证

确认服务监听 loopback：

```bash
ss -ltn | grep '127.0.0.1:8000'
```

执行 MCP 初始化请求：

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
      "clientInfo":{"name":"manual-smoke","version":"0.1.0"}
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
- Remote 默认 `on_query` freshness；网络/认证失败时 fail-closed，不静默使用 stale cache。
- cache 目录/文件分别按 0700/0600 管理，不保存 credential；内部路径使用 opaque IDs。
- 排序当前只支持 `oldest_first`。
- `match_ref` 和 cursor：单实例、服务端有状态、短期随机 token，重启后失效。
- 错误：稳定错误代码 + 去敏消息 + `retryable`，不暴露 Secret、远端绝对路径、cache 路径、backtrace 或底层系统调用文本。

## 性能边界

M6 已建立可重复工程 benchmark，而不是产品 SLA。当前基线证明：

- unchanged refresh 只读取 64 KiB continuity window；
- append 只传新增 payload + bounded probes；
- cache local scan 不访问 SSH；
- 1 GiB full bootstrap 可以完成；
- 10 GiB logical log 可只缓存 64 MiB tail；
- 300 次连续 bounded range read 不泄漏 SFTP file handle。

详见 [M6 性能基线](./docs/M6_PERFORMANCE_BASELINE_V2.md)。

## 文档索引

- [生产安装指南](./docs/INSTALL.md)
- [生产运维指南](./docs/OPERATIONS.md)
- [生产验收清单](./docs/PRODUCTION_CHECKLIST.md)
- [v2 Release Readiness](./docs/RELEASE_READINESS_V2.md)
- [M6 性能基线](./docs/M6_PERFORMANCE_BASELINE_V2.md)
- [M6 安全/故障矩阵](./docs/M6_SECURITY_FAULT_MATRIX_V2.md)
- [v2 Remote SSH + Cache 设计](./docs/REMOTE_SSH_CACHE_DESIGN_V2.md)
- [v2 Remote 实施 TODO](./docs/REMOTE_SSH_CACHE_TODO_V2.md)
- [M5 Remote Query 实现基线](./docs/M5_IMPLEMENTATION_BASELINE_V2.md)
- [v1 实现基线](./docs/IMPLEMENTATION_BASELINE_V1.md)
- [v1 MCP API](./docs/MCP_API_V1.md)
- [v1 错误模型](./docs/ERROR_MODEL_V1.md)
- [v1 配置 Schema 说明](./docs/CONFIG_SCHEMA_V1.md)
- [架构决策记录](./docs/adr/README.md)
- [MCP 工具机器 Schema](./schemas/mcp-tools-v1.schema.json)
- [v2 工具错误机器 Schema](./schemas/tool-error-v2.schema.json)
- [v1 服务配置机器 Schema](./schemas/log-query-mcp-config-v1.schema.json)
- [v2 服务配置机器 Schema](./schemas/log-query-mcp-config-v2.schema.json)
- [完整需求文档](./REQUIREMENTS.md)

## 开发验证

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --release --locked --bins
python3 scripts/validate_contracts.py
```

发布包 dry-run：

```bash
cargo build --release --locked --bins --target x86_64-unknown-linux-gnu
bash scripts/package_release.sh --target x86_64-unknown-linux-gnu --out-dir dist --require-docs
bash scripts/validate_release_package.sh dist/log-query-mcp-v0.1.0-x86_64-unknown-linux-gnu.tar.gz dist/SHA256SUMS
bash tests/upgrade_rollback_test.sh
```

## 当前不包含

- `newest_first` 实际扫描。
- 正则表达式或复杂查询语言。
- 压缩日志和实时 follow。
- Kubernetes、Loki、Elasticsearch。
- 多实例共享 cursor / `match_ref`。
- Remote Exec、部署、文件上传或远端写入。
- 自动根因分析和代码修复。

## License

暂未指定。