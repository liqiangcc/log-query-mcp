# Log Query MCP

面向 AI 问题排查的只读日志搜索 MCP 服务。

Log Query MCP 部署在能够访问日志文件的 Linux 服务器上。管理员配置允许查询的日志来源，本地或内网 AI 客户端通过 MCP 搜索运行日志，并结合代码仓库定位开发、测试或生产环境问题。

当前生产入口：

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
| systemd unit | `/etc/systemd/system/log-query-mcp.service` |
| service user | `log-query-mcp` |

安装后先编辑 `/etc/log-query-mcp/config.json`，确认日志来源白名单、文件权限和限制参数，再启动服务：

```bash
sudo systemctl enable --now log-query-mcp.service
sudo systemctl status --no-pager log-query-mcp.service
```

完整步骤见 [生产安装指南](./docs/INSTALL.md)。

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

## v1 工具

```text
list_log_sources
search_logs
get_log_context
```

核心能力：

- 按字面量关键字搜索一个或多个来源。
- 支持 `requestId`、`traceId`、异常名、错误码和业务 ID。
- 支持 RFC 3339 时间范围、分页和有限上下文。
- 限制扫描字节、结果数量、响应大小、上下文和并发任务。
- 不接受客户端服务器路径，不执行 Shell，不修改日志。

## 生产约束

- 目标平台：Linux kernel `>= 5.6`，首批发布目标为 `x86_64-unknown-linux-gnu` glibc 动态链接二进制。
- v1 不内置认证和 TLS。默认只监听 `127.0.0.1:8000`；暴露到非 loopback 时必须由内网 ACL、反向代理或上层网关负责认证、TLS 和访问控制。
- 文件访问：来源白名单 + `openat2()` + `RESOLVE_NO_XDEV` + 普通文件校验。
- 排序：v1 只支持 `oldest_first`。
- `match_ref` 和 cursor：单实例、服务端有状态、短期随机 token，重启后失效。
- 错误：稳定错误代码 + 去敏消息 + `retryable`，不暴露绝对路径、inode、offset、backtrace 或底层系统调用文本。

## 文档索引

- [生产安装指南](./docs/INSTALL.md)
- [生产运维指南](./docs/OPERATIONS.md)
- [生产验收清单](./docs/PRODUCTION_CHECKLIST.md)
- [生产发布迭代计划](./docs/ITERATION_PLAN.md)
- [v1 实现基线](./docs/IMPLEMENTATION_BASELINE_V1.md)
- [v1 MCP API](./docs/MCP_API_V1.md)
- [v1 错误模型](./docs/ERROR_MODEL_V1.md)
- [v1 配置 Schema 说明](./docs/CONFIG_SCHEMA_V1.md)
- [架构决策记录](./docs/adr/README.md)
- [Codex 交接说明](./docs/CODEX_HANDOFF.md)
- [MCP 工具机器 Schema](./schemas/mcp-tools-v1.schema.json)
- [工具错误机器 Schema](./schemas/tool-error-v1.schema.json)
- [服务配置机器 Schema](./schemas/log-query-mcp-config-v1.schema.json)
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
scripts/package_release.sh --target x86_64-unknown-linux-gnu --out-dir dist --require-docs
```

## 首期不包含

- `newest_first` 实际扫描。
- 正则表达式或复杂查询语言。
- 压缩日志和实时 tail。
- Kubernetes、Loki、Elasticsearch。
- 多实例共享 cursor / `match_ref`。
- 自动根因分析和代码修复。

## License

暂未指定。
