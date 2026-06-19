# Log Query MCP

面向 AI 问题排查的只读日志搜索 MCP 服务。

服务部署在能够访问日志文件的 Linux 服务器上。管理员配置允许查询的日志来源；本地 AI 通过 MCP 搜索运行日志，并结合本地代码仓库定位开发环境或测试环境问题。

> 当前为技术预研原型。真实文件搜索、分页和上下文读取链路已经接通，但尚未达到生产发布状态。

## 工作方式

```text
用户描述问题
      ↓
本地 AI 读取代码仓库
      ↓
AI 调用 Log Query MCP
      ↓
MCP 搜索白名单日志文件
      ↓
AI 结合代码与日志诊断问题
```

职责划分：

- **本地 AI**：读取代码、分析日志、诊断问题。
- **Log Query MCP**：安全搜索服务器日志，返回匹配位置和有限上下文。
- **管理员**：配置允许查询的日志文件及只读权限。

## 当前能力

- `list_log_sources`：查询已配置日志来源，不暴露绝对路径。
- `search_logs`：对一个或多个来源执行字面量子串搜索。
- `get_log_context`：通过短期不透明 `match_ref` 读取有限前后文。
- 支持 `requestId`、`traceId`、异常名、错误码和业务 ID。
- 支持时间范围、跨来源结果排序和分页游标。
- 限制扫描字节、结果数量、上下文、响应大小和并发任务。
- 使用 Linux `openat2()` 阻止路径逃逸和软链接访问。
- 不执行 Shell，不接受客户端文件路径，不修改日志。

## 技术栈

```text
Rust stable
rmcp 1.7
Tokio
Axum / Streamable HTTP
rustix / openat2
Serde / Schemars
```

目标平台：

```text
Linux kernel >= 5.6
```

## 快速运行

### 1. 构建

```bash
cargo build --release --locked
```

### 2. 准备配置

```bash
cp deploy/log-query-mcp.example.json log-query-mcp.json
```

修改 `root` 和 `files`，确保文件真实存在且当前用户只有读取权限。

当前配置使用显式文件列表：

```json
{
  "sources": [
    {
      "source_id": "payment-test",
      "name": "支付服务测试环境",
      "description": "payment-service application logs",
      "service": "payment-service",
      "environment": "test",
      "tags": ["payment", "java"],
      "root": "/var/log/payment-service",
      "files": ["application.log", "application.log.1"],
      "timestamp_rule": {
        "type": "rfc3339",
        "prefix_bytes": 64
      }
    }
  ]
}
```

客户端不能提交或覆盖这些路径。

### 3. 启动

```bash
LOG_QUERY_MCP_CONFIG=./log-query-mcp.json \
LOG_QUERY_MCP_BIND=127.0.0.1:8000 \
RUST_LOG=log_query_mcp=info \
  target/release/log-query-mcp
```

MCP 地址：

```text
http://127.0.0.1:8000/mcp
```

### 4. 验证

使用 MCP Inspector 连接 Streamable HTTP 地址：

```bash
npx @modelcontextprotocol/inspector
```

依次验证：

1. `list_log_sources`
2. `search_logs`
3. 使用搜索结果中的 `match_ref` 调用 `get_log_context`

## MCP 工具示例

### `search_logs`

```json
{
  "source_ids": ["payment-test", "order-test"],
  "keyword": "traceId=abc123",
  "case_sensitive": false,
  "start_time": "2026-06-19T14:00:00+09:00",
  "end_time": "2026-06-19T15:00:00+09:00",
  "order": "oldest_first",
  "max_results": 50
}
```

响应中的 `match_ref` 和 `next_cursor` 均为短期、服务端有状态的不透明 token。服务重启后会失效。

### `get_log_context`

```json
{
  "match_ref": "mref_...",
  "before_lines": 10,
  "after_lines": 30
}
```

## 安全边界

- 仅能选择管理员配置的 `source_id`。
- 客户端不能提交服务器路径、glob 或目录。
- 文件从预先打开的来源根目录解析。
- 禁止路径穿越、软链接和 magic link。
- 打开后只接受普通文件。
- 服务应使用非 root 账号和只读文件权限。
- 日志内容被视为不可信数据，不作为 MCP 指令执行。
- 首期仅用于受控内网，不内置认证和 TLS。

## Linux 部署

参阅：

- [Linux 与 systemd 部署](./docs/DEPLOYMENT.md)
- [示例配置](./deploy/log-query-mcp.example.json)
- [systemd unit](./deploy/log-query-mcp.service)

## 性能验证

```bash
cargo build --release --locked --bin log-query-benchmark

python3 research/scripts/generate_benchmark_log.py \
  --output /tmp/log-query-benchmark.log \
  --size-mib 1024

target/release/log-query-benchmark \
  /tmp/log-query-benchmark.log \
  __NO_MATCH__ \
  3
```

完整方法见 [R-10 性能基准执行指南](./docs/PERFORMANCE_BENCHMARK.md)。

## 当前限制

- 日志文件仍需显式配置，尚未提供目录 glob 自动发现。
- `newest_first` 尚未接入真实前向扫描链路。
- 查询资源限制目前主要使用代码默认值。
- 暂无健康检查端点和配置热加载。
- 多实例不共享 `match_ref` 和游标。
- 尚未完成真实服务器上的 1 GiB、10 GiB 和大量小文件基准。
- 尚未完成目标 AI 客户端兼容性记录。

## 文档

- [需求文档](./REQUIREMENTS.md)
- [技术预研计划](./docs/TECHNICAL_RESEARCH_PLAN.md)
- [工具 Schema 草案](./docs/TOOL_SCHEMA_DRAFT.md)
- [安全文件访问预研](./docs/SAFE_FILE_ACCESS_RESEARCH.md)
- [扫描器预研](./docs/SCANNER_RESEARCH.md)
- [执行器预研](./docs/EXECUTOR_RESEARCH.md)
- [匹配引用预研](./docs/MATCH_REFERENCE_RESEARCH.md)
- [分页游标预研](./docs/SEARCH_CURSOR_RESEARCH.md)
- [时间范围预研](./docs/TIME_FILTER_RESEARCH.md)
- [部署指南](./docs/DEPLOYMENT.md)
- [性能基准](./docs/PERFORMANCE_BENCHMARK.md)

## License

暂未指定。
